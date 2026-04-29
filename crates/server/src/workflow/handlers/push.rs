use local_repo::{LocalRepo, PushResult, PushStatus};
use remote_repo::{CreatePrOpts, GhBackend, RemoteRepo};
use tracing::{error, info, warn};

use super::hook_log::write_hook_failure_log;

use ticket_db::LifecycleStatus;
use ur_rpc::workflow_condition;
use ur_rpc::workflow_event::WorkflowEvent;

use crate::workflow::{HandlerFuture, TransitionRequest, WorkflowContext, WorkflowHandler};

/// Handler for the Verifying → Pushing transition.
///
/// Performs the actual git push via `local_repo.push()` through builderd,
/// parses the result, and transitions accordingly:
///
/// - **Success / ForcePushed / UpToDate**: create a PR if none exists, then
///   transition to InReview.
/// - **Rejected (non-fast-forward)** on a non-protected branch: retry with
///   `force_push` (force-with-lease).
/// - **Rejected (non-fast-forward)** on a protected branch: stall the agent.
/// - **RemoteRejected**: stall the agent.
pub struct PushHandler;

impl WorkflowHandler for PushHandler {
    fn handle(&self, ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move { handle_push(&ctx, &ticket_id).await })
    }
}

/// Core push logic, extracted to reduce nesting depth.
async fn handle_push(ctx: &WorkflowContext, ticket_id: &str) -> anyhow::Result<()> {
    // 1. Load ticket to get branch and project.
    let ticket = ctx
        .ticket_repo
        .get_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

    let branch = ticket.branch.as_deref().ok_or_else(|| {
        anyhow::anyhow!("ticket {ticket_id} has no branch set — cannot push without a branch")
    })?;

    let project_key = &ticket.project;

    // 2. Resolve worker and slot to get the working directory.
    let workflow = ctx
        .workflow_repo
        .get_workflow_by_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no workflow found for ticket {ticket_id}"))?;
    if workflow.worker_id.is_empty() {
        anyhow::bail!("no worker_id on workflow for ticket {ticket_id} — cannot run push");
    }
    let worker_id = &workflow.worker_id;

    let worker_slot = ctx
        .worker_repo
        .get_worker_slot(worker_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no slot linked to worker {worker_id}"))?;

    let slot = ctx
        .worker_repo
        .get_slot(&worker_slot.slot_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("slot {} not found in database", worker_slot.slot_id))?;

    let working_dir = &slot.host_path;

    let no_verify = workflow.noverify;

    info!(
        ticket_id = %ticket_id,
        branch = %branch,
        working_dir = %working_dir,
        no_verify = %no_verify,
        "push handler: pushing branch via local_repo"
    );

    // 3. Execute push via local_repo (through builderd).
    let local_repo = local_repo::GitBackend {
        client: ctx.builderd_client.clone(),
    };
    let push_result = local_repo.push(branch, working_dir, no_verify).await?;

    info!(
        ticket_id = %ticket_id,
        branch = %branch,
        status = ?push_result.status,
        summary = %push_result.summary,
        "push result received"
    );

    // 4. Handle based on push status.
    match &push_result.status {
        PushStatus::Success | PushStatus::ForcePushed | PushStatus::UpToDate => {
            handle_push_success(
                ctx,
                ticket_id,
                branch,
                &ticket.title,
                &ticket.body,
                &push_result,
            )
            .await
        }
        PushStatus::Rejected { reason } => {
            let params = RejectedPushParams {
                ctx,
                ticket_id,
                branch,
                title: &ticket.title,
                body: &ticket.body,
                worker_id,
                project_key,
                reason,
                local_repo: &local_repo,
                working_dir,
                no_verify,
            };
            handle_push_rejected(&params).await
        }
        PushStatus::RemoteRejected { reason } => {
            warn!(
                ticket_id = %ticket_id,
                branch = %branch,
                reason = %reason,
                "push remote-rejected — stalling agent"
            );
            add_push_activity(
                ctx,
                ticket_id,
                "remote_rejected",
                &format!("Remote rejected push: {reason}"),
            )
            .await?;
            stall_agent(ctx, ticket_id, worker_id).await
        }
        PushStatus::HookFailed { summary } => {
            handle_hook_failed(ctx, ticket_id, branch, worker_id, summary).await
        }
    }
}

/// Handle a successful push: record activity, create PR, initialize conditions, advance to InReview.
async fn handle_push_success(
    ctx: &WorkflowContext,
    ticket_id: &str,
    branch: &str,
    title: &str,
    body: &str,
    push_result: &PushResult,
) -> anyhow::Result<()> {
    let result_label = push_status_label(&push_result.status);
    add_push_activity(ctx, ticket_id, result_label, &push_result.summary).await?;
    ensure_pr(ctx, ticket_id, branch, title, body).await?;
    initialize_conditions_and_emit_event(ctx, ticket_id).await?;
    advance_to_in_review(ctx, ticket_id).await
}

/// Initialize workflow conditions and emit a pr_created event after PR creation.
///
/// Sets ci_status=pending, mergeable=unknown, review_status=pending (or approved
/// if the ticket has "autoapprove" metadata set).
async fn initialize_conditions_and_emit_event(
    ctx: &WorkflowContext,
    ticket_id: &str,
) -> anyhow::Result<()> {
    ctx.workflow_repo
        .initialize_workflow_conditions(ticket_id)
        .await?;

    // Check autoapprove metadata — if set, override review_status to approved.
    let meta = ctx.ticket_repo.get_meta(ticket_id, "ticket").await?;
    if meta.contains_key(ur_rpc::ticket_meta::AUTOAPPROVE) {
        ctx.workflow_repo
            .update_workflow_condition(
                ticket_id,
                workflow_condition::WorkflowCondition::ReviewStatus,
                workflow_condition::review_status::APPROVED,
            )
            .await?;
        info!(ticket_id = %ticket_id, "autoapprove set — review_status initialized to approved");
    }

    // Emit pr_created workflow event.
    let workflow = ctx.workflow_repo.get_workflow_by_ticket(ticket_id).await?;
    match workflow {
        Some(w) => {
            if let Err(e) = ctx
                .workflow_repo
                .insert_workflow_event(&w.id, WorkflowEvent::PrCreated)
                .await
            {
                error!(
                    error = %e,
                    ticket_id = %ticket_id,
                    "failed to insert pr_created workflow event"
                );
            }
        }
        None => {
            error!(
                ticket_id = %ticket_id,
                "no workflow found when emitting pr_created event"
            );
        }
    }

    Ok(())
}

/// Parameters for handling a rejected push, grouped to keep argument count manageable.
struct RejectedPushParams<'a> {
    ctx: &'a WorkflowContext,
    ticket_id: &'a str,
    branch: &'a str,
    title: &'a str,
    body: &'a str,
    worker_id: &'a str,
    project_key: &'a str,
    reason: &'a str,
    local_repo: &'a dyn LocalRepo,
    working_dir: &'a str,
    no_verify: bool,
}

