use std::path::PathBuf;

use local_repo::LocalRepo;
use tracing::{info, warn};

use ur_config::{ResolvedTemplatePath, resolve_template_path};
use ur_db::model::LifecycleStatus;

use crate::workflow::{HandlerFuture, WorkflowContext, WorkflowHandler};

/// Handler for the Implementing -> Verifying transition.
///
/// Runs the project's `pre-push` workflow hook (if configured) to verify
/// that the worker's changes pass local checks before pushing.
///
/// Hook resolution:
/// 1. Read the ticket's project to find the `ProjectConfig`.
/// 2. Read `workflow_hooks_dir` from the project config.
/// 3. Resolve the template path and locate `pre-push` inside it.
/// 4. Execute via `local_repo.run_hook()` through builderd.
///
/// Outcomes:
/// - Hook not configured or not found: skip verification, transition to Pushing.
/// - Hook passes (exit 0): transition to Pushing.
/// - Hook fails: increment `fix_attempt_count` meta. If under
///   `max_fix_attempts`, transition to Implementing. If over, set `agent_status`
///   to `stalled`.
pub struct VerifyHandler;

impl WorkflowHandler for VerifyHandler {
    fn handle(&self, ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move { run_verification(&ctx, &ticket_id).await })
    }
}

/// Core verification logic extracted from the handler.
async fn run_verification(ctx: &WorkflowContext, ticket_id: &str) -> anyhow::Result<()> {
    // 1. Load ticket to get project key.
    let ticket = ctx
        .ticket_repo
        .get_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

    let project_key = &ticket.project;

    // 2. Look up project config.
    let project_config = match ctx.config.projects.get(project_key) {
        Some(pc) => pc,
        None => {
            info!(
                ticket_id = %ticket_id,
                project_key = %project_key,
                "no project config found — skipping verification, advancing to pushing"
            );
            advance_to_pushing(ctx, ticket_id).await?;
            return Ok(());
        }
    };

    // 3. Resolve the hook path and working directory.
    let (hook_path_str, working_dir) =
        match resolve_hook_context(ctx, ticket_id, project_key, project_config).await? {
            Some(resolved) => resolved,
            None => {
                // No hook configured — already advanced to pushing inside resolve_hook_context.
                return Ok(());
            }
        };

    info!(
        ticket_id = %ticket_id,
        project_key = %project_key,
        hook_path = %hook_path_str,
        working_dir = %working_dir,
        "running pre-push verification hook"
    );

    // 4. Execute the hook via local_repo (through builderd).
    let local_repo = local_repo::GitBackend {
        client: ctx.builderd_client.clone(),
    };
    let hook_result = match local_repo.run_hook(&hook_path_str, &working_dir).await {
        Ok(result) => result,
        Err(e) => {
            warn!(
                ticket_id = %ticket_id,
                error = %e,
                hook_path = %hook_path_str,
                "hook execution failed (possibly not found) — skipping verification, advancing to pushing"
            );
            add_hook_activity(
                ctx,
                ticket_id,
                "pass",
                "hook not found or not executable, skipping verification",
            )
            .await?;
            advance_to_pushing(ctx, ticket_id).await?;
            return Ok(());
        }
    };

    // 5. Process hook result.
    let output_summary = build_output_summary(&hook_result.stdout, &hook_result.stderr);
    let meta = ctx.ticket_repo.get_meta(ticket_id, "ticket").await?;
    let worker_id = meta.get("worker_id").ok_or_else(|| {
        anyhow::anyhow!("no worker_id metadata on ticket {ticket_id} — cannot run verification")
    })?;

    if hook_result.success() {
        info!(ticket_id = %ticket_id, "pre-push hook passed — advancing to pushing");
        add_hook_activity(ctx, ticket_id, "pass", &output_summary).await?;
        advance_to_pushing(ctx, ticket_id).await?;
    } else {
        info!(ticket_id = %ticket_id, exit_code = hook_result.exit_code, "pre-push hook failed");
        add_hook_activity(ctx, ticket_id, "fail", &output_summary).await?;
        handle_hook_failure(ctx, ticket_id, worker_id, project_config.max_fix_attempts).await?;
    }

    Ok(())
}

/// Resolve the hook script path and working directory for verification.
///
/// Returns `None` if no hook is configured (and advances the ticket to Pushing).
/// Returns `Some((hook_path, working_dir))` if a hook was resolved.
async fn resolve_hook_context(
    ctx: &WorkflowContext,
    ticket_id: &str,
    project_key: &str,
    project_config: &ur_config::ProjectConfig,
) -> anyhow::Result<Option<(String, String)>> {
    // Read workflow_hooks_dir from project config.
    let hooks_dir_template = match &project_config.workflow_hooks_dir {
        Some(dir) => dir,
        None => {
            info!(
                ticket_id = %ticket_id,
                project_key = %project_key,
                "no workflow_hooks_dir configured — skipping verification, advancing to pushing"
            );
            advance_to_pushing(ctx, ticket_id).await?;
            return Ok(None);
        }
    };

    // Resolve the template path.
    let resolved = resolve_template_path(hooks_dir_template, &ctx.config.config_dir)?;

    // Resolve worker and slot to get the working directory.
    let meta = ctx.ticket_repo.get_meta(ticket_id, "ticket").await?;
    let worker_id = meta.get("worker_id").ok_or_else(|| {
        anyhow::anyhow!("no worker_id metadata on ticket {ticket_id} — cannot run verification")
    })?;

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

    // Compute the hook script path based on the resolved template.
    let hook_script_path = match resolved {
        ResolvedTemplatePath::ProjectRelative(rel_path) => {
            PathBuf::from(working_dir).join(rel_path).join("pre-push")
        }
        ResolvedTemplatePath::HostPath(abs_path) => abs_path.join("pre-push"),
    };

    let hook_path_str = hook_script_path.to_string_lossy().to_string();
    Ok(Some((hook_path_str, working_dir.clone())))
}

