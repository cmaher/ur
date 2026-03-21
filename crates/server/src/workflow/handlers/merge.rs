use anyhow::bail;
use remote_repo::{GhBackend, MergeStrategy, RemoteRepo};
use tracing::{error, info, warn};
use ur_db::model::{LifecycleStatus, TicketUpdate};
use ur_rpc::workflow_condition;

use crate::WorkerId;
use crate::workflow::ticket_client::{self, TicketClient};
use crate::workflow::{HandlerFuture, TransitionRequest, WorkflowContext, WorkflowHandler};

/// Handler for the Merging transition.
///
/// Verifies all three workflow conditions (CI, mergeable, review) before
/// attempting merge. On failure, creates a child ticket via `TicketClient`
/// with error context and transitions back to Implementing.
pub struct MergeHandler {
    pub ticket_client: TicketClient,
}

impl WorkflowHandler for MergeHandler {
    fn handle(&self, ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        let ticket_client = self.ticket_client.clone();
        Box::pin(async move { execute_merge(&ctx, &ticket_id, &ticket_client).await })
    }
}

async fn execute_merge(
    ctx: &WorkflowContext,
    ticket_id: &str,
    ticket_client: &TicketClient,
) -> Result<(), anyhow::Error> {
    // 1. Load ticket.
    ctx.ticket_repo
        .get_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

    // 2. Read worker_id from workflow table, pr_number/gh_repo from ticket metadata.
    let workflow = ctx
        .ticket_repo
        .get_workflow_by_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no workflow found for ticket {ticket_id}"))?;
    if workflow.worker_id.is_empty() {
        anyhow::bail!("no worker_id on workflow for ticket {ticket_id} — cannot merge");
    }
    let worker_id = &workflow.worker_id;

    // 3. Pre-merge gate: verify all three conditions before attempting merge.
    check_pre_merge_conditions(&workflow, ticket_id)?;

    let meta = ctx.ticket_repo.get_meta(ticket_id, "ticket").await?;

    let pr_number = meta.get("pr_number").ok_or_else(|| {
        anyhow::anyhow!("no pr_number metadata on ticket {ticket_id} — cannot merge PR")
    })?;

    let gh_repo = meta.get("gh_repo").ok_or_else(|| {
        anyhow::anyhow!("no gh_repo metadata on ticket {ticket_id} — cannot merge PR")
    })?;

    info!(
        ticket_id = %ticket_id,
        pr_number = %pr_number,
        "merging PR"
    );

    // 4. Kill worker and release slot.
    kill_worker(ctx, worker_id).await?;

    // 5. Merge the PR via GhBackend through builderd.
    merge_pr(ctx, ticket_id, pr_number, gh_repo, ticket_client).await?;

    // 6. Mark workflow as done (no further transitions for this ticket).
    if let Err(e) = ctx
        .ticket_repo
        .update_workflow_status(ticket_id, LifecycleStatus::Done)
        .await
    {
        warn!(ticket_id = %ticket_id, error = %e, "failed to mark workflow as done");
    }

    // 7. Close ticket.
    close_ticket(ctx, ticket_id).await
}

/// Verify ci_status=succeeded, mergeable=mergeable, and review_status=approved.
/// Returns an error if any condition is not met (race between poller and handler).
fn check_pre_merge_conditions(
    workflow: &ur_db::model::Workflow,
    ticket_id: &str,
) -> Result<(), anyhow::Error> {
    let mut failures = Vec::new();

    if workflow.ci_status != workflow_condition::ci_status::SUCCEEDED {
        failures.push(format!(
            "ci_status={} (expected {})",
            workflow.ci_status,
            workflow_condition::ci_status::SUCCEEDED,
        ));
    }
    if workflow.mergeable != workflow_condition::mergeable::MERGEABLE {
        failures.push(format!(
            "mergeable={} (expected {})",
            workflow.mergeable,
            workflow_condition::mergeable::MERGEABLE,
        ));
    }
    if workflow.review_status != workflow_condition::review_status::APPROVED {
        failures.push(format!(
            "review_status={} (expected {})",
            workflow.review_status,
            workflow_condition::review_status::APPROVED,
        ));
    }

    if !failures.is_empty() {
        bail!(
            "pre-merge conditions not met for ticket {ticket_id}: {}",
            failures.join(", ")
        );
    }
    Ok(())
}

async fn merge_pr(
    ctx: &WorkflowContext,
    ticket_id: &str,
    pr_number: &str,
    gh_repo: &str,
    ticket_client: &TicketClient,
) -> Result<(), anyhow::Error> {
    info!(
        ticket_id = %ticket_id,
        pr_number = %pr_number,
        gh_repo = %gh_repo,
        "merging PR via GhBackend::merge_pr --squash"
    );

    let backend = GhBackend {
        client: ctx.builderd_client.clone(),
        gh_repo: gh_repo.to_owned(),
    };
    let pr_num: i64 = pr_number
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid pr_number '{pr_number}' on ticket {ticket_id}"))?;
    let result = backend.merge_pr(pr_num, MergeStrategy::Squash).await?;

    if !result.success {
        if is_merge_conflict(&result.error_message) {
            return handle_merge_conflict(
                ctx,
                ticket_id,
                pr_number,
                &result.error_message,
                ticket_client,
            )
            .await;
        }
        return handle_merge_rejection(
            ctx,
            ticket_id,
            pr_number,
            &result.error_message,
            ticket_client,
        )
        .await;
    }

    info!(
        ticket_id = %ticket_id,
        pr_number = %pr_number,
        sha = %result.sha,
        "PR merged successfully"
    );
    Ok(())
}

