//! Worker reconciliation against builderd (ur-j5puq, ur-ezhdb).
//!
//! **Server startup** ([`WorkerReconcileManager::startup`], before the server
//! serves or dispatches, so no launch or stop is in flight):
//!
//! 1. Container rows: `WorkerRepo::reconcile_workers` probes containers via
//!    builderd. It never moves a row to `running`: `provisioning` →
//!    `stopping`, `stopping` left alone, `running` + dead → `stopped` (slot
//!    unlinked), `stopped` + alive → that container is stopped through
//!    builderd's Stop for the row's worker id, unless a live row owns it now
//!    (container names are reused by a slot's next worker). The row stays
//!    `stopped`. A failed stray stop fails startup: that container's slot is
//!    already free, so serving would let the next acquire reset it.
//! 2. Local `provisioning` rows (a launch the previous server never
//!    finished; its agent may be live) → `stopping`. Their slot link,
//!    inserted with the row at launch start, keeps the slot out of the pool.
//!    Known limit: a `provisioning` row written by a pre-ur-j5puq build has
//!    no slot link (that build linked at finalize), so its slot is free
//!    while the stop converges it.
//! 3. One periodic pass.
//!
//! **Periodic pass** ([`WorkerReconcileManager::reconcile_once`], every 15s):
//! container rows in `stopping` are retried via `stop_by_worker_id`. Local
//! rows are converged on builderd's `ListLocalWorkers` (a local worker's tmux
//! session is alive only while builderd lists it):
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
//! `provisioning` rows of either runtime are never touched by a periodic
//! pass: a stop of one goes through builderd's Stop barrier (it waits for
//! the in-flight launch). The stop path (`WorkerManager::stop_by_worker_id`)
//! releases the slot and marks the row `stopped` only after builderd's Stop
//! succeeds, and leaves it `stopping` otherwise. A failed local list changes
//! no local row.
//!
//! Rows are read before the list: a row that was `running` then was already
//! registered, so its absence afterwards means builderd has torn it down. An
//! unknown listed id is re-read before it is stopped, because a launch
//! records its row before builderd registers the worker.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::sync::watch;
use tonic::Code;
use tracing::{error, info, warn};
use ur_rpc::proto::builder_container::{InspectWorkerRequest, StopWorkerRequest};
use workflow_db::WorkerRepo;
use workflow_db::model::Worker;

use crate::builder_container_client::BuilderContainerClient;
use crate::builder_local_worker_client::BuilderLocalWorkerClient;
use crate::worker::{WorkerId, WorkerManager};

/// How often the loop runs after its startup pass.
pub const DEFAULT_RECONCILE_INTERVAL: Duration = Duration::from_secs(15);

const RUNNING: &str = "running";
const STOPPING: &str = "stopping";
const STOPPED: &str = "stopped";

/// Converges worker rows on builderd's live containers and local workers.
#[derive(Clone)]
pub struct WorkerReconcileManager {
    worker_manager: WorkerManager,
    worker_repo: WorkerRepo,
    client: BuilderLocalWorkerClient,
    container_client: BuilderContainerClient,
    interval: Duration,
}

impl WorkerReconcileManager {
    pub fn new(
        worker_manager: WorkerManager,
        worker_repo: WorkerRepo,
        client: BuilderLocalWorkerClient,
        container_client: BuilderContainerClient,
        interval: Duration,
    ) -> Self {
        Self {
            worker_manager,
            worker_repo,
            client,
            container_client,
            interval,
        }
    }

    /// The startup pass. Must finish before the server serves or dispatches.
    /// `Err` on DB errors or a failed stray container stop; a failed
    /// builderd list is logged and left to the loop.
    pub async fn startup(&self) -> Result<(), String> {
        self.reconcile_containers().await?;
        let stale: Vec<_> = self
            .worker_repo
            .list_workers_by_container_status("provisioning")
            .await
            .map_err(|e| format!("list provisioning workers: {e}"))?
            .into_iter()
            .filter(|w| w.runtime == ur_config::WorkerRuntime::Local)
            .collect();
        for row in &stale {
            warn!(
                worker_id = row.worker_id,
                "unfinished local launch from a previous server; stopping it"
            );
            self.worker_repo
                .begin_stopping(&row.worker_id)
                .await
                .map_err(|e| format!("mark stale launch stopping: {e}"))?;
        }
        if let Err(e) = self.reconcile_once().await {
            warn!(error = %e, "startup worker reconcile skipped; the loop retries");
        }
        Ok(())
    }

