use std::path::PathBuf;

use local_repo::LocalRepo;
use tracing::{info, warn};

use ticket_db::LifecycleStatus;

use crate::workflow::handlers::hook_log::write_hook_failure_log;
use crate::workflow::{HandlerFuture, TransitionRequest, WorkflowContext, WorkflowHandler};

/// Handler for the Implementing -> Verifying transition.
///
/// Runs the project's `pre-push` workflow hook (if configured) to verify
/// that the worker's changes pass local checks before pushing.
///
/// Hook resolution (two-layer overlay):
/// 1. Check host overlay: `<config_dir>/projects/<project_key>/hooks/workflow/pre-push`
/// 2. Check in-repo: `<slot.host_path>/ur-hooks/workflow/pre-push`
/// 3. Neither found: skip verification, transition to Pushing.
///
/// Outcomes:
/// - Hook not found: skip verification, transition to Pushing.
/// - Hook passes (exit 0): transition to Pushing.
/// - Hook fails: transition to Implementing for another attempt. The implement
///   cycle limit is enforced in `dispatch_implement`, not here.
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
        match resolve_hook_context(ctx, ticket_id, project_key).await? {
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
    let workflow = ctx
        .workflow_repo
        .get_workflow_by_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no workflow found for ticket {ticket_id}"))?;
    if workflow.worker_id.is_empty() {
        anyhow::bail!("no worker_id on workflow for ticket {ticket_id} — cannot run verification");
    }
    let worker_id = &workflow.worker_id;

    let push_again_exit_code = project_config.push_again_exit_code;

    match classify_hook_outcome(hook_result.exit_code, push_again_exit_code) {
        HookOutcome::Pass => {
            info!(ticket_id = %ticket_id, "pre-push hook passed — advancing to pushing");
            add_hook_activity(ctx, ticket_id, "pass", &output_summary).await?;
            advance_to_pushing(ctx, ticket_id).await?;
        }
        HookOutcome::PushAgain => {
            info!(
                ticket_id = %ticket_id,
                exit_code = hook_result.exit_code,
                "pre-push hook returned push_again — advancing to pushing without penalizing implement cycles"
            );
            add_hook_activity(ctx, ticket_id, "push_again", &output_summary).await?;
            advance_to_pushing(ctx, ticket_id).await?;
        }
        HookOutcome::Fail => {
            info!(ticket_id = %ticket_id, exit_code = hook_result.exit_code, "pre-push hook failed");
            let log_path = write_hook_failure_log(
                &ctx.config.logs_dir,
                worker_id,
                "verify",
                &hook_result.stdout,
                &hook_result.stderr,
                hook_result.exit_code,
            );
            add_hook_failure_activity(ctx, ticket_id, &log_path, &output_summary).await?;
            handle_hook_failure(ctx, ticket_id, worker_id).await?;
        }
    }

    Ok(())
}

