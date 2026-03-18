use anyhow::bail;
use remote_repo::{GhBackend, MergeStrategy, RemoteRepo};
use tracing::{info, warn};
use ur_db::model::{EdgeKind, LifecycleStatus, TicketFilter, TicketUpdate};

use crate::workflow::{HandlerFuture, TransitionKey, WorkflowContext, WorkflowHandler};

/// Handler for the FeedbackCreating → FeedbackResolving transition.
///
/// Reads `feedback_mode` metadata on the ticket (set by `ur ticket approve` or
/// `ur admin autoapprove`) to determine the resolution path:
///
/// - **feedback_later**: merge the PR, close the original ticket, and dispatch
///   follow-up epic children as independent work (new branches, new PRs).
/// - **feedback_now**: close the original ticket, copy branch + PR meta to the
///   follow-up epic, and transition it to implementing (triggers DispatchImplement).
pub struct FeedbackResolveHandler;

impl WorkflowHandler for FeedbackResolveHandler {
    fn handle(
        &self,
        ctx: &WorkflowContext,
        ticket_id: &str,
        _transition: &TransitionKey,
    ) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move {
            // 1. Load ticket.
            let _ticket = ctx
                .ticket_repo
                .get_ticket(&ticket_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

            // 2. Read metadata.
            let meta = ctx.ticket_repo.get_meta(&ticket_id, "ticket").await?;

            let feedback_mode = meta.get("feedback_mode").ok_or_else(|| {
                anyhow::anyhow!(
                    "no feedback_mode metadata on ticket {ticket_id} — cannot resolve feedback"
                )
            })?;

            let worker_id = meta.get("worker_id").ok_or_else(|| {
                anyhow::anyhow!(
                    "no worker_id metadata on ticket {ticket_id} — cannot resolve feedback"
                )
            })?;

            // 3. Find follow-up epic via follow_up edge.
            let follow_up_epic_id = find_follow_up_epic(&ctx, &ticket_id).await?;

            info!(
                ticket_id = %ticket_id,
                feedback_mode = %feedback_mode,
                follow_up_epic_id = %follow_up_epic_id,
                "resolving feedback"
            );

            // 4. Kill worker and release slot.
            kill_worker(&ctx, worker_id).await?;

            match feedback_mode.as_str() {
                "feedback_later" => {
                    resolve_feedback_later(&ctx, &ticket_id, &meta, &follow_up_epic_id).await
                }
                "feedback_now" => {
                    resolve_feedback_now(&ctx, &ticket_id, &meta, &follow_up_epic_id).await
                }
                other => bail!("unknown feedback_mode '{}' on ticket {}", other, ticket_id),
            }
        })
    }
}

