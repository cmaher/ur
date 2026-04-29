use anyhow::bail;
use local_repo::LocalRepo;
use tracing::{info, warn};

use crate::workflow::{HandlerFuture, WorkflowContext, WorkflowHandler};

/// Handler for all transitions into Implementing.
///
/// This is the single entry point for dispatching implement work to a worker,
/// covering initial dispatch and all re-dispatch paths:
/// - Initial dispatch (AwaitingDispatch → Implementing)
/// - Verification failure re-dispatch (Verifying → Implementing)
/// - CI failure re-dispatch (Pushing → Implementing via poller)
/// - Merge conflict re-dispatch (Merging → Implementing)
/// - Feedback re-dispatch (AddressingFeedback → Implementing)
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
    // 0. Load ticket — needed for both cycle-limit check (project lookup) and
    //    branch resolution later.
    let ticket = ctx
        .ticket_repo
        .get_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ticket not found: {ticket_id}"))?;

    // 0b. Check implement cycle limit before doing any work.
    if check_cycle_limit(ctx, ticket_id, &ticket.project).await? {
        return Ok(());
    }

    // 0c. Increment implement_cycles for this transition.
    ctx.workflow_repo
        .increment_implement_cycles(ticket_id)
        .await?;

    // 1. Resolve the assigned worker from workflow table.
    let workflow = ctx
        .workflow_repo
        .get_workflow_by_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no workflow found for ticket {ticket_id}"))?;
    if workflow.worker_id.is_empty() {
        anyhow::bail!(
            "no worker_id on workflow for ticket {ticket_id} — cannot dispatch implement"
        );
    }
    let worker_id = &workflow.worker_id;

    // 2. Look up the worker to get process_id for container name derivation.
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

    // 3. Ensure branch is set on the ticket.
    let branch = ensure_ticket_branch(ctx, &ticket, ticket_id, worker_id).await?;

    // 4. Derive workerd gRPC address and send Implement RPC.
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
    ticket: &ticket_db::Ticket,
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
    let update = ticket_db::TicketUpdate {
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

/// Check whether the implement cycle limit has been reached for this workflow.
///
/// Returns `true` if the limit was reached and the workflow was stalled (caller
/// should return `Ok(())` without dispatching). Returns `false` if dispatch
/// should proceed.
///
/// Resolves the effective limit by looking up `project_key` in config:
/// per-project `max_implement_cycles` takes precedence over the server-wide
/// value. Falls back to the server value for orphaned tickets (project not in
/// config).
async fn check_cycle_limit(
    ctx: &WorkflowContext,
    ticket_id: &str,
    project_key: &str,
) -> anyhow::Result<bool> {
    let max_cycles = resolve_cycle_limit(ctx, project_key);

    let max_cycles = match max_cycles {
        Some(max) => max,
        None => return Ok(false), // No limit configured.
    };

    let workflow = ctx
        .workflow_repo
        .get_workflow_by_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no workflow found for ticket {ticket_id}"))?;

    if workflow.implement_cycles as u32 >= max_cycles {
        let reason = format!(
            "implement cycle limit reached ({}/{})",
            workflow.implement_cycles, max_cycles
        );
        warn!(
            ticket_id = %ticket_id,
            implement_cycles = workflow.implement_cycles,
            max_cycles = max_cycles,
            "implement cycle limit reached — stalling workflow"
        );
        ctx.workflow_repo
            .set_workflow_stalled(ticket_id, &reason)
            .await?;
        return Ok(true);
    }

    Ok(false)
}

/// Resolve the effective implement cycle limit for a project key.
///
/// Precedence: per-project override → server-wide default → `None` (no limit).
fn resolve_cycle_limit(ctx: &WorkflowContext, project_key: &str) -> Option<u32> {
    if let Some(project_config) = ctx.config.projects.get(project_key)
        && project_config.max_implement_cycles.is_some()
    {
        return project_config.max_implement_cycles;
    }
    ctx.config.server.max_implement_cycles
}