/// Handle a rejected push: force-push on non-protected branches, stall on protected.
async fn handle_push_rejected(params: &RejectedPushParams<'_>) -> anyhow::Result<()> {
    let RejectedPushParams {
        ctx,
        ticket_id,
        branch,
        title,
        body,
        worker_id,
        project_key,
        reason,
        local_repo,
        working_dir,
        no_verify,
    } = params;
    let protected = is_branch_protected(branch, ctx, project_key);

    if protected {
        warn!(
            ticket_id = %ticket_id,
            branch = %branch,
            reason = %reason,
            "push rejected on protected branch — stalling agent"
        );
        add_push_activity(
            ctx,
            ticket_id,
            "rejected_protected",
            &format!("Push rejected on protected branch: {reason}"),
        )
        .await?;
        return stall_agent(ctx, ticket_id, worker_id).await;
    }

    // Retry with force-with-lease on non-protected branch.
    info!(
        ticket_id = %ticket_id,
        branch = %branch,
        reason = %reason,
        "push rejected (non-fast-forward) on non-protected branch — retrying with force-with-lease"
    );

    let force_result = local_repo
        .force_push(branch, working_dir, *no_verify)
        .await?;

    info!(
        ticket_id = %ticket_id,
        branch = %branch,
        status = ?force_result.status,
        summary = %force_result.summary,
        "force push result received"
    );

    match &force_result.status {
        PushStatus::Success | PushStatus::ForcePushed | PushStatus::UpToDate => {
            let force_result_with_context = PushResult {
                status: force_result.status.clone(),
                ref_name: force_result.ref_name.clone(),
                summary: format!("Force push after rejection: {}", force_result.summary),
            };
            handle_push_success(
                ctx,
                ticket_id,
                branch,
                title,
                body,
                &force_result_with_context,
            )
            .await
        }
        PushStatus::Rejected {
            reason: retry_reason,
        }
        | PushStatus::RemoteRejected {
            reason: retry_reason,
        } => {
            warn!(
                ticket_id = %ticket_id,
                branch = %branch,
                reason = %retry_reason,
                "force push also rejected — stalling agent"
            );
            add_push_activity(
                ctx,
                ticket_id,
                "rejected",
                &format!("Force push also rejected: {retry_reason}"),
            )
            .await?;
            stall_agent(ctx, ticket_id, worker_id).await
        }
        PushStatus::HookFailed { summary } => {
            handle_hook_failed(ctx, ticket_id, branch, worker_id, summary).await
        }
    }
}

