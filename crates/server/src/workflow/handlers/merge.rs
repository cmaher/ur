use anyhow::bail;
use remote_repo::{GhBackend, MergeStrategy, RemoteRepo};
use tracing::{error, info, warn};
use ur_db::model::{LifecycleStatus, TicketFilter, TicketUpdate};

use crate::workflow::{HandlerFuture, TransitionRequest, WorkflowContext, WorkflowHandler};

/// Handler for the FeedbackCreating → Merging transition.
///
/// Merges the PR (squash), kills the worker, closes the epic, and dispatches
/// follow-up children as independent work with cleared branches.
///
/// If the merge fails due to a conflict, transitions back to Implementing.
pub struct MergeHandler;

impl WorkflowHandler for MergeHandler {
    fn handle(&self, ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move { execute_merge(&ctx, &ticket_id).await })
    }
}

async fn execute_merge(ctx: &WorkflowContext, ticket_id: &str) -> Result<(), anyhow::Error> {
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

    let meta = ctx.ticket_repo.get_meta(ticket_id, "ticket").await?;

    let pr_number = meta.get("pr_number").ok_or_else(|| {
        anyhow::anyhow!("no pr_number metadata on ticket {ticket_id} — cannot merge PR")
    })?;

    let gh_repo = meta.get("gh_repo").ok_or_else(|| {
        anyhow::anyhow!("no gh_repo metadata on ticket {ticket_id} — cannot merge PR")
    })?;

    // 3. Find follow-up epic via follow_up edge.
    let follow_up_epic_id = find_follow_up_epic(ctx, ticket_id).await?;

    info!(
        ticket_id = %ticket_id,
        follow_up_epic_id = %follow_up_epic_id,
        pr_number = %pr_number,
        "merging PR"
    );

    // 4. Kill worker and release slot.
    kill_worker(ctx, worker_id).await?;

    // 5. Merge the PR via GhBackend through builderd.
    merge_pr(ctx, ticket_id, pr_number, gh_repo).await?;

    // 6. Mark workflow as done (no further transitions for this ticket).
    if let Err(e) = ctx
        .ticket_repo
        .update_workflow_status(ticket_id, LifecycleStatus::Done)
        .await
    {
        warn!(ticket_id = %ticket_id, error = %e, "failed to mark workflow as done");
    }

    // 7. Close original ticket and follow-up epic.
    close_ticket(ctx, ticket_id).await?;
    close_ticket(ctx, &follow_up_epic_id).await?;

    // 8. Dispatch follow-up epic children as independent work.
    dispatch_children(ctx, ticket_id, &follow_up_epic_id).await
}

async fn merge_pr(
    ctx: &WorkflowContext,
    ticket_id: &str,
    pr_number: &str,
    gh_repo: &str,
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
            return handle_merge_conflict(ctx, ticket_id, pr_number, &result.error_message).await;
        }
        bail!(
            "merge_pr failed for PR #{} on ticket {}: {}",
            pr_number,
            ticket_id,
            result.error_message
        );
    }

    info!(
        ticket_id = %ticket_id,
        pr_number = %pr_number,
        sha = %result.sha,
        "PR merged successfully"
    );
    Ok(())
}

async fn dispatch_children(
    ctx: &WorkflowContext,
    ticket_id: &str,
    follow_up_epic_id: &str,
) -> Result<(), anyhow::Error> {
    let children = ctx
        .ticket_repo
        .list_tickets(&TicketFilter {
            parent_id: Some(follow_up_epic_id.to_string()),
            status: Some("open".to_string()),
            project: None,
            type_: None,
            lifecycle_status: None,
        })
        .await?;

    for child in &children {
        if child.lifecycle_status != LifecycleStatus::Design
            && child.lifecycle_status != LifecycleStatus::Open
        {
            continue;
        }
        // Clear any inherited branch so they start fresh.
        let update = TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::Open),
            lifecycle_managed: None,
            branch: Some(None),
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            parent_id: None,
            project: None,
        };
        ctx.ticket_repo.update_ticket(&child.id, &update).await?;
        info!(
            child_id = %child.id,
            follow_up_epic_id = %follow_up_epic_id,
            "dispatched follow-up child as independent work (lifecycle → open)"
        );
    }

    info!(
        ticket_id = %ticket_id,
        follow_up_epic_id = %follow_up_epic_id,
        children_dispatched = children.len(),
        "merge resolution complete"
    );

    Ok(())
}

/// Find the follow-up epic ID linked to the given ticket via a `follow_up` edge.
async fn find_follow_up_epic(
    ctx: &WorkflowContext,
    ticket_id: &str,
) -> Result<String, anyhow::Error> {
    let edges = ctx
        .ticket_repo
        .edges_for(ticket_id, Some(ur_db::model::EdgeKind::FollowUp))
        .await?;

    for edge in &edges {
        if edge.source_id == ticket_id {
            return Ok(edge.target_id.clone());
        }
    }
    for edge in &edges {
        if edge.target_id == ticket_id {
            return Ok(edge.source_id.clone());
        }
    }

    anyhow::bail!("no follow_up edge found for ticket {ticket_id}")
}

/// Kill the worker and mark it stopped, releasing its slot.
async fn kill_worker(ctx: &WorkflowContext, worker_id: &str) -> Result<(), anyhow::Error> {
    let worker = ctx
        .worker_repo
        .get_worker(worker_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found in database"))?;

    if worker.container_status == "stopped" {
        info!(worker_id = %worker_id, "worker already stopped");
        return Ok(());
    }

    // Unlink worker from slot to free it for reuse.
    if let Err(e) = ctx.worker_repo.unlink_worker_slot(worker_id).await {
        warn!(worker_id = %worker_id, error = %e, "failed to unlink worker slot");
    }

    // Mark worker as stopped.
    ctx.worker_repo
        .update_worker_container_status(worker_id, "stopped")
        .await?;

    info!(worker_id = %worker_id, "worker stopped and slot released");
    Ok(())
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
) -> Result<(), anyhow::Error> {
    warn!(
        ticket_id = %ticket_id,
        pr_number = %pr_number,
        error = %error_message,
        "merge failed due to conflicts — sending transition to implementing"
    );

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

    ctx.transition_tx
        .send(TransitionRequest {
            ticket_id: ticket_id.to_owned(),
            target_status: LifecycleStatus::Implementing,
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to send Implementing transition: {e}"))?;
    Ok(())
}
