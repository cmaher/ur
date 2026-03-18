use anyhow::bail;
use tracing::info;

use crate::workflow::{HandlerFuture, TransitionKey, WorkflowContext, WorkflowHandler};

/// Handler for the Verifying -> Fixing transition.
///
/// Reads the `fix_phase` metadata from the ticket and dispatches the
/// `Fix(ticket_id, fix_phase)` RPC to the worker's workerd daemon.
/// The workerd invokes the `/fix` skill via tmux (fire-and-forget).
///
/// `fix_phase` values: "verify", "ci", "merge"
pub struct FixDispatchHandler;

impl WorkflowHandler for FixDispatchHandler {
    fn handle(
        &self,
        ctx: &WorkflowContext,
        ticket_id: &str,
        _transition: &TransitionKey,
    ) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move {
            // 1. Load ticket to verify it exists.
            let _ticket = ctx
                .ticket_repo
                .get_ticket(&ticket_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

            // 2. Read fix_phase from ticket metadata.
            let meta = ctx.ticket_repo.get_meta(&ticket_id, "ticket").await?;

            let fix_phase = meta.get("fix_phase").ok_or_else(|| {
                anyhow::anyhow!("no fix_phase metadata on ticket {ticket_id} — cannot dispatch fix")
            })?;

            // 3. Resolve the assigned worker from ticket metadata.
            let worker_id = meta.get("worker_id").ok_or_else(|| {
                anyhow::anyhow!("no worker_id metadata on ticket {ticket_id} — cannot dispatch fix")
            })?;

            // 4. Look up the worker record and verify it is running.
            let worker = ctx
                .worker_repo
                .get_worker(worker_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found in database"))?;

            if worker.container_status != "running" {
                bail!(
                    "worker {} is not running (status: {}) — cannot dispatch fix for ticket {}",
                    worker_id,
                    worker.container_status,
                    ticket_id
                );
            }

            // 5. Derive workerd gRPC address and send Fix RPC.
            let container_name = format!("{}{}", ctx.worker_prefix, worker.process_id);
            let workerd_addr =
                format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);

            info!(
                ticket_id = %ticket_id,
                fix_phase = %fix_phase,
                worker_id = %worker_id,
                workerd_addr = %workerd_addr,
                "dispatching fix RPC to workerd"
            );

            let workerd_client = crate::WorkerdClient::new(workerd_addr);
            workerd_client
                .fix(&ticket_id, fix_phase)
                .await
                .map_err(|e| anyhow::anyhow!("workerd fix RPC failed: {e}"))?;

            info!(
                ticket_id = %ticket_id,
                worker_id = %worker_id,
                fix_phase = %fix_phase,
                "fix dispatched successfully"
            );

            Ok(())
        })
    }
}
