use anyhow::bail;
use local_repo::LocalRepo;
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
/// - If not set, reads the worker's current branch from the checkout and persists it.
///
/// Then resolves the assigned worker (via `worker_id` metadata) and sends
/// the `Implement(ticket_id)` RPC to the worker's workerd daemon. The workerd
/// handler sends /clear before /implement to ensure a fresh agent context.
pub struct ImplementHandler;

impl WorkflowHandler for ImplementHandler {
    fn handle(&self, ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move { dispatch_implement(&ctx, &ticket_id).await })
    }
}

async fn dispatch_implement(ctx: &WorkflowContext, ticket_id: &str) -> anyhow::Result<()> {
    // 1. Load ticket to check branch field.
    let ticket = ctx
        .ticket_repo
        .get_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

    // 2. Resolve the assigned worker from ticket metadata.
    let meta = ctx.ticket_repo.get_meta(ticket_id, "ticket").await?;
    let worker_id = meta.get("worker_id").ok_or_else(|| {
        anyhow::anyhow!("no worker_id metadata on ticket {ticket_id} — cannot dispatch implement")
    })?;

    // 3. Look up the worker to get process_id for container name derivation.
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

    // 4. Ensure branch is set on the ticket.
    let branch = ensure_ticket_branch(ctx, &ticket, ticket_id, worker_id).await?;

    // 5. Derive workerd gRPC address and send Implement RPC.
    let container_name = format!("{}{}", ctx.worker_prefix, worker.process_id);
    let workerd_addr = format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);

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
        .implement(ticket_id)
        .await
        .map_err(|e| anyhow::anyhow!("workerd implement RPC failed: {e}"))?;

    info!(
        ticket_id = %ticket_id,
        worker_id = %worker_id,
        "implement dispatched successfully"
    );

    Ok(())
}

/// Return the ticket's branch, reading it from the worker's checkout and persisting
/// it on the ticket if not already set.
async fn ensure_ticket_branch(
    ctx: &WorkflowContext,
    ticket: &ur_db::model::Ticket,
    ticket_id: &str,
    worker_id: &str,
) -> anyhow::Result<String> {
    if let Some(ref b) = ticket.branch {
        info!(
            ticket_id = %ticket_id,
            branch = %b,
            "ticket already has branch — worker will checkout + pull"
        );
        return Ok(b.clone());
    }

    let branch = read_worker_branch(ctx, worker_id, ticket_id).await?;
    info!(
        ticket_id = %ticket_id,
        branch = %branch,
        "no branch set — read current branch from worker checkout"
    );
    let update = ur_db::model::TicketUpdate {
        branch: Some(Some(branch.clone())),
        ..Default::default()
    };
    ctx.ticket_repo.update_ticket(ticket_id, &update).await?;
    Ok(branch)
}

/// Read the current git branch from the worker's checkout directory via builderd.
async fn read_worker_branch(
    ctx: &WorkflowContext,
    worker_id: &str,
    ticket_id: &str,
) -> anyhow::Result<String> {
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

    let local_repo = local_repo::GitBackend {
        client: ctx.builderd_client.clone(),
    };
    let branch = local_repo.current_branch(&slot.host_path).await?;

    if branch == "HEAD" {
        anyhow::bail!(
            "worker {worker_id} checkout is in detached HEAD state — \
             cannot determine branch for ticket {ticket_id}"
        );
    }

    Ok(branch)
}
