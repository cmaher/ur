use std::future::Future;
use std::pin::Pin;

use anyhow::bail;
use tracing::info;

use crate::workflow::{TransitionKey, WorkflowContext, WorkflowHandler};

/// Handler for the InReview → FeedbackCreating transition.
///
/// Resolves the existing worker assigned to this ticket and sends the
/// `CreateFeedbackTickets(ticket_id, pr_number)` RPC. The worker creates a
/// follow-up epic with child tickets from PR comments, links the follow-up
/// epic to the original ticket via a `follow_up` edge, and transitions
/// `lifecycle_status` to `feedback_resolving` when done.
///
/// `pr_number` is expected as metadata on the ticket (set by the `/push` skill).
pub struct FeedbackCreateHandler;

impl WorkflowHandler for FeedbackCreateHandler {
    fn handle(
        &self,
        ctx: &WorkflowContext,
        ticket_id: &str,
        _transition: &TransitionKey,
    ) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + '_>> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move {
            // 1. Load ticket to verify it exists.
            let _ticket = ctx
                .ticket_repo
                .get_ticket(&ticket_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

            // 2. Read metadata: worker_id and pr_number.
            let meta = ctx.ticket_repo.get_meta(&ticket_id, "ticket").await?;

            let worker_id = meta.get("worker_id").ok_or_else(|| {
                anyhow::anyhow!(
                    "no worker_id metadata on ticket {ticket_id} — cannot dispatch feedback create"
                )
            })?;

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

            let workerd_client = crate::WorkerdClient::new(workerd_addr);
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
