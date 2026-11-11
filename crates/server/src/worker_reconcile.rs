//! Worker reconciliation against builderd (ur-j5puq, ur-ezhdb).
//!
//! The database says what should exist; builderd lists what does, by worker
//! id (local: `ListLocalWorkers`; containers: `ListWorkerContainers`, by the
//! `ur.worker_id` label, scoped to this instance). One table converges both
//! runtimes:
//!
//! | DB row | builderd list | action |
//! |---|---|---|
//! | `running` | listed alive | none |
//! | `running` | absent or dead | stop path |
//! | `stopping` | any | stop path (retry) |
//! | `provisioning` | any | periodic: none · startup: → `stopping` |
//! | no active row (none / `stopped`) | listed | builderd Stop(worker_id) |
//! | — | list failed | no change for that runtime this pass |
//!
//! The stop path (`WorkerManager::stop_by_worker_id`) marks the row
//! `stopped` and unlinks its slot only after builderd's Stop(worker_id)
//! succeeds, and leaves it `stopping` otherwise.
//!
//! Rows are read before the lists: a row that was `running` then was already
//! launched, so its absence afterwards means it is gone. An unknown listed id
//! is re-read before it is stopped, because a launch records its row before
//! builderd starts the worker.
//!
//! A stop path that begins while the row's own Launch RPC is unresolved
//! (launch pending) leaves it `stopping` with its slot linked; a stop begun
//! after the launch resolves finishes it.
//!
//! [`WorkerReconcileManager::startup`] runs before the server serves or
//! dispatches, so no launch is in flight: it clears every launch-pending
//! marker (a dead server cannot deliver a Launch), and a `provisioning` row then is a
//! launch the previous server never finished. It becomes `stopping` (its slot
//! link, inserted with the row, keeps the slot out of the pool) and the pass
//! that follows stops it. While the server runs, a stop of a `provisioning`
//! row goes through builderd's Stop barrier (it waits for the in-flight
//! launch), so periodic passes leave `provisioning` rows alone.
//!
//! Known limits (upgrade): a local `provisioning` row written by a
//! pre-ur-j5puq build has no slot link, so its slot is free while startup's
//! stop converges it. A container launched before ur-ezhdb has a process-id
//! name and no labels, so it is never listed: its `running` row is left alone
//! here, and only the row-based stop path (which names its Docker ID to
//! builderd) stops it.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{error, info, warn};
use ur_config::WorkerRuntime;
use ur_rpc::proto::builder_container::StopWorkerRequest;
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

