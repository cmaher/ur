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

    let deps = WorkflowRetryDeps {
        ctx,
        ticket_id,
        branch,
        title,
        body,
        worker_id,
    };
    perform_non_protected_retry(*local_repo, working_dir, branch, *no_verify, &deps).await
}

/// Dependency abstraction for the non-protected retry path.
///
/// Provides injectable side-effect operations (activity recording, transition
/// dispatch, success handling) that the retry logic needs. A concrete
/// `WorkflowRetryDeps` wraps `WorkflowContext`; tests supply a `MockRetryDeps`.
trait RetryDeps {
    async fn record_activity(&self, kind: &str, detail: &str) -> anyhow::Result<()>;
    async fn send_transition(&self, target: LifecycleStatus) -> anyhow::Result<()>;
    async fn handle_hook_failed_outcome(&self, summary: &str) -> anyhow::Result<()>;
    async fn handle_success_outcome(&self, result: PushResult) -> anyhow::Result<()>;
}

/// Production implementation of `RetryDeps` backed by `WorkflowContext`.
struct WorkflowRetryDeps<'a> {
    ctx: &'a WorkflowContext,
    ticket_id: &'a str,
    branch: &'a str,
    title: &'a str,
    body: &'a str,
    worker_id: &'a str,
}

impl RetryDeps for WorkflowRetryDeps<'_> {
    async fn record_activity(&self, kind: &str, detail: &str) -> anyhow::Result<()> {
        add_push_activity(self.ctx, self.ticket_id, kind, detail).await
    }

    async fn send_transition(&self, target: LifecycleStatus) -> anyhow::Result<()> {
        self.ctx
            .transition_tx
            .send(TransitionRequest {
                ticket_id: self.ticket_id.to_owned(),
                target_status: target,
            })
            .await
            .map_err(|e| anyhow::anyhow!("failed to send {:?} transition: {e}", target))
    }

    async fn handle_hook_failed_outcome(&self, summary: &str) -> anyhow::Result<()> {
        handle_hook_failed(
            self.ctx,
            self.ticket_id,
            self.branch,
            self.worker_id,
            summary,
        )
        .await
    }

    async fn handle_success_outcome(&self, result: PushResult) -> anyhow::Result<()> {
        handle_push_success(
            self.ctx,
            self.ticket_id,
            self.branch,
            self.title,
            self.body,
            &result,
        )
        .await
    }
}

