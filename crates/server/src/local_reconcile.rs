//! `runtime = local` worker reconciliation against builderd (ur-j5puq).
//!
//! builderd keeps one invariant: a local worker's tmux session is alive only
//! while builderd lists it in `ListLocalWorkers`. Each pass reads the active
//! local rows, then that list, and converges the rows on it:
//!
//! | row | builderd | action |
//! |---|---|---|
//! | `running` | listed, session alive | none |
//! | `running` | not listed | stop path (Stop of an unknown id is Ok) |
//! | `running` | listed, session dead | stop path |
//! | `stopping` | any | stop path (retry) |
//! | `provisioning` | any | none |
//! | none / `stopped` | listed | `StopLocalWorker` |
//!
//! The stop path (`WorkerManager::stop_by_worker_id`) releases the slot and
//! marks the row `stopped` only after `StopLocalWorker` succeeds, and leaves
//! it `stopping` otherwise. A failed list changes nothing.
//!
//! Rows are read before the list: a row that was `running` then was already
//! registered, so its absence afterwards means builderd has torn it down. An
//! unknown listed id is re-read before it is stopped, because a launch
//! records its row before builderd registers the worker.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{error, info, warn};
use workflow_db::WorkerRepo;

use crate::builder_local_worker_client::BuilderLocalWorkerClient;
use crate::worker::{WorkerId, WorkerManager};

/// How often the loop runs after its startup pass.
pub const DEFAULT_LOCAL_RECONCILE_INTERVAL: Duration = Duration::from_secs(15);

const RUNNING: &str = "running";
const STOPPING: &str = "stopping";
const STOPPED: &str = "stopped";

/// Converges `runtime = local` worker rows on builderd's live local workers.
#[derive(Clone)]
pub struct LocalWorkerReconcileManager {
    worker_manager: WorkerManager,
    worker_repo: WorkerRepo,
    client: BuilderLocalWorkerClient,
    interval: Duration,
}

impl LocalWorkerReconcileManager {
    pub fn new(
        worker_manager: WorkerManager,
        worker_repo: WorkerRepo,
        client: BuilderLocalWorkerClient,
        interval: Duration,
    ) -> Self {
        Self {
            worker_manager,
            worker_repo,
            client,
            interval,
        }
    }

    /// Run a pass now (server startup), then one every `interval`.
    pub fn spawn(self, shutdown_rx: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(self.run(shutdown_rx))
    }

    async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        info!(interval = ?self.interval, "local worker reconciliation started");
        loop {
            if let Err(e) = self.reconcile_once().await {
                warn!(error = %e, "local worker reconcile pass skipped; nothing changed");
            }
            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("local worker reconciliation shutting down");
                        return;
                    }
                }
            }
        }
    }

    /// One pass. `Err` (no row changed) when the rows or builderd's list
    /// cannot be read; per-worker stop failures are logged and left on the
    /// row as `stopping` for the next pass.
    pub async fn reconcile_once(&self) -> Result<(), String> {
        let rows: Vec<_> = self
            .worker_repo
            .list_active_workers()
            .await
            .map_err(|e| format!("list active workers: {e}"))?
            .into_iter()
            .filter(|w| w.runtime == ur_config::WorkerRuntime::Local)
            .collect();
        let listed: HashMap<String, bool> = self
            .client
            .list_local_workers()
            .await
            .map_err(|status| format!("ListLocalWorkers: {status}"))?
            .into_iter()
            .map(|w| (w.worker_id, w.session_alive))
            .collect();

        for row in &rows {
            let why = match (row.container_status.as_str(), listed.get(&row.worker_id)) {
                (RUNNING, Some(true)) => continue,
                (RUNNING, None) => "gone from builderd",
                (RUNNING, Some(false)) => "session dead",
                (STOPPING, _) => "retrying stop",
                _ => continue,
            };
            info!(worker_id = row.worker_id, why, "stopping local worker");
            let worker_id = WorkerId(row.worker_id.clone());
            if let Err(e) = self.worker_manager.stop_by_worker_id(&worker_id).await {
                error!(worker_id = row.worker_id, error = %e, "local worker stop failed; row left stopping");
            }
        }

        // Provisioning rows count: builderd registers before the row runs.
        let wanted: HashSet<&str> = rows.iter().map(|w| w.worker_id.as_str()).collect();
        for worker_id in listed.keys().filter(|id| !wanted.contains(id.as_str())) {
            self.stop_unwanted(worker_id).await;
        }
        Ok(())
    }

    /// Stop a listed worker no active row claims, unless a row appeared
    /// since the pass read them (a launch in progress).
    async fn stop_unwanted(&self, worker_id: &str) {
        match self.worker_repo.get_worker(worker_id).await {
            Ok(Some(row)) if row.container_status != STOPPED => return,
            Ok(_) => {}
            Err(e) => {
                error!(worker_id, error = %e, "cannot read worker row; leaving local worker");
                return;
            }
        }
        warn!(
            worker_id,
            "builderd runs a local worker with no active row; stopping it"
        );
        if let Err(status) = self.client.stop_local_worker(worker_id).await {
            error!(worker_id, error = %status, "stopping unwanted local worker failed");
        }
    }
}