/// Find the follow-up epic ID linked to the given ticket via a `follow_up` edge.
async fn find_follow_up_epic(
    ctx: &WorkflowContext,
    ticket_id: &str,
) -> Result<String, anyhow::Error> {
    let edges = ctx
        .ticket_repo
        .edges_for(ticket_id, Some(EdgeKind::FollowUp))
        .await?;

    // The follow_up edge has source_id = original ticket, target_id = follow-up epic.
    // But we also handle the reverse direction just in case.
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

/// Close the original ticket by setting status=closed and lifecycle_status=done.
async fn close_ticket(ctx: &WorkflowContext, ticket_id: &str) -> Result<(), anyhow::Error> {
    let update = TicketUpdate {
        status: Some("closed".to_string()),
        lifecycle_status: Some(LifecycleStatus::Done),
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

/// feedback_later path:
/// 1. `gh pr merge <pr_number> --squash`
/// 2. If merge fails due to conflicts → transition back to pushing for worker resolution
/// 3. Close original ticket
/// 4. Dispatch follow-up epic children as independent work (transition to open)
async fn resolve_feedback_later(
    ctx: &WorkflowContext,
    ticket_id: &str,
    meta: &std::collections::HashMap<String, String>,
    follow_up_epic_id: &str,
) -> Result<(), anyhow::Error> {
    let pr_number = meta.get("pr_number").ok_or_else(|| {
        anyhow::anyhow!("no pr_number metadata on ticket {ticket_id} — cannot merge PR")
    })?;

    let gh_repo = meta.get("gh_repo").ok_or_else(|| {
        anyhow::anyhow!("no gh_repo metadata on ticket {ticket_id} — cannot merge PR")
    })?;

    // 1. Merge the PR via GhBackend through builderd.
    info!(
        ticket_id = %ticket_id,
        pr_number = %pr_number,
        gh_repo = %gh_repo,
        "merging PR via GhBackend::merge_pr --squash"
    );

    let backend = GhBackend {
        builderd_addr: ctx.builderd_addr.clone(),
        gh_repo: gh_repo.clone(),
    };
    let pr_num: i64 = pr_number
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid pr_number '{pr_number}' on ticket {ticket_id}"))?;
    let result = backend.merge_pr(pr_num, MergeStrategy::Squash).await?;

    if !result.success {
        let error_lower = result.error_message.to_lowercase();

        // Merge conflicts are recoverable: transition back to pushing so a
        // worker can resolve conflicts, push, and re-enter the review cycle.
        if error_lower.contains("merge conflict")
            || error_lower.contains("not mergeable")
            || error_lower.contains("conflicts")
        {
            warn!(
                ticket_id = %ticket_id,
                pr_number = %pr_number,
                error = %result.error_message,
                "merge failed due to conflicts — transitioning back to pushing for worker resolution"
            );

            let update = TicketUpdate {
                lifecycle_status: Some(LifecycleStatus::Pushing),
                status: None,
                type_: None,
                priority: None,
                title: None,
                body: None,
                branch: None,
                parent_id: None,
                project: None,
            };
            ctx.ticket_repo.update_ticket(ticket_id, &update).await?;
            return Ok(());
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

    // 2. Close original ticket.
    close_ticket(ctx, ticket_id).await?;

    // 3. Dispatch follow-up epic children as independent work.
    // List children of the follow-up epic and transition open ones to open lifecycle.
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
        if child.lifecycle_status == LifecycleStatus::Design
            || child.lifecycle_status == LifecycleStatus::Open
        {
            // Each child gets its own branch (no branch set = DispatchImplement generates one).
            // Clear any inherited branch so they start fresh.
            let update = TicketUpdate {
                lifecycle_status: Some(LifecycleStatus::Open),
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
    }

    info!(
        ticket_id = %ticket_id,
        follow_up_epic_id = %follow_up_epic_id,
        children_dispatched = children.len(),
        "feedback_later resolution complete"
    );

    Ok(())
}

/// feedback_now path:
/// 1. Close original ticket
/// 2. Copy branch + PR meta to follow-up epic
/// 3. Transition follow-up epic to implementing (triggers DispatchImplement)
async fn resolve_feedback_now(
    ctx: &WorkflowContext,
    ticket_id: &str,
    meta: &std::collections::HashMap<String, String>,
    follow_up_epic_id: &str,
) -> Result<(), anyhow::Error> {
    // 1. Close original ticket.
    close_ticket(ctx, ticket_id).await?;

    // 2. Copy branch + PR meta to follow-up epic.
    if let Some(branch) = meta.get("branch") {
        // Set branch on the follow-up epic ticket record.
        let update = TicketUpdate {
            branch: Some(Some(branch.clone())),
            status: None,
            lifecycle_status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            parent_id: None,
            project: None,
        };
        ctx.ticket_repo
            .update_ticket(follow_up_epic_id, &update)
            .await?;
    }

    if let Some(pr_number) = meta.get("pr_number") {
        ctx.ticket_repo
            .set_meta(follow_up_epic_id, "ticket", "pr_number", pr_number)
            .await?;
    }

    // Also copy branch from the original ticket record if not in meta.
    let original_ticket = ctx.ticket_repo.get_ticket(ticket_id).await?;
    if let Some(ref ticket) = original_ticket
        && let Some(ref branch) = ticket.branch
    {
        let update = TicketUpdate {
            branch: Some(Some(branch.clone())),
            status: None,
            lifecycle_status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            parent_id: None,
            project: None,
        };
        ctx.ticket_repo
            .update_ticket(follow_up_epic_id, &update)
            .await?;
    }

    // 3. Transition follow-up epic to implementing (triggers DispatchImplement).
    let update = TicketUpdate {
        lifecycle_status: Some(LifecycleStatus::Implementing),
        status: None,
        type_: None,
        priority: None,
        title: None,
        body: None,
        branch: None,
        parent_id: None,
        project: None,
    };
    ctx.ticket_repo
        .update_ticket(follow_up_epic_id, &update)
        .await?;

    info!(
        ticket_id = %ticket_id,
        follow_up_epic_id = %follow_up_epic_id,
        "feedback_now resolution complete — follow-up epic transitioned to implementing"
    );

    Ok(())
}