    /// Startup step 1 (module docs): container rows against builderd.
    async fn reconcile_containers(&self) -> Result<(), String> {
        let result = self
            .worker_repo
            .reconcile_workers(|container_id| {
                let client = self.container_client.clone();
                async move {
                    client
                        .inspect_worker(InspectWorkerRequest {
                            container_id: container_id.clone(),
                        })
                        .await
                        .map(|response| response.running)
                        .map_err(|status| format!("{container_id}: {status}"))
                }
            })
            .await
            .map_err(|e| format!("worker reconciliation failed: {e}"))?;
        if !result.indeterminate.is_empty() {
            warn!(
                indeterminate = ?result.indeterminate,
                "worker liveness could not be determined; rows left unchanged"
            );
        }
        info!(
            reclaimed = ?result.reclaimed,
            stopped = ?result.marked_stopped,
            stopping = ?result.marked_stopping,
            stray = result.stray.len(),
            "container worker reconciliation complete"
        );
        let mut failures = Vec::new();
        for worker in &result.stray {
            if let Err(e) = self.stop_stray_container(worker).await {
                error!(worker_id = worker.worker_id, error = %e, "stopping a stopped worker's live container failed");
                failures.push(format!("{}: {e}", worker.worker_id));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "live containers of stopped workers (their slots are free) could not be stopped: {}",
                failures.join("; ")
            ))
        }
    }

    /// Stop the live container of a `stopped` row, unless another active row
    /// owns that container now. The row is not changed.
    async fn stop_stray_container(&self, worker: &Worker) -> Result<(), String> {
        let state = self
            .container_client
            .inspect_worker(InspectWorkerRequest {
                container_id: worker.container_id.clone(),
            })
            .await
            .map_err(|status| format!("inspect {}: {status}", worker.container_id))?;
        if !state.running {
            return Ok(());
        }
        let owner = self
            .worker_repo
            .get_worker_by_container_id(&state.container_id)
            .await
            .map_err(|e| format!("db error: {e}"))?;
        if let Some(owner) = owner
            && owner.worker_id != worker.worker_id
            && owner.container_status != STOPPED
        {
            info!(
                worker_id = worker.worker_id,
                owner = owner.worker_id,
                "container now belongs to another worker; leaving it"
            );
            return Ok(());
        }
        warn!(
            worker_id = worker.worker_id,
            container_id = state.container_id,
            "stopped worker's container is alive; stopping it"
        );
        match self
            .container_client
            .stop_worker(StopWorkerRequest {
                container_id: state.container_id,
                worker_id: worker.worker_id.clone(),
            })
            .await
        {
            Ok(_) => Ok(()),
            Err(status) if status.code() == Code::NotFound => Ok(()),
            Err(status) => Err(status.to_string()),
        }
    }

    /// Run a pass every `interval`, starting one interval after
    /// [`Self::startup`].
    pub fn spawn(self, shutdown_rx: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(self.run(shutdown_rx))
    }

    async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        info!(interval = ?self.interval, "worker reconciliation started");
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("worker reconciliation shutting down");
                        return;
                    }
                }
            }
            if let Err(e) = self.reconcile_once().await {
                warn!(error = %e, "worker reconcile pass skipped; nothing changed");
            }
        }
    }

    /// One pass. Container `stopping` rows are retried first. `Err` when the
    /// rows cannot be read (nothing changed) or builderd's local list cannot
    /// be read (no local row changed); per-worker stop failures are logged
    /// and left on the row as `stopping` for the next pass.
    pub async fn reconcile_once(&self) -> Result<(), String> {
        let (local, container): (Vec<_>, Vec<_>) = self
            .worker_repo
            .list_active_workers()
            .await
            .map_err(|e| format!("list active workers: {e}"))?
            .into_iter()
            .partition(|w| w.runtime == ur_config::WorkerRuntime::Local);
        for row in container.iter().filter(|w| w.container_status == STOPPING) {
            self.stop_row(row, "retrying container stop").await;
        }
        self.reconcile_local(&local).await
    }

    /// `stop_by_worker_id`, logging a failure (the row stays `stopping`).
    async fn stop_row(&self, row: &Worker, why: &str) {
        info!(worker_id = row.worker_id, why, "stopping worker");
        let worker_id = WorkerId(row.worker_id.clone());
        if let Err(e) = self.worker_manager.stop_by_worker_id(&worker_id).await {
            error!(worker_id = row.worker_id, error = %e, "worker stop failed; row left stopping");
        }
    }

    /// The local half of a pass (module docs table).
    async fn reconcile_local(&self, rows: &[Worker]) -> Result<(), String> {
        let listed: HashMap<String, bool> = self
            .client
            .list_local_workers()
            .await
            .map_err(|status| format!("ListLocalWorkers: {status}"))?
            .into_iter()
            .map(|w| (w.worker_id, w.session_alive))
            .collect();

        for row in rows {
            let why = match (row.container_status.as_str(), listed.get(&row.worker_id)) {
                (RUNNING, Some(true)) => continue,
                (RUNNING, None) => "gone from builderd",
                (RUNNING, Some(false)) => "session dead",
                (STOPPING, _) => "retrying stop",
                _ => continue,
            };
            self.stop_row(row, why).await;
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
