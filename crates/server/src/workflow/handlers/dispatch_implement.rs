use anyhow::bail;
use tracing::info;

use crate::workflow::{HandlerFuture, WorkflowContext, WorkflowHandler};

/// Handler for all transitions into Implementing.
///
/// This is the single entry point for dispatching implement work to a worker,
/// covering initial dispatch and all re-dispatch paths:
/// - Initial dispatch (AwaitingDispatch → Implementing)
/// - Verification failure re-dispatch (Verifying → Implementing)
/// - CI failure re-dispatch (Pushing → Implementing via poller)
/// - Merge conflict re-dispatch (Merging → Implementing)
/// - Feedback re-dispatch (FeedbackCreating → Implementing)
///
/// Looks up the ticket's branch field:
/// - If set, the worker will check out and pull that branch.
/// - If not set, generates a branch name from the ticket ID and persists it.
///
/// Then resolves the assigned worker (via `worker_id` metadata) and sends
/// the `Implement(ticket_id)` RPC to the worker's workerd daemon. The workerd
/// handler sends /clear before /implement to ensure a fresh agent context.
pub struct ImplementHandler;

impl WorkflowHandler for ImplementHandler {
    fn handle(&self, ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move {
            // 1. Load ticket to check branch field.
            let ticket = ctx
                .ticket_repo
                .get_ticket(&ticket_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

            // 2. Ensure branch is set on the ticket.
            let branch = if let Some(ref b) = ticket.branch {
                info!(
                    ticket_id = %ticket_id,
                    branch = %b,
                    "ticket already has branch — worker will checkout + pull"
                );
                b.clone()
            } else {
                let new_branch = ticket_id.clone();
                info!(
                    ticket_id = %ticket_id,
                    branch = %new_branch,
                    "no branch set — generating from ticket ID"
                );
                let update = ur_db::model::TicketUpdate {
                    branch: Some(Some(new_branch.clone())),
                    status: None,
                    lifecycle_status: None,
                    lifecycle_managed: None,
                    type_: None,
                    priority: None,
                    title: None,
                    body: None,
                    parent_id: None,
                    project: None,
                };
                ctx.ticket_repo.update_ticket(&ticket_id, &update).await?;
                new_branch
            };

            // 3. Resolve the assigned worker from ticket metadata.
            let meta = ctx.ticket_repo.get_meta(&ticket_id, "ticket").await?;
            let worker_id = meta.get("worker_id").ok_or_else(|| {
                anyhow::anyhow!(
                    "no worker_id metadata on ticket {ticket_id} — cannot dispatch implement"
                )
            })?;

            // 4. Look up the worker to get process_id for container name derivation.
            let worker = ctx
                .worker_repo
                .get_worker(worker_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found in database"))?;

            if worker.container_status != "running" {
                bail!(
                    "worker {} is not running (status: {})",
                    worker_id,
                    worker.container_status
                );
            }

            // 5. Derive workerd gRPC address and send Implement RPC.
            let container_name = format!("{}{}", ctx.worker_prefix, worker.process_id);
            let workerd_addr =
                format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);

            info!(
                ticket_id = %ticket_id,
                branch = %branch,
                worker_id = %worker_id,
                workerd_addr = %workerd_addr,
                "dispatching implement RPC to workerd"
            );

            let workerd_client = crate::WorkerdClient::with_status_tracking(
                workerd_addr,
                ctx.worker_repo.clone(),
                worker_id.clone(),
            );
            workerd_client
                .implement(&ticket_id)
                .await
                .map_err(|e| anyhow::anyhow!("workerd implement RPC failed: {e}"))?;

            info!(
                ticket_id = %ticket_id,
                worker_id = %worker_id,
                "implement dispatched successfully"
            );

            Ok(())
        })
    }
}
