use ticket_db::TicketRepo;
use workflow_db::{WorkerRepo, WorkflowRepo};

/// Dependencies required to push a worker's tmux status-left label.
///
/// Holds the repositories and worker configuration needed to resolve a
/// worker's assigned ticket, read PR metadata, and construct a
/// `WorkerdClient` for the target worker.
#[derive(Clone)]
pub struct WorkerLabelDeps {
    pub workflow_repo: WorkflowRepo,
    pub ticket_repo: TicketRepo,
    pub worker_repo: WorkerRepo,
    /// Docker container name prefix for workers (e.g., `ur-worker-`).
    pub worker_prefix: String,
}

/// Build the tmux status-left label string for a worker.
///
/// Returns `"[<worker_id> PR-<n>] "` when a PR number is present,
/// or `"[<worker_id>] "` otherwise.
///
/// This function is pure (no I/O) and fully unit-testable.
pub fn build_label(worker_id: &str, pr_number: Option<&str>) -> String {
    match pr_number {
        Some(pr) => format!("[{worker_id} PR-{pr}] "),
        None => format!("[{worker_id}] "),
    }
}

/// Resolve the worker's assigned ticket, read ticket meta to find a PR number,
/// build the label string, and push it to the worker's workerd daemon.
///
/// Returns `Err` on any failure. Does not log — callers decide the log level.
pub async fn push(deps: &WorkerLabelDeps, worker_id: &str) -> anyhow::Result<()> {
    // 1. Resolve ticket_id from worker_id via the workflow repo.
    let ticket_id = deps
        .workflow_repo
        .ticket_id_by_worker_id(worker_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no active workflow found for worker {worker_id}"))?;

    // 2. Read ticket meta and look up pr_number.
    let meta = deps.ticket_repo.get_meta(&ticket_id, "ticket").await?;
    let pr_number = meta.get("pr_number").map(String::as_str);

    // 3. Build the label string.
    let label = build_label(worker_id, pr_number);

    // 4. Construct a WorkerdClient for the worker and call set_status_left.
    let worker = deps
        .worker_repo
        .get_worker(worker_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found in database"))?;

    let container_name = format!("{}{}", deps.worker_prefix, worker.process_id);
    let workerd_addr = format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);

    let workerd_client = crate::WorkerdClient::new(workerd_addr);
    workerd_client
        .set_status_left(&label, 50)
        .await
        .map_err(|e| anyhow::anyhow!("workerd SetStatusLeft failed: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_label;

    #[test]
    fn build_label_without_pr_number() {
        assert_eq!(build_label("ur-abc12", None), "[ur-abc12] ");
    }

    #[test]
    fn build_label_with_pr_number() {
        assert_eq!(build_label("ur-abc12", Some("325")), "[ur-abc12 PR-325] ");
    }
}