/// Handle a pre-push hook failure: write logs, record activity, and send transition to Implementing.
async fn handle_hook_failed(
    ctx: &WorkflowContext,
    ticket_id: &str,
    branch: &str,
    worker_id: &str,
    summary: &str,
) -> anyhow::Result<()> {
    warn!(
        ticket_id = %ticket_id,
        branch = %branch,
        "pre-push hook failed — sending transition to implementing"
    );
    let log_path = write_hook_failure_log(&ctx.config.logs_dir, worker_id, "push", summary, "", 1);
    let first_line = format!("[workflow] push failed — logs at {log_path}");
    let message = format!(
        "{first_line}\n\
         source: workflow\n\
         result: hook_failed\n\
         ---\n\
         {summary}"
    );
    ctx.ticket_repo
        .add_activity(ticket_id, "workflow", &message)
        .await?;
    ctx.transition_tx
        .send(TransitionRequest {
            ticket_id: ticket_id.to_owned(),
            target_status: LifecycleStatus::Implementing,
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to send Implementing transition: {e}"))?;
    Ok(())
}

/// Map a PushStatus to a human-readable label for activity logs.
fn push_status_label(status: &PushStatus) -> &'static str {
    match status {
        PushStatus::Success => "success",
        PushStatus::ForcePushed => "force_pushed",
        PushStatus::UpToDate => "up_to_date",
        PushStatus::Rejected { .. } => "rejected",
        PushStatus::RemoteRejected { .. } => "remote_rejected",
        PushStatus::HookFailed { .. } => "hook_failed",
    }
}

/// Check whether a branch name matches any protected branch pattern.
fn is_branch_protected(branch: &str, ctx: &WorkflowContext, project_key: &str) -> bool {
    let protected_branches = match ctx.config.projects.get(project_key) {
        Some(pc) => &pc.protected_branches,
        None => return default_is_protected(branch),
    };

    for pattern in protected_branches {
        if pattern_matches(pattern, branch) {
            return true;
        }
    }
    false
}

/// Default protection check when no project config exists.
fn default_is_protected(branch: &str) -> bool {
    branch == "main" || branch == "master"
}

/// Simple glob pattern matching supporting `*` wildcards.
///
/// Supports patterns like `release/*`, `main`, `hotfix/**`.
fn pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern == value {
        return true;
    }
    // `**` matches across path separators.
    let parts: Vec<&str> = pattern.split("**").collect();
    if parts.len() > 1 {
        return glob_match_double_star(&parts, value);
    }
    // `*` matches anything except `/`.
    glob_match_single_star(pattern, value)
}

fn glob_match_double_star(parts: &[&str], value: &str) -> bool {
    if parts.len() == 1 {
        return glob_match_single_star(parts[0], value);
    }
    let first = parts[0];
    let last = parts[parts.len() - 1];

    if !first.is_empty() && !value.starts_with(first) {
        return false;
    }
    if !last.is_empty() && !value.ends_with(last) {
        return false;
    }
    true
}

fn glob_match_single_star(pattern: &str, value: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }

    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            // For non-trailing empty segments, verify no `/` in the gap.
            if i > 0 && i < parts.len() - 1 {
                // There's a `*` here — it must not span a `/`.
                // (handled below via the slash check)
            }
            continue;
        }
        let remaining = &value[pos..];
        match remaining.find(part) {
            Some(found) => {
                if i == 0 && found != 0 {
                    return false;
                }
                // The gap between pos and the match must not contain `/`
                // (single `*` doesn't cross path separators).
                if i > 0 && remaining[..found].contains('/') {
                    return false;
                }
                pos += found + part.len();
            }
            None => return false,
        }
    }
    if !pattern.ends_with('*') {
        return pos == value.len();
    }
    // Trailing `*` — remaining value must not contain `/`.
    !value[pos..].contains('/')
}