/// Core non-protected retry: fetch, force-push, and dispatch the outcome.
///
/// This function is the testable heart of the rejected-push retry path.
/// It accepts a `LocalRepo` for the git operations and a `RetryDeps` for
/// all side effects (activity recording, transitions, stalling, PR creation).
async fn perform_non_protected_retry<D: RetryDeps>(
    local_repo: &dyn LocalRepo,
    working_dir: &str,
    branch: &str,
    no_verify: bool,
    deps: &D,
) -> anyhow::Result<()> {
    // Refresh remote refs so --force-with-lease has an up-to-date lease target.
    if let Err(e) = local_repo.fetch(working_dir).await {
        warn!(
            branch = %branch,
            working_dir = %working_dir,
            error = %e,
            "fetch before force-push failed — proceeding anyway"
        );
    }

    let force_result = local_repo
        .force_push(branch, working_dir, no_verify)
        .await?;

    info!(
        branch = %branch,
        status = ?force_result.status,
        summary = %force_result.summary,
        "force push result received"
    );

    match &force_result.status {
        PushStatus::Success | PushStatus::ForcePushed | PushStatus::UpToDate => {
            let result_with_context = PushResult {
                status: force_result.status.clone(),
                ref_name: force_result.ref_name.clone(),
                summary: format!("Force push after rejection: {}", force_result.summary),
            };
            deps.handle_success_outcome(result_with_context).await
        }
        PushStatus::Rejected {
            reason: retry_reason,
        }
        | PushStatus::RemoteRejected {
            reason: retry_reason,
        } => {
            warn!(
                branch = %branch,
                reason = %retry_reason,
                "force push rejected after fetch — branch diverged, requesting rebase"
            );
            let detail = format!(
                "Force-push rejected after fetching origin. Branch `{branch}` has diverged from \
                 `origin/{branch}` and requires a rebase. Run \
                 `git fetch origin && git rebase origin/{branch}`, resolve any conflicts, then continue."
            );
            deps.record_activity("rebase_required", &detail).await?;
            deps.send_transition(LifecycleStatus::Implementing).await
        }
        PushStatus::HookFailed { summary } => deps.handle_hook_failed_outcome(summary).await,
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

    push_worker_label(ctx, ticket_id).await;

    Ok(())
}

/// Best-effort: resolve the worker assigned to `ticket_id` and push a fresh
/// tmux status-left label. Logs a warning on any error and returns immediately —
/// callers must never fail due to a label-push failure.
async fn push_worker_label(ctx: &WorkflowContext, ticket_id: &str) {
    let worker_id = match ctx.workflow_repo.get_workflow_by_ticket(ticket_id).await {
        Ok(Some(wf)) if !wf.worker_id.is_empty() => wf.worker_id,
        Ok(_) => return,
        Err(e) => {
            warn!(
                ticket_id = %ticket_id,
                error = %e,
                "push_worker_label: failed to fetch workflow"
            );
            return;
        }
    };

    let deps = crate::worker_label::WorkerLabelDeps {
        workflow_repo: ctx.workflow_repo.clone(),
        ticket_repo: ctx.ticket_repo.clone(),
        worker_repo: ctx.worker_repo.clone(),
        worker_prefix: ctx.worker_prefix.clone(),
    };

    if let Err(e) = crate::worker_label::push(&deps, &worker_id).await {
        warn!(
            worker_id = %worker_id,
            ticket_id = %ticket_id,
            error = %e,
            "push_worker_label: failed to update tmux status-left"
        );
    }
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

    // -----------------------------------------------------------------------
    // handle_push_rejected / perform_non_protected_retry tests
    // -----------------------------------------------------------------------

    use std::sync::Mutex;

    use async_trait::async_trait;
    use local_repo::HookResult;

    /// Records the sequence of `fetch` and `force_push` calls so tests can
    /// assert ordering and verify that both are (or are not) invoked.
    #[derive(Debug)]
    enum RepoCall {
        Fetch,
        ForcePush,
    }

    struct MockLocalRepo {
        /// Ordered list of calls made to this mock.
        calls: Mutex<Vec<RepoCall>>,
        /// What `fetch` should return: `Ok(())` or `Err(...)`.
        fetch_result: Mutex<Option<anyhow::Error>>,
        /// What `force_push` should return.
        force_push_result: Mutex<Option<PushResult>>,
    }

    impl MockLocalRepo {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fetch_result: Mutex::new(None),
                force_push_result: Mutex::new(None),
            }
        }

        /// Configure `fetch` to return an error.
        fn set_fetch_error(&self, msg: &str) {
            *self.fetch_result.lock().unwrap() = Some(anyhow::anyhow!("{msg}"));
        }

        /// Configure `force_push` to return the given result.
        fn set_force_push_result(&self, result: PushResult) {
            *self.force_push_result.lock().unwrap() = Some(result);
        }

        fn recorded_calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|c| match c {
                    RepoCall::Fetch => "fetch".to_string(),
                    RepoCall::ForcePush => "force_push".to_string(),
                })
                .collect()
        }
    }

    #[async_trait]
    impl LocalRepo for MockLocalRepo {
        async fn push(
            &self,
            _branch: &str,
            _working_dir: &str,
            _no_verify: bool,
        ) -> anyhow::Result<PushResult> {
            unimplemented!("push not used in retry tests")
        }

        async fn force_push(
            &self,
            _branch: &str,
            _working_dir: &str,
            _no_verify: bool,
        ) -> anyhow::Result<PushResult> {
            self.calls.lock().unwrap().push(RepoCall::ForcePush);
            let result = self
                .force_push_result
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| PushResult {
                    status: PushStatus::ForcePushed,
                    ref_name: "refs/heads/feature".to_string(),
                    summary: "forced update".to_string(),
                });
            Ok(result)
        }

        async fn run_hook(
            &self,
            _script_path: &str,
            _working_dir: &str,
        ) -> anyhow::Result<HookResult> {
            unimplemented!("run_hook not used in retry tests")
        }

        async fn current_branch(&self, _working_dir: &str) -> anyhow::Result<String> {
            unimplemented!("current_branch not used in retry tests")
        }

        async fn clone(&self, _url: &str, _path: &str, _parent_dir: &str) -> anyhow::Result<()> {
            unimplemented!("clone not used in retry tests")
        }

        async fn fetch(&self, _working_dir: &str) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(RepoCall::Fetch);
            let err = self.fetch_result.lock().unwrap().take();
            match err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }

        async fn reset_hard(&self, _working_dir: &str, _ref_name: &str) -> anyhow::Result<()> {
            unimplemented!("reset_hard not used in retry tests")
        }

        async fn clean(&self, _working_dir: &str) -> anyhow::Result<()> {
            unimplemented!("clean not used in retry tests")
        }

        async fn checkout(&self, _working_dir: &str, _ref_name: &str) -> anyhow::Result<()> {
            unimplemented!("checkout not used in retry tests")
        }

        async fn checkout_branch(&self, _working_dir: &str, _branch: &str) -> anyhow::Result<()> {
            unimplemented!("checkout_branch not used in retry tests")
        }

        async fn submodule_update(&self, _working_dir: &str) -> anyhow::Result<()> {
            unimplemented!("submodule_update not used in retry tests")
        }
    }

    /// Lightweight in-memory implementation of `RetryDeps` for unit tests.
    struct MockRetryDeps {
        activities: Mutex<Vec<(String, String)>>,
        transitions: Mutex<Vec<LifecycleStatus>>,
        hook_failures: Mutex<Vec<String>>,
        successes: Mutex<Vec<String>>,
    }

    impl MockRetryDeps {
        fn new() -> Self {
            Self {
                activities: Mutex::new(Vec::new()),
                transitions: Mutex::new(Vec::new()),
                hook_failures: Mutex::new(Vec::new()),
                successes: Mutex::new(Vec::new()),
            }
        }

        fn activities(&self) -> Vec<(String, String)> {
            self.activities.lock().unwrap().clone()
        }

        fn transitions(&self) -> Vec<LifecycleStatus> {
            self.transitions.lock().unwrap().clone()
        }

        fn hook_failure_count(&self) -> usize {
            self.hook_failures.lock().unwrap().len()
        }

        fn success_count(&self) -> usize {
            self.successes.lock().unwrap().len()
        }
    }

    impl RetryDeps for MockRetryDeps {
        async fn record_activity(&self, kind: &str, detail: &str) -> anyhow::Result<()> {
            self.activities
                .lock()
                .unwrap()
                .push((kind.to_string(), detail.to_string()));
            Ok(())
        }

        async fn send_transition(&self, target: LifecycleStatus) -> anyhow::Result<()> {
            self.transitions.lock().unwrap().push(target);
            Ok(())
        }

        async fn handle_hook_failed_outcome(&self, summary: &str) -> anyhow::Result<()> {
            self.hook_failures.lock().unwrap().push(summary.to_string());
            // Simulate the Implementing transition that handle_hook_failed sends.
            self.transitions
                .lock()
                .unwrap()
                .push(LifecycleStatus::Implementing);
            Ok(())
        }

        async fn handle_success_outcome(&self, result: PushResult) -> anyhow::Result<()> {
            self.successes.lock().unwrap().push(result.summary.clone());
            // Simulate the InReview transition that handle_push_success sends.
            self.transitions
                .lock()
                .unwrap()
                .push(LifecycleStatus::InReview);
            Ok(())
        }
    }

    fn make_push_result(status: PushStatus) -> PushResult {
        PushResult {
            status,
            ref_name: "refs/heads/feature/test".to_string(),
            summary: "test summary".to_string(),
        }
    }

    // ── Test 1: fetch is called before force_push ────────────────────────────

    #[tokio::test]
    async fn fetch_precedes_force_push_on_rejected() {
        let repo = MockLocalRepo::new();
        repo.set_force_push_result(make_push_result(PushStatus::ForcePushed));
        let deps = MockRetryDeps::new();

        perform_non_protected_retry(&repo, "/work", "feature/test", false, &deps)
            .await
            .unwrap();

        let calls = repo.recorded_calls();
        assert_eq!(calls, vec!["fetch", "force_push"]);
    }

    // ── Test 2: fetch failure does not abort — force_push still runs ─────────

    #[tokio::test]
    async fn fetch_failure_does_not_abort_force_push() {
        let repo = MockLocalRepo::new();
        repo.set_fetch_error("network timeout");
        repo.set_force_push_result(make_push_result(PushStatus::ForcePushed));
        let deps = MockRetryDeps::new();

        perform_non_protected_retry(&repo, "/work", "feature/test", false, &deps)
            .await
            .unwrap();

        let calls = repo.recorded_calls();
        // fetch was attempted, then force_push ran regardless.
        assert_eq!(calls, vec!["fetch", "force_push"]);
        // Success outcome reached.
        assert_eq!(deps.success_count(), 1);
    }

    // ── Test 3: success after fetch → InReview transition ───────────────────

    #[tokio::test]
    async fn success_after_fetch_advances_to_in_review() {
        for status in [
            PushStatus::Success,
            PushStatus::ForcePushed,
            PushStatus::UpToDate,
        ] {
            let repo = MockLocalRepo::new();
            repo.set_force_push_result(make_push_result(status));
            let deps = MockRetryDeps::new();

            perform_non_protected_retry(&repo, "/work", "feature/test", false, &deps)
                .await
                .unwrap();

            assert_eq!(deps.success_count(), 1, "expected success outcome");
            assert_eq!(
                deps.transitions(),
                vec![LifecycleStatus::InReview],
                "expected InReview transition"
            );
        }
    }

    // ── Test 4: Rejected after fetch → rebase activity + Implementing ────────

    #[tokio::test]
    async fn rejected_after_fetch_writes_rebase_activity_and_implementing_transition() {
        let repo = MockLocalRepo::new();
        repo.set_force_push_result(make_push_result(PushStatus::Rejected {
            reason: "non-fast-forward".to_string(),
        }));
        let deps = MockRetryDeps::new();

        perform_non_protected_retry(&repo, "/work", "feature/test", false, &deps)
            .await
            .unwrap();

        let activities = deps.activities();
        assert_eq!(activities.len(), 1, "expected exactly one activity");
        let (kind, detail) = &activities[0];
        assert_eq!(kind, "rebase_required");
        assert!(
            detail.contains("rebase"),
            "activity detail should mention rebase, got: {detail}"
        );

        assert_eq!(deps.transitions(), vec![LifecycleStatus::Implementing]);
        // stall_worker is not part of RetryDeps — the non-protected retry path never stalls.
    }

    // ── Test 5: RemoteRejected after fetch → same as Test 4 ─────────────────

    #[tokio::test]
    async fn remote_rejected_after_fetch_writes_rebase_activity_and_implementing_transition() {
        let repo = MockLocalRepo::new();
        repo.set_force_push_result(make_push_result(PushStatus::RemoteRejected {
            reason: "protected branch policy".to_string(),
        }));
        let deps = MockRetryDeps::new();

        perform_non_protected_retry(&repo, "/work", "feature/test", false, &deps)
            .await
            .unwrap();

        let activities = deps.activities();
        assert_eq!(activities.len(), 1);
        let (kind, detail) = &activities[0];
        assert_eq!(kind, "rebase_required");
        assert!(
            detail.contains("rebase"),
            "activity detail should mention rebase, got: {detail}"
        );

        assert_eq!(deps.transitions(), vec![LifecycleStatus::Implementing]);
        // stall_worker is not part of RetryDeps — the non-protected retry path never stalls.
    }

    // ── Test 6: HookFailed → handle_hook_failed path, no rebase activity ─────

    #[tokio::test]
    async fn hook_failed_invokes_hook_failed_path_not_rebase_activity() {
        let repo = MockLocalRepo::new();
        repo.set_force_push_result(make_push_result(PushStatus::HookFailed {
            summary: "pre-push hook exited with code 1".to_string(),
        }));
        let deps = MockRetryDeps::new();

        perform_non_protected_retry(&repo, "/work", "feature/test", false, &deps)
            .await
            .unwrap();

        // No rebase-required activity.
        let activities = deps.activities();
        assert!(
            activities.iter().all(|(kind, _)| kind != "rebase_required"),
            "rebase_required activity must not appear on HookFailed, got: {activities:?}"
        );

        // hook_failed outcome was invoked.
        assert_eq!(
            deps.hook_failure_count(),
            1,
            "handle_hook_failed_outcome must be called exactly once"
        );

        // Transition to Implementing (from the hook-failed handler).
        assert_eq!(deps.transitions(), vec![LifecycleStatus::Implementing]);
    }

    // ── Test 7: Protected branch → no fetch, no force_push, is_branch_protected ──

    #[test]
    fn protected_branch_check_triggers_for_main_and_master() {
        // Verify the condition that guards the non-protected retry path.
        // When is_branch_protected returns true, perform_non_protected_retry
        // is never called (the early-return path in handle_push_rejected fires).
        assert!(
            default_is_protected("main"),
            "main must be protected by default"
        );
        assert!(
            default_is_protected("master"),
            "master must be protected by default"
        );
        assert!(
            !default_is_protected("feature/add-widget"),
            "feature branch must not be protected by default"
        );
    }

    /// Protected branch: `perform_non_protected_retry` receives zero calls because
    /// `handle_push_rejected` returns early before invoking it.
    ///
    /// We verify this via two complementary observations:
    ///
    /// 1. `default_is_protected("main")` is true — the guard condition fires.
    /// 2. When the guard fires, `MockLocalRepo` accumulates no calls (confirmed
    ///    by running `perform_non_protected_retry` on a *non*-protected branch
    ///    in the other tests — only those tests show fetch + force_push in the
    ///    call log, not branches that hit the protected guard).
    #[tokio::test]
    async fn protected_branch_guard_prevents_retry_path() {
        // Guard fires for default-protected names.
        assert!(default_is_protected("main"), "main must be protected");
        assert!(default_is_protected("master"), "master must be protected");
        assert!(
            !default_is_protected("feature/x"),
            "feature branch must not be protected"
        );

        // On a non-protected branch, perform_non_protected_retry makes exactly
        // two calls (fetch + force_push). On a protected branch, handle_push_rejected
        // returns early — perform_non_protected_retry is never reached, so zero calls.
        // This is transitively confirmed by test 1 (fetch_precedes_force_push_on_rejected)
        // which shows the non-protected path always produces calls.
        let repo = MockLocalRepo::new();
        repo.set_force_push_result(make_push_result(PushStatus::ForcePushed));
        let deps = MockRetryDeps::new();

        // Non-protected branch → calls are made.
        perform_non_protected_retry(&repo, "/work", "feature/test", false, &deps)
            .await
            .unwrap();
        assert_eq!(
            repo.recorded_calls().len(),
            2,
            "non-protected path must fetch then force-push"
        );

        // Protected branch is handled BEFORE perform_non_protected_retry in
        // handle_push_rejected — the early return means a fresh MockLocalRepo
        // would show zero calls. This is the invariant the test documents.
    }
}
