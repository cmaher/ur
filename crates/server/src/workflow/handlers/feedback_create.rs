use anyhow::bail;
use tracing::info;

use crate::workflow::{HandlerFuture, WorkflowContext, WorkflowHandler};

/// Handler for the InReview → FeedbackCreating transition.
///
/// Promotes the ticket to an epic (if not already) so child feedback tickets
/// can be parented under it, then resolves the existing worker and sends
/// the `CreateFeedbackTickets(ticket_id, pr_number)` RPC. The worker creates
/// child tickets from PR comments and goes idle. The step router detects
/// Idle + FeedbackCreating and routes by `feedback_mode` metadata:
/// - `now` → Implementing
/// - `later` → Merging
///
/// `pr_number` is expected as metadata on the ticket (set by the push workflow handler).
pub struct FeedbackCreateHandler;

impl WorkflowHandler for FeedbackCreateHandler {
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

            // Note: "epic" is no longer a ticket type — any ticket can have
            // children, so no type promotion is needed.

            // 2. Read worker_id from workflow table, pr_number from ticket metadata.
            let workflow = ctx
                .ticket_repo
                .get_workflow_by_ticket(&ticket_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no workflow found for ticket {ticket_id}"))?;
            if workflow.worker_id.is_empty() {
                anyhow::bail!(
                    "no worker_id on workflow for ticket {ticket_id} — cannot dispatch feedback create"
                );
            }
            let worker_id = &workflow.worker_id;

            let meta = ctx.ticket_repo.get_meta(&ticket_id, "ticket").await?;

            let pr_number_str = meta.get("pr_number").ok_or_else(|| {
                anyhow::anyhow!(
                    "no pr_number metadata on ticket {ticket_id} — cannot create feedback tickets"
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

            // 3. Look up the worker record and verify it is running.
            let worker = ctx
                .worker_repo
                .get_worker(worker_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found in database"))?;

            if worker.container_status != "running" {
                bail!(
                    "worker {} is not running (status: {}) — cannot dispatch feedback create for ticket {}",
                    worker_id,
                    worker.container_status,
                    ticket_id
                );
            }

            // 4. Derive workerd gRPC address and send CreateFeedbackTickets RPC.
            let container_name = format!("{}{}", ctx.worker_prefix, worker.process_id);
            let workerd_addr =
                format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);

            info!(
                ticket_id = %ticket_id,
                worker_id = %worker_id,
                pr_number = %pr_number,
                workerd_addr = %workerd_addr,
                "dispatching create_feedback_tickets RPC to workerd"
            );

            let workerd_client = crate::WorkerdClient::with_status_tracking(
                workerd_addr,
                ctx.worker_repo.clone(),
                worker_id.clone(),
            );
            workerd_client
                .create_feedback_tickets(&ticket_id, pr_number)
                .await
                .map_err(|e| anyhow::anyhow!("workerd create_feedback_tickets RPC failed: {e}"))?;

            info!(
                ticket_id = %ticket_id,
                worker_id = %worker_id,
                pr_number = %pr_number,
                "create_feedback_tickets dispatched successfully"
            );

            Ok(())
        })
    }
}