/// Stop the worker container, release its pool slot, and mark it stopped.
async fn kill_worker(ctx: &WorkflowContext, worker_id: &str) -> Result<(), anyhow::Error> {
    ctx.worker_manager
        .stop_by_worker_id(&WorkerId(worker_id.to_owned()))
        .await
        .map_err(|e| anyhow::anyhow!("failed to stop worker {worker_id}: {e}"))
}

/// Close a ticket by setting status=closed and lifecycle_status=done.
async fn close_ticket(ctx: &WorkflowContext, ticket_id: &str) -> Result<(), anyhow::Error> {
    let update = TicketUpdate {
        status: Some("closed".to_string()),
        lifecycle_status: Some(LifecycleStatus::Done),
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
    info!(ticket_id = %ticket_id, "ticket closed (lifecycle_status=done)");
    Ok(())
}

fn is_merge_conflict(error_message: &str) -> bool {
    let error_lower = error_message.to_lowercase();
    error_lower.contains("merge conflict")
        || error_lower.contains("not mergeable")
        || error_lower.contains("conflicts")
}

async fn handle_merge_conflict(
    ctx: &WorkflowContext,
    ticket_id: &str,
    pr_number: &str,
    error_message: &str,
    ticket_client: &TicketClient,
) -> Result<(), anyhow::Error> {
    warn!(
        ticket_id = %ticket_id,
        pr_number = %pr_number,
        error = %error_message,
        "merge failed due to conflicts — creating issue ticket and transitioning to implementing"
    );

    let body = format!(
        "PR #{pr_number} merge failed due to conflicts.\n\n\
         Error: {error_message}\n\n\
         Resolve the merge conflicts and push updated changes."
    );
    if let Err(e) = ticket_client
        .create_workflow_issue_ticket(
            ticket_id,
            ticket_client::issue_type::MERGE_CONFLICT,
            &format!("Merge conflict on PR #{pr_number}"),
            &body,
        )
        .await
    {
        error!(
            ticket_id = %ticket_id,
            error = %e,
            "failed to create merge conflict issue ticket"
        );
    }

    let message = format!(
        "[workflow] merge conflict detected\n\
         source: workflow\n\
         result: fail\n\
         ---\n\
         PR #{pr_number} merge failed: {error_message}"
    );
    if let Err(e) = ctx
        .ticket_repo
        .add_activity(ticket_id, "workflow", &message)
        .await
    {
        error!(ticket_id = %ticket_id, error = %e, "failed to add merge conflict activity");
    }

    transition_to_implementing(ctx, ticket_id).await
}

async fn handle_merge_rejection(
    ctx: &WorkflowContext,
    ticket_id: &str,
    pr_number: &str,
    error_message: &str,
    ticket_client: &TicketClient,
) -> Result<(), anyhow::Error> {
    warn!(
        ticket_id = %ticket_id,
        pr_number = %pr_number,
        error = %error_message,
        "merge rejected — creating issue ticket and transitioning to implementing"
    );

    let body = format!(
        "PR #{pr_number} merge was rejected.\n\n\
         Error: {error_message}\n\n\
         This may be caused by branch protection rules or other repository settings."
    );
    if let Err(e) = ticket_client
        .create_workflow_issue_ticket(
            ticket_id,
            ticket_client::issue_type::MERGE_REJECTION,
            &format!("Merge rejected on PR #{pr_number}"),
            &body,
        )
        .await
    {
        error!(
            ticket_id = %ticket_id,
            error = %e,
            "failed to create merge rejection issue ticket"
        );
    }

    let message = format!(
        "[workflow] merge rejected\n\
         source: workflow\n\
         result: fail\n\
         ---\n\
         PR #{pr_number} merge rejected: {error_message}"
    );
    if let Err(e) = ctx
        .ticket_repo
        .add_activity(ticket_id, "workflow", &message)
        .await
    {
        error!(ticket_id = %ticket_id, error = %e, "failed to add merge rejection activity");
    }

    transition_to_implementing(ctx, ticket_id).await
}

async fn transition_to_implementing(
    ctx: &WorkflowContext,
    ticket_id: &str,
) -> Result<(), anyhow::Error> {
    ctx.transition_tx
        .send(TransitionRequest {
            ticket_id: ticket_id.to_owned(),
            target_status: LifecycleStatus::Implementing,
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to send Implementing transition: {e}"))?;
    Ok(())
}