/// Converges worker rows (both runtimes) on what builderd lists.
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
    /// `Err` only when stale `provisioning` rows cannot be marked (DB
    /// errors); a failed builderd list is logged and left to the loop.
    pub async fn startup(&self) -> Result<(), String> {
        // A dead server can no longer deliver a Launch: its pending markers
        // go, so this pass finishes those workers' stops.
        let pending = self
            .worker_repo
            .clear_all_launch_pending()
            .await
            .map_err(|e| format!("clear launch markers: {e}"))?;
        if !pending.is_empty() {
            warn!(
                ?pending,
                "launches a previous server left pending; stops can finish"
            );
        }
        let stale = self
            .worker_repo
            .list_workers_by_container_status("provisioning")
            .await
            .map_err(|e| format!("list provisioning workers: {e}"))?;
        for row in &stale {
            warn!(
                worker_id = row.worker_id,
                "unfinished launch from a previous server; stopping it"
            );
            self.worker_repo
                .begin_stopping(&row.worker_id)
                .await
                .map_err(|e| format!("mark stale launch stopping: {e}"))?;
        }
        if let Err(e) = self.reconcile_once().await {
            warn!(error = %e, "startup worker reconcile incomplete; the loop retries");
        }
        Ok(())
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
                warn!(error = %e, "worker reconcile pass incomplete");
            }
        }
    }

    /// One pass over both runtimes. `Err` when the rows cannot be read
    /// (nothing changed) or a builderd list failed (that runtime unchanged);
    /// per-worker stop failures are logged and left on the row as `stopping`.
    pub async fn reconcile_once(&self) -> Result<(), String> {
        let (local, container): (Vec<_>, Vec<_>) = self
            .worker_repo
            .list_active_workers()
            .await
            .map_err(|e| format!("list active workers: {e}"))?
            .into_iter()
            .partition(|w| w.runtime == WorkerRuntime::Local);
        let local_listed = self
            .client
            .list_local_workers()
            .await
            .map(|l| {
                l.into_iter()
                    .map(|w| (w.worker_id, w.session_alive))
                    .collect()
            })
            .map_err(|status| format!("ListLocalWorkers: {status}"));
        let container_listed = self
            .container_client
            .list_worker_containers()
            .await
            .map(|l| l.into_iter().map(|c| (c.worker_id, c.running)).collect())
            .map_err(|status| format!("ListWorkerContainers: {status}"));
        let errors: Vec<String> = [
            self.converge(WorkerRuntime::Local, &local, local_listed)
                .await,
            self.converge(WorkerRuntime::Container, &container, container_listed)
                .await,
        ]
        .into_iter()
        .filter_map(Result::err)
        .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// The table (module docs) for one runtime's active `rows`.
    async fn converge(
        &self,
        runtime: WorkerRuntime,
        rows: &[Worker],
        listed: Result<HashMap<String, bool>, String>,
    ) -> Result<(), String> {
        let listed = listed?;
        for row in rows {
            // A pre-ur-ezhdb container is never listed (no labels).
            let unlisted = self.worker_manager.is_legacy_container(row);
            let why = match (row.container_status.as_str(), listed.get(&row.worker_id)) {
                (STOPPING, _) => "retrying stop",
                (RUNNING, _) if unlisted => continue,
                (RUNNING, Some(true)) => continue,
                (RUNNING, None) => "gone from builderd",
                (RUNNING, Some(false)) => "not alive",
                _ => continue,
            };
            info!(
                worker_id = row.worker_id,
                why,
                runtime = runtime.as_str(),
                "stopping worker"
            );
            let worker_id = WorkerId(row.worker_id.clone());
            if let Err(e) = self.worker_manager.stop_by_worker_id(&worker_id).await {
                error!(worker_id = row.worker_id, error = %e, "worker stop failed; row left stopping");
            }
        }

        // Provisioning rows count: builderd starts a worker before its row runs.
        let wanted: HashSet<&str> = rows.iter().map(|w| w.worker_id.as_str()).collect();
        for worker_id in listed.keys().filter(|id| !wanted.contains(id.as_str())) {
            self.stop_unwanted(runtime, worker_id).await;
        }
        Ok(())
    }

    /// Stop a listed worker no active row claims, unless a row appeared
    /// since the pass read them (a launch in progress).
    async fn stop_unwanted(&self, runtime: WorkerRuntime, worker_id: &str) {
        match self.worker_repo.get_worker(worker_id).await {
            Ok(Some(row)) if row.container_status != STOPPED => return,
            Ok(_) => {}
            Err(e) => {
                error!(worker_id, error = %e, "cannot read worker row; leaving worker");
                return;
            }
        }
        warn!(
            worker_id,
            runtime = runtime.as_str(),
            "builderd runs a worker with no active row; stopping it"
        );
        let stopped = match runtime {
            WorkerRuntime::Local => self.client.stop_local_worker(worker_id).await,
            WorkerRuntime::Container => self
                .container_client
                .stop_worker(StopWorkerRequest {
                    worker_id: worker_id.to_owned(),
                    legacy_container_id: String::new(),
                })
                .await
                .map(|_| ()),
        };
        if let Err(status) = stopped {
            error!(worker_id, error = %status, "stopping unwanted worker failed");
        }
    }
}
