use anyhow::bail;
use tracing::info;

use crate::workflow::{HandlerFuture, TransitionKey, WorkflowContext, WorkflowHandler};

/// Handler for the Implementing → Pushing transition.
///
/// Checks whether a worker is already active for this ticket (via `worker_id`
/// metadata on the ticket). If a running worker exists, sends the `Push()` RPC
/// directly. If no worker is active, dispatches a new worker on the ticket's
/// branch and then sends `Push()`.
///
/// The worker's `/push` skill handles PR creation, merge conflicts, and CI
/// fixes. It also sets `pr_number` and `pr_url` metadata on the ticket.
/// The subsequent transition to InReview is owned by GithubPoller, not this
/// handler.
pub struct DispatchPushHandler;

impl WorkflowHandler for DispatchPushHandler {
    fn handle(
        &self,
        ctx: &WorkflowContext,
        ticket_id: &str,
        _transition: &TransitionKey,
    ) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move {
            // 1. Load ticket to verify it exists and has a branch.
            let ticket = ctx
                .ticket_repo
                .get_ticket(&ticket_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

            let branch = ticket.branch.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "ticket {ticket_id} has no branch set — cannot push without a branch"
                )
            })?;

            info!(
                ticket_id = %ticket_id,
                branch = %branch,
                "dispatch_push: resolving worker for push"
            );

            // 2. Check for an assigned worker via ticket metadata.
            let meta = ctx.ticket_repo.get_meta(&ticket_id, "ticket").await?;
            let worker_id = meta.get("worker_id").ok_or_else(|| {
                anyhow::anyhow!(
                    "no worker_id metadata on ticket {ticket_id} — cannot dispatch push"
                )
            })?;

            // 3. Look up the worker record.
            let worker = ctx
                .worker_repo
                .get_worker(worker_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found in database"))?;

            if worker.container_status != "running" {
                bail!(
                    "worker {} is not running (status: {}) — cannot dispatch push for ticket {}",
                    worker_id,
                    worker.container_status,
                    ticket_id
                );
            }

            // 4. Derive workerd gRPC address and send Push RPC.
            let container_name = format!("{}{}", ctx.worker_prefix, worker.process_id);
            let workerd_addr =
                format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);

            info!(
                ticket_id = %ticket_id,
                branch = %branch,
                worker_id = %worker_id,
                workerd_addr = %workerd_addr,
                "dispatching push RPC to workerd"
            );

            let workerd_client = crate::WorkerdClient::new(workerd_addr);
            workerd_client
                .push()
                .await
                .map_err(|e| anyhow::anyhow!("workerd push RPC failed: {e}"))?;

            info!(
                ticket_id = %ticket_id,
                worker_id = %worker_id,
                "push dispatched successfully"
            );

            Ok(())
        })
    }
}
