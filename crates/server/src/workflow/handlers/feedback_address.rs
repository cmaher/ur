use anyhow::bail;
use tracing::info;

use crate::workflow::{HandlerFuture, WorkflowContext, WorkflowHandler};

/// Handler for the InReview → AddressingFeedback transition.
///
/// Queries `workflow_comments` for comment IDs that have been seen but not
/// yet processed into feedback tickets, then sends the
/// `AddressFeedbackTickets(ticket_id, pr_number, handled_comment_ids)` RPC
/// to the assigned worker. The worker creates child tickets from new PR
/// comments (skipping already-handled ones) and goes idle. The step router
/// detects Idle + AddressingFeedback and routes by `feedback_mode` metadata:
/// - `now` → Implementing
/// - `later` → Merging
///
/// On successful step completion, `mark_feedback_created` is called by the
/// step-complete handler to mark pending comments as processed. If the worker
/// dies mid-way, comments remain `feedback_created = 0` and will be
/// re-processed on recovery.
///
/// `pr_number` is expected as metadata on the ticket (set by the push workflow handler).
pub struct FeedbackAddressHandler;

impl WorkflowHandler for FeedbackAddressHandler {
    fn handle(&self, ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move {
            // 1. Load ticket to verify it exists.
            let _ticket = ctx
                .ticket_repo
                .get_ticket(&ticket_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

            // 2. Read worker_id from workflow table, pr_number from ticket metadata.
            let workflow = ctx
                .workflow_repo
                .get_workflow_by_ticket(&ticket_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no workflow found for ticket {ticket_id}"))?;
            if workflow.worker_id.is_empty() {
                anyhow::bail!(
                    "no worker_id on workflow for ticket {ticket_id} — cannot dispatch address feedback"
                );
            }
            let worker_id = &workflow.worker_id;

            let meta = ctx.ticket_repo.get_meta(&ticket_id, "ticket").await?;

            let pr_number_str = meta.get("pr_number").ok_or_else(|| {
                anyhow::anyhow!(
                    "no pr_number metadata on ticket {ticket_id} — cannot address feedback tickets"
                )
            })?;

            let pr_number: u32 = pr_number_str.parse().map_err(|e| {
                anyhow::anyhow!(
                    "invalid pr_number '{}' on ticket {}: {}",
                    pr_number_str,
                    ticket_id,
                    e
                )
            })?;

            // 3. Query pending comments (seen but not yet feedback_created) and
            //    already-handled comments (feedback_created = 1).
            //    Pending comments are tracked so mark_feedback_created can be called
            //    on step completion. Handled IDs are passed to the worker so it
            //    skips comments that already have feedback tickets.
            let pending_comments = ctx
                .workflow_repo
                .get_pending_feedback_comments(&ticket_id)
                .await?;

            let all_seen = ctx.workflow_repo.get_seen_comment_ids(&ticket_id).await?;

            let pending_set: std::collections::HashSet<&str> =
                pending_comments.iter().map(|s| s.as_str()).collect();
            let handled_comment_ids: Vec<String> = all_seen
                .into_iter()
                .filter(|id| !pending_set.contains(id.as_str()))
                .collect();

            info!(
                ticket_id = %ticket_id,
                pending_count = pending_comments.len(),
                handled_count = handled_comment_ids.len(),
                "queried workflow_comments for address feedback"
            );

            // 4. Look up the worker record and verify it is running.
            let worker = ctx
                .worker_repo
                .get_worker(worker_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found in database"))?;

            if worker.container_status != "running" {
                bail!(
                    "worker {} is not running (status: {}) — cannot dispatch address feedback for ticket {}",
                    worker_id,
                    worker.container_status,
                    ticket_id
                );
            }

            // 5. Derive workerd gRPC address and send AddressFeedbackTickets RPC.
            let container_name = format!("{}{}", ctx.worker_prefix, worker.process_id);
            let workerd_addr =
                format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);

            info!(
                ticket_id = %ticket_id,
                worker_id = %worker_id,
                pr_number = %pr_number,
                workerd_addr = %workerd_addr,
                "dispatching address_feedback_tickets RPC to workerd"
            );

            let workerd_client = crate::WorkerdClient::with_status_tracking(
                workerd_addr,
                ctx.worker_repo.clone(),
                worker_id.clone(),
            );
            workerd_client
                .address_feedback_tickets(&ticket_id, pr_number, handled_comment_ids)
                .await
                .map_err(|e| anyhow::anyhow!("workerd address_feedback_tickets RPC failed: {e}"))?;

            info!(
                ticket_id = %ticket_id,
                worker_id = %worker_id,
                pr_number = %pr_number,
                "address_feedback_tickets dispatched successfully"
            );

            Ok(())
        })
    }
}