/// Handle a failed hook: increment fix attempts, then either transition to
/// Implementing or stall the agent.
async fn handle_hook_failure(
    ctx: &WorkflowContext,
    ticket_id: &str,
    worker_id: &str,
    max_fix_attempts: u32,
) -> anyhow::Result<()> {
    let fix_attempt_count = increment_fix_attempts(ctx, ticket_id).await?;

    if fix_attempt_count >= max_fix_attempts {
        warn!(
            ticket_id = %ticket_id,
            fix_attempt_count = fix_attempt_count,
            max_fix_attempts = max_fix_attempts,
            "fix attempt limit reached — setting agent_status to stalled"
        );
        set_agent_stalled(ctx, ticket_id, worker_id).await?;
    } else {
        info!(
            ticket_id = %ticket_id,
            fix_attempt_count = fix_attempt_count,
            max_fix_attempts = max_fix_attempts,
            "under fix limit — transitioning to implementing"
        );
        advance_to_implementing(ctx, ticket_id).await?;
    }
    Ok(())
}

/// Transition the ticket's lifecycle status to Pushing.
async fn advance_to_pushing(ctx: &WorkflowContext, ticket_id: &str) -> anyhow::Result<()> {
    let update = ur_db::model::TicketUpdate {
        lifecycle_status: Some(LifecycleStatus::Pushing),
        status: None,
        lifecycle_managed: None,
        type_: None,
        priority: None,
        title: None,
        body: None,
        branch: None,
        parent_id: None,
        project: None,
    };
    ctx.ticket_repo.update_ticket(ticket_id, &update).await?;
    Ok(())
}

/// Transition the ticket's lifecycle status to Implementing.
async fn advance_to_implementing(ctx: &WorkflowContext, ticket_id: &str) -> anyhow::Result<()> {
    let update = ur_db::model::TicketUpdate {
        lifecycle_status: Some(LifecycleStatus::Implementing),
        status: None,
        lifecycle_managed: None,
        type_: None,
        priority: None,
        title: None,
        body: None,
        branch: None,
        parent_id: None,
        project: None,
    };
    ctx.ticket_repo.update_ticket(ticket_id, &update).await?;
    Ok(())
}

/// Add an activity to the ticket with hook metadata.
async fn add_hook_activity(
    ctx: &WorkflowContext,
    ticket_id: &str,
    result: &str,
    output: &str,
) -> anyhow::Result<()> {
    let message = format!(
        "[workflow] pre-push hook {result}\n\
         source: workflow\n\
         hook: pre-push\n\
         result: {result}\n\
         ---\n\
         {output}"
    );
    ctx.ticket_repo
        .add_activity(ticket_id, "workflow", &message)
        .await?;
    Ok(())
}

/// Increment the `fix_attempt_count` metadata on the ticket and return the new value.
async fn increment_fix_attempts(ctx: &WorkflowContext, ticket_id: &str) -> anyhow::Result<u32> {
    let meta = ctx.ticket_repo.get_meta(ticket_id, "ticket").await?;
    let current: u32 = meta
        .get("fix_attempt_count")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let new_count = current + 1;
    ctx.ticket_repo
        .set_meta(
            ticket_id,
            "ticket",
            "fix_attempt_count",
            &new_count.to_string(),
        )
        .await?;
    Ok(new_count)
}

/// Set the worker's agent_status to "stalled".
async fn set_agent_stalled(
    ctx: &WorkflowContext,
    ticket_id: &str,
    worker_id: &str,
) -> anyhow::Result<()> {
    ctx.worker_repo
        .update_worker_agent_status(worker_id, ur_db::model::AgentStatus::Stalled)
        .await
        .map_err(|e| anyhow::anyhow!("failed to set agent_status to stalled for worker {worker_id} (ticket {ticket_id}): {e}"))?;
    Ok(())
}

/// Build a truncated output summary from stdout and stderr.
fn build_output_summary(stdout: &str, stderr: &str) -> String {
    let mut parts = Vec::new();
    if !stdout.is_empty() {
        parts.push(format!("stdout:\n{stdout}"));
    }
    if !stderr.is_empty() {
        parts.push(format!("stderr:\n{stderr}"));
    }
    if parts.is_empty() {
        "(no output)".to_string()
    } else {
        let combined = parts.join("\n");
        // Truncate to a reasonable size for activity logs.
        if combined.len() > 4000 {
            format!("{}...(truncated)", &combined[..4000])
        } else {
            combined
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_output_summary_both() {
        let s = build_output_summary("hello", "world");
        assert!(s.contains("stdout:\nhello"));
        assert!(s.contains("stderr:\nworld"));
    }

    #[test]
    fn build_output_summary_empty() {
        let s = build_output_summary("", "");
        assert_eq!(s, "(no output)");
    }

    #[test]
    fn build_output_summary_truncates() {
        let long = "x".repeat(5000);
        let s = build_output_summary(&long, "");
        assert!(s.len() < 5000);
        assert!(s.ends_with("...(truncated)"));
    }
}