/// Derive the GitHub `owner/repo` identifier from a git remote URL.
///
/// Supports:
/// - `git@github.com:owner/repo.git`
/// - `https://github.com/owner/repo.git`
/// - `owner/repo` (passthrough)
fn gh_repo_from_url(url: &str) -> Option<String> {
    if let Some(path) = url.strip_prefix("git@github.com:") {
        let trimmed = path.trim_end_matches(".git");
        return Some(trimmed.to_string());
    }
    if let Some(rest) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        let trimmed = rest.trim_end_matches(".git");
        return Some(trimmed.to_string());
    }
    if url.contains('/') && !url.contains(':') && !url.contains("//") {
        return Some(url.to_string());
    }
    None
}

/// Ensure a PR exists for this ticket's branch.
///
/// If `pr_number` metadata already exists, skip creation. Otherwise, create
/// a new PR and store `pr_number`, `pr_url`, and `gh_repo` metadata.
async fn ensure_pr(
    ctx: &WorkflowContext,
    ticket_id: &str,
    branch: &str,
    title: &str,
    body: &str,
) -> anyhow::Result<()> {
    let meta = ctx.ticket_repo.get_meta(ticket_id, "ticket").await?;

    if meta.contains_key("pr_number") {
        info!(
            ticket_id = %ticket_id,
            pr_number = meta.get("pr_number").unwrap(),
            "PR already exists — skipping creation"
        );
        return Ok(());
    }

    let gh_repo = resolve_gh_repo(ctx, ticket_id, &meta).await?;

    let pr_body = if body.is_empty() {
        format!("Ticket: {ticket_id}")
    } else {
        format!("{body}\n\nTicket: {ticket_id}")
    };

    let opts = CreatePrOpts {
        title: build_pr_title(title, &meta),
        body: pr_body,
        head: branch.to_string(),
        base: String::new(),
        draft: false,
    };

    info!(
        ticket_id = %ticket_id,
        gh_repo = %gh_repo,
        branch = %branch,
        "creating PR via GhBackend"
    );

    let backend = GhBackend {
        client: ctx.builderd_client.clone(),
        gh_repo: gh_repo.clone(),
    };

    let pr = backend.create_pr(opts).await?;

    info!(
        ticket_id = %ticket_id,
        pr_number = pr.number,
        pr_url = %pr.url,
        "PR created"
    );

    ctx.ticket_repo
        .set_meta(ticket_id, "ticket", "pr_number", &pr.number.to_string())
        .await?;
    ctx.ticket_repo
        .set_meta(ticket_id, "ticket", "pr_url", &pr.url)
        .await?;
    ctx.ticket_repo
        .set_meta(ticket_id, "ticket", "gh_repo", &gh_repo)
        .await?;

    Ok(())
}

/// Resolve `gh_repo` from ticket metadata or derive it from the project config.
async fn resolve_gh_repo(
    ctx: &WorkflowContext,
    ticket_id: &str,
    meta: &std::collections::HashMap<String, String>,
) -> anyhow::Result<String> {
    if let Some(r) = meta.get("gh_repo") {
        return Ok(r.clone());
    }

    let ticket = ctx
        .ticket_repo
        .get_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

    let project_config = ctx.config.projects.get(&ticket.project).ok_or_else(|| {
        anyhow::anyhow!(
            "no project config for '{}' — cannot determine gh_repo for PR creation",
            ticket.project
        )
    })?;

    let derived = gh_repo_from_url(&project_config.repo).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot derive GitHub owner/repo from project repo URL '{}' for project '{}'",
            project_config.repo,
            ticket.project
        )
    })?;

    ctx.ticket_repo
        .set_meta(ticket_id, "ticket", "gh_repo", &derived)
        .await?;

    Ok(derived)
}