/// Resolve the hook script path and working directory for verification.
///
/// Uses a two-layer overlay convention:
/// 1. Host overlay: `<config_dir>/projects/<project_key>/hooks/workflow/pre-push`
/// 2. In-repo: `<slot.host_path>/ur-hooks/workflow/pre-push`
///
/// Returns `None` if neither path exists (and advances the ticket to Pushing).
/// Returns `Some((hook_path, working_dir))` if a hook was resolved.
async fn resolve_hook_context(
    ctx: &WorkflowContext,
    ticket_id: &str,
    project_key: &str,
) -> anyhow::Result<Option<(String, String)>> {
    // Resolve worker and slot to get the working directory.
    let workflow = ctx
        .workflow_repo
        .get_workflow_by_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no workflow found for ticket {ticket_id}"))?;
    if workflow.worker_id.is_empty() {
        anyhow::bail!("no worker_id on workflow for ticket {ticket_id} — cannot run verification");
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

    // Resolve using two-layer overlay: host overlay → in-repo → None.
    match find_workflow_hook_path(&ctx.config.config_dir, project_key, working_dir) {
        Some(hook_path) => {
            let hook_path_str = hook_path.to_string_lossy().into_owned();
            Ok(Some((hook_path_str, working_dir.clone())))
        }
        None => {
            info!(
                ticket_id = %ticket_id,
                project_key = %project_key,
                "no workflow hooks configured — skipping verification, advancing to pushing"
            );
            advance_to_pushing(ctx, ticket_id).await?;
            Ok(None)
        }
    }
}

/// Resolve the workflow hook path using the two-layer overlay convention.
///
/// This is the pure filesystem portion of the resolution, extracted for testing.
///
/// Priority:
/// 1. `<config_dir>/projects/<project_key>/hooks/workflow/pre-push` (host overlay)
/// 2. `<slot_host_path>/ur-hooks/workflow/pre-push` (in-repo)
/// 3. Neither → `None`
fn find_workflow_hook_path(
    config_dir: &std::path::Path,
    project_key: &str,
    slot_host_path: &str,
) -> Option<PathBuf> {
    let host_overlay = config_dir
        .join("projects")
        .join(project_key)
        .join("hooks")
        .join("workflow")
        .join("pre-push");

    if host_overlay.exists() {
        return Some(host_overlay);
    }

    let in_repo = PathBuf::from(slot_host_path)
        .join("ur-hooks")
        .join("workflow")
        .join("pre-push");

    if in_repo.exists() {
        return Some(in_repo);
    }

    None
}

/// Handle a failed hook: transition back to Implementing for another attempt.
///
/// The implement cycle limit is enforced in `dispatch_implement`, so this
/// handler always re-dispatches. The cycle count is incremented when
/// Implementing is entered, not here.
async fn handle_hook_failure(
    ctx: &WorkflowContext,
    ticket_id: &str,
    _worker_id: &str,
) -> anyhow::Result<()> {
    info!(
        ticket_id = %ticket_id,
        "hook failed — transitioning to implementing"
    );
    advance_to_implementing(ctx, ticket_id).await?;
    Ok(())
}

/// Send a transition request to Pushing via the coordinator channel.
async fn advance_to_pushing(ctx: &WorkflowContext, ticket_id: &str) -> anyhow::Result<()> {
    ctx.transition_tx
        .send(TransitionRequest {
            ticket_id: ticket_id.to_owned(),
            target_status: LifecycleStatus::Pushing,
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to send Pushing transition: {e}"))?;
    Ok(())
}

/// Send a transition request to Implementing via the coordinator channel.
async fn advance_to_implementing(ctx: &WorkflowContext, ticket_id: &str) -> anyhow::Result<()> {
    ctx.transition_tx
        .send(TransitionRequest {
            ticket_id: ticket_id.to_owned(),
            target_status: LifecycleStatus::Implementing,
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to send Implementing transition: {e}"))?;
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

/// Add an activity for a hook failure, including the log file path.
async fn add_hook_failure_activity(
    ctx: &WorkflowContext,
    ticket_id: &str,
    log_path: &str,
    output: &str,
) -> anyhow::Result<()> {
    let message = format!(
        "[workflow] verify failed — logs at {log_path}\n\
         source: workflow\n\
         hook: pre-push\n\
         result: fail\n\
         ---\n\
         {output}"
    );
    ctx.ticket_repo
        .add_activity(ticket_id, "workflow", &message)
        .await?;
    Ok(())
}

/// Classify the outcome of a hook execution based on its exit code.
///
/// Used to determine which branch of the verify handler to take.
#[derive(Debug, PartialEq, Eq)]
enum HookOutcome {
    /// Exit code 0 — hook passed.
    Pass,
    /// Exit code matches `push_again_exit_code` — push again without penalizing cycles.
    PushAgain,
    /// Any other non-zero exit code — hook failed.
    Fail,
}

fn classify_hook_outcome(exit_code: i32, push_again_exit_code: i32) -> HookOutcome {
    if exit_code == 0 {
        HookOutcome::Pass
    } else if exit_code == push_again_exit_code {
        HookOutcome::PushAgain
    } else {
        HookOutcome::Fail
    }
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
    fn classify_exit_0_is_pass() {
        assert_eq!(classify_hook_outcome(0, 200), HookOutcome::Pass);
    }

    #[test]
    fn classify_push_again_code_is_push_again() {
        assert_eq!(classify_hook_outcome(200, 200), HookOutcome::PushAgain);
    }

    #[test]
    fn classify_nonzero_non_push_again_is_fail() {
        assert_eq!(classify_hook_outcome(1, 200), HookOutcome::Fail);
        assert_eq!(classify_hook_outcome(127, 200), HookOutcome::Fail);
        assert_eq!(classify_hook_outcome(201, 200), HookOutcome::Fail);
    }

    #[test]
    fn classify_custom_push_again_code() {
        assert_eq!(classify_hook_outcome(42, 42), HookOutcome::PushAgain);
        assert_eq!(classify_hook_outcome(200, 42), HookOutcome::Fail);
        assert_eq!(classify_hook_outcome(0, 42), HookOutcome::Pass);
    }

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

    // --- Hook overlay resolution tests ---

    fn create_file(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    }

    #[test]
    fn hook_resolution_overlay_only() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let slot_dir = tmp.path().join("slot");

        let overlay = config_dir
            .join("projects")
            .join("myproject")
            .join("hooks")
            .join("workflow")
            .join("pre-push");
        create_file(&overlay);

        let result = find_workflow_hook_path(&config_dir, "myproject", slot_dir.to_str().unwrap());
        assert_eq!(result, Some(overlay));
    }

    #[test]
    fn hook_resolution_in_repo_only() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let slot_dir = tmp.path().join("slot");

        let in_repo = slot_dir.join("ur-hooks").join("workflow").join("pre-push");
        create_file(&in_repo);

        let result = find_workflow_hook_path(&config_dir, "myproject", slot_dir.to_str().unwrap());
        assert_eq!(result, Some(in_repo));
    }

    #[test]
    fn hook_resolution_overlay_wins_when_both_present() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let slot_dir = tmp.path().join("slot");

        let overlay = config_dir
            .join("projects")
            .join("myproject")
            .join("hooks")
            .join("workflow")
            .join("pre-push");
        create_file(&overlay);

        let in_repo = slot_dir.join("ur-hooks").join("workflow").join("pre-push");
        create_file(&in_repo);

        let result = find_workflow_hook_path(&config_dir, "myproject", slot_dir.to_str().unwrap());
        // Host overlay takes priority over in-repo.
        assert_eq!(result, Some(overlay));
    }

    #[test]
    fn hook_resolution_neither_present_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let slot_dir = tmp.path().join("slot");

        let result = find_workflow_hook_path(&config_dir, "myproject", slot_dir.to_str().unwrap());
        assert_eq!(result, None);
    }
}