/// Send a transition request to InReview via the coordinator channel.
async fn advance_to_in_review(ctx: &WorkflowContext, ticket_id: &str) -> anyhow::Result<()> {
    ctx.transition_tx
        .send(TransitionRequest {
            ticket_id: ticket_id.to_owned(),
            target_status: LifecycleStatus::InReview,
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to send InReview transition: {e}"))?;
    Ok(())
}

/// Record a push activity on the ticket with workflow metadata.
async fn add_push_activity(
    ctx: &WorkflowContext,
    ticket_id: &str,
    result: &str,
    detail: &str,
) -> anyhow::Result<()> {
    let message = format!(
        "[workflow] push {result}\n\
         source: workflow\n\
         result: {result}\n\
         ---\n\
         {detail}"
    );
    ctx.ticket_repo
        .add_activity(ticket_id, "workflow", &message)
        .await?;
    Ok(())
}

/// Set the worker's agent_status to "stalled".
async fn stall_agent(
    ctx: &WorkflowContext,
    ticket_id: &str,
    worker_id: &str,
) -> anyhow::Result<()> {
    ctx.worker_repo
        .update_worker_agent_status(worker_id, workflow_db::AgentStatus::Stalled)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to set agent_status to stalled for worker {worker_id} \
                 (ticket {ticket_id}): {e}"
            )
        })?;
    Ok(())
}

/// Build a PR title by optionally prepending a `ref` metadata value.
///
/// If the ticket has a `ref` metadata key whose trimmed value is non-empty,
/// the title is formatted as `"<ref> <title>"`. Otherwise `title` is returned
/// unchanged.
pub fn build_pr_title(title: &str, meta: &std::collections::HashMap<String, String>) -> String {
    match meta.get(ur_rpc::ticket_meta::REF) {
        Some(r) => {
            let trimmed = r.trim();
            if trimmed.is_empty() {
                title.to_string()
            } else {
                format!("{trimmed} {title}")
            }
        }
        None => title.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_matches_exact() {
        assert!(pattern_matches("main", "main"));
        assert!(!pattern_matches("main", "master"));
    }

    #[test]
    fn pattern_matches_single_star() {
        assert!(pattern_matches("release/*", "release/1.0"));
        assert!(pattern_matches("release/*", "release/v2"));
        assert!(!pattern_matches("release/*", "main"));
        assert!(!pattern_matches("release/*", "release/1.0/hotfix"));
    }

    #[test]
    fn pattern_matches_double_star() {
        assert!(pattern_matches("release/**", "release/1.0"));
        assert!(pattern_matches("release/**", "release/1.0/hotfix"));
        assert!(!pattern_matches("release/**", "main"));
    }

    #[test]
    fn gh_repo_from_ssh_url() {
        assert_eq!(
            gh_repo_from_url("git@github.com:owner/repo.git"),
            Some("owner/repo".to_string())
        );
    }

    #[test]
    fn gh_repo_from_https_url() {
        assert_eq!(
            gh_repo_from_url("https://github.com/owner/repo.git"),
            Some("owner/repo".to_string())
        );
    }

    #[test]
    fn gh_repo_from_nwo() {
        assert_eq!(
            gh_repo_from_url("owner/repo"),
            Some("owner/repo".to_string())
        );
    }

    #[test]
    fn gh_repo_from_unknown_url() {
        assert_eq!(gh_repo_from_url("git@gitlab.com:foo/bar.git"), None);
    }

    #[test]
    fn default_protected_branches() {
        assert!(default_is_protected("main"));
        assert!(default_is_protected("master"));
        assert!(!default_is_protected("feature/foo"));
    }

    // build_pr_title tests

    fn meta_with_ref(val: &str) -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert(ur_rpc::ticket_meta::REF.to_string(), val.to_string());
        m
    }

    #[test]
    fn build_pr_title_no_ref() {
        let meta = std::collections::HashMap::new();
        assert_eq!(build_pr_title("My feature", &meta), "My feature");
    }

    #[test]
    fn build_pr_title_with_ref() {
        let meta = meta_with_ref("JIRA-123");
        assert_eq!(build_pr_title("My feature", &meta), "JIRA-123 My feature");
    }

    #[test]
    fn build_pr_title_ref_with_leading_trailing_spaces() {
        let meta = meta_with_ref("  JIRA-456  ");
        assert_eq!(build_pr_title("Fix the bug", &meta), "JIRA-456 Fix the bug");
    }

    #[test]
    fn build_pr_title_ref_with_internal_spaces() {
        let meta = meta_with_ref("PROJECT 789");
        assert_eq!(
            build_pr_title("Add feature", &meta),
            "PROJECT 789 Add feature"
        );
    }

    #[test]
    fn build_pr_title_empty_ref() {
        let meta = meta_with_ref("");
        assert_eq!(build_pr_title("My feature", &meta), "My feature");
    }

    #[test]
    fn build_pr_title_whitespace_only_ref() {
        let meta = meta_with_ref("   ");
        assert_eq!(build_pr_title("My feature", &meta), "My feature");
    }
}
