// WorkerRepo: CRUD operations for worker and slot tables, plus startup reconciliation.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::model::{AgentStatus, Slot, Worker, WorkerRuntime, WorkerSlot};

/// Result of slot reconciliation: reports what was cleaned up or discovered.
pub struct SlotReconcileResult {
    /// Slot IDs that were deleted because their host_path no longer exists on disk.
    pub deleted_stale: Vec<String>,
    /// Slot IDs that were inserted because an on-disk directory had no DB row.
    pub inserted_orphaned: Vec<String>,
}

/// Result of worker reconciliation: reports what was reclaimed or marked dead.
pub struct WorkerReconcileResult {
    /// Worker IDs whose containers are still alive (kept as running).
    pub reclaimed: Vec<String>,
    /// Worker IDs whose containers are dead (marked stopped, slots released).
    pub marked_stopped: Vec<String>,
    /// Worker IDs whose liveness could not be determined, left untouched.
    pub indeterminate: Vec<String>,
}

#[derive(Clone)]
pub struct WorkerRepo {
    pool: PgPool,
}

/// Column tuple type returned by worker SELECT queries.
type WorkerRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    i32,
    Option<String>,
    String,
);

/// Convert a SELECTed row into a [`Worker`]. Fails (rather than defaulting) if
/// `runtime` holds a value the CHECK constraint should have made impossible.
fn worker_from_row(row: WorkerRow) -> Result<Worker, sqlx::Error> {
    let runtime = row
        .14
        .parse::<WorkerRuntime>()
        .map_err(|e| sqlx::Error::Decode(e.into()))?;
    Ok(Worker {
        worker_id: row.0,
        process_id: row.1,
        project_key: row.2,
        container_id: row.3,
        worker_secret: row.4,
        strategy: row.5,
        agent_type: row.6,
        container_status: row.7,
        agent_status: row.8,
        workspace_path: row.9,
        created_at: row.10,
        updated_at: row.11,
        idle_redispatch_count: row.12,
        brain_path: row.13,
        runtime,
    })
}

fn slot_from_row(
    (id, project_key, slot_name, host_path, created_at, updated_at): (
        String,
        String,
        String,
        String,
        String,
        String,
    ),
) -> Slot {
    Slot {
        id,
        project_key,
        slot_name,
        host_path,
        created_at,
        updated_at,
    }
}

impl WorkerRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // --- Worker methods ---

    pub async fn insert_worker(&self, worker: &Worker) -> Result<(), sqlx::Error> {
        insert_worker_row(worker).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn get_worker(&self, worker_id: &str) -> Result<Option<Worker>, sqlx::Error> {
        let row = sqlx::query_as::<_, WorkerRow>(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, agent_type, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count, brain_path, runtime
             FROM worker WHERE worker_id = $1",
        )
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(worker_from_row).transpose()
    }

    /// Look up a worker by the container it owns.
    ///
    /// Container IDs are unique per launch, unlike the slot-derived container
    /// name, so this is the safe way to ask "which worker does this live
    /// container belong to?" before acting on it.
    pub async fn get_worker_by_container_id(
        &self,
        container_id: &str,
    ) -> Result<Option<Worker>, sqlx::Error> {
        let row = sqlx::query_as::<_, WorkerRow>(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, agent_type, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count, brain_path, runtime
             FROM worker WHERE container_id = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(container_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(worker_from_row).transpose()
    }

    pub async fn update_worker_container_status(
        &self,
        worker_id: &str,
        container_status: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE worker SET container_status = $1, updated_at = $2 WHERE worker_id = $3",
        )
        .bind(container_status)
        .bind(&now)
        .bind(worker_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Claim `slot_id` for a new `provisioning` worker: insert the row and
    /// its `worker_slot` link in one transaction. `UNIQUE (slot_id)` makes
    /// the insert the arbiter: `false` means another worker owns the slot
    /// (nothing is written). A link left by a `stopped` worker owns nothing
    /// and is dropped first.
    pub async fn claim_slot(&self, worker: &Worker, slot_id: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM worker_slot WHERE slot_id = $1 AND worker_id IN
               (SELECT worker_id FROM worker WHERE container_status = 'stopped')",
        )
        .bind(slot_id)
        .execute(&mut *tx)
        .await?;
        insert_worker_row(worker).execute(&mut *tx).await?;
        let linked = sqlx::query(
            "INSERT INTO worker_slot (worker_id, slot_id, created_at) VALUES ($1, $2, $3)",
        )
        .bind(&worker.worker_id)
        .bind(slot_id)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await;
        match linked {
            Ok(_) => {
                tx.commit().await?;
                Ok(true)
            }
            Err(sqlx::Error::Database(e)) if e.code().is_some_and(|c| c == "23505") => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Refresh a claimed `provisioning` row with the launch's final details
    /// (container name, secret, ...). `false` when it is no longer
    /// `provisioning` (stopped mid-launch) or gone.
    pub async fn update_provisioning_worker(&self, worker: &Worker) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE worker SET process_id = $2, project_key = $3, container_id = $4,
               worker_secret = $5, strategy = $6, agent_type = $7, workspace_path = $8,
               updated_at = $9, brain_path = $10, runtime = $11
             WHERE worker_id = $1 AND container_status = 'provisioning'",
        )
        .bind(&worker.worker_id)
        .bind(&worker.process_id)
        .bind(&worker.project_key)
        .bind(&worker.container_id)
        .bind(&worker.worker_secret)
        .bind(&worker.strategy)
        .bind(&worker.agent_type)
        .bind(&worker.workspace_path)
        .bind(&worker.updated_at)
        .bind(&worker.brain_path)
        .bind(worker.runtime.as_str())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Move an active (`provisioning`/`running`/`stopping`) worker to
    /// `stopping`, returning the status it had. `None` when the row is gone or
    /// already `stopped`, so a stale stopper can tell it has nothing to stop.
    pub async fn begin_stopping(&self, worker_id: &str) -> Result<Option<String>, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query_scalar::<_, String>(
            "UPDATE worker w SET container_status = 'stopping', updated_at = $1
             FROM (SELECT container_status FROM worker WHERE worker_id = $2 FOR UPDATE) old
             WHERE w.worker_id = $2
               AND old.container_status IN ('provisioning', 'running', 'stopping')
             RETURNING old.container_status",
        )
        .bind(&now)
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await
    }

    /// `stopping → stopped` and delete the worker's slot link, together.
    /// Called only once the worker is confirmed gone. `false` (nothing
    /// changed) when the row is not `stopping`.
    pub async fn finish_stop(&self, worker_id: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let stopped = sqlx::query(
            "UPDATE worker SET container_status = 'stopped', updated_at = $1
             WHERE worker_id = $2 AND container_status = 'stopping'",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(worker_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
        if stopped {
            sqlx::query("DELETE FROM worker_slot WHERE worker_id = $1")
                .bind(worker_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(stopped)
    }

    /// `provisioning → running`, recording Docker's immutable ID (or the
    /// local session name). `false` when the row is no longer
    /// `provisioning`: it was stopped mid-launch, and the launch path must
    /// stop the worker itself.
    pub async fn finalize_worker_launch(
        &self,
        worker_id: &str,
        container_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE worker SET container_id = $1, container_status = 'running', updated_at = $2
             WHERE worker_id = $3 AND container_status = 'provisioning'",
        )
        .bind(container_id)
        .bind(&now)
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete one worker row. Associated slot links are cascade-deleted.
    pub async fn delete_worker(&self, worker_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM worker WHERE worker_id = $1")
            .bind(worker_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn update_worker_agent_status(
        &self,
        worker_id: &str,
        agent_status: AgentStatus,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();

        sqlx::query("UPDATE worker SET agent_status = $1, updated_at = $2 WHERE worker_id = $3")
            .bind(agent_status.as_str())
            .bind(&now)
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Increment the idle redispatch count for a worker and return the new value.
    pub async fn increment_idle_redispatch_count(
        &self,
        worker_id: &str,
    ) -> Result<i32, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE worker SET idle_redispatch_count = idle_redispatch_count + 1, updated_at = $1 WHERE worker_id = $2",
        )
        .bind(&now)
        .bind(worker_id)
        .execute(&self.pool)
        .await?;

        let count = sqlx::query_scalar::<_, i32>(
            "SELECT idle_redispatch_count FROM worker WHERE worker_id = $1",
        )
        .bind(worker_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(count)
    }

    pub async fn list_workers_by_container_status(
        &self,
        container_status: &str,
    ) -> Result<Vec<Worker>, sqlx::Error> {
        let rows = sqlx::query_as::<_, WorkerRow>(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, agent_type, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count, brain_path, runtime
             FROM worker WHERE container_status = $1 ORDER BY created_at ASC",
        )
        .bind(container_status)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(worker_from_row).collect()
    }

    pub async fn verify_worker(&self, worker_id: &str, secret: &str) -> Result<bool, sqlx::Error> {
        let count = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*)::INT4 FROM worker WHERE worker_id = $1 AND worker_secret = $2",
        )
        .bind(worker_id)
        .bind(secret)
        .fetch_one(&self.pool)
        .await?;

        Ok(count > 0)
    }

    pub async fn get_worker_context(
        &self,
        project_key: &str,
        workspace_path: &str,
    ) -> Result<Option<Worker>, sqlx::Error> {
        let row = sqlx::query_as::<_, WorkerRow>(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, agent_type, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count, brain_path, runtime
             FROM worker WHERE project_key = $1 AND workspace_path = $2",
        )
        .bind(project_key)
        .bind(workspace_path)
        .fetch_optional(&self.pool)
        .await?;

        row.map(worker_from_row).transpose()
    }

    // --- Slot methods ---

    pub async fn insert_slot(&self, slot: &Slot) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO slot (id, project_key, slot_name, host_path, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&slot.id)
        .bind(&slot.project_key)
        .bind(&slot.slot_name)
        .bind(&slot.host_path)
        .bind(&slot.created_at)
        .bind(&slot.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_slot(&self, id: &str) -> Result<Option<Slot>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT id, project_key, slot_name, host_path, created_at, updated_at
             FROM slot WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(slot_from_row))
    }

    pub async fn get_slot_by_host_path(
        &self,
        host_path: &str,
    ) -> Result<Option<Slot>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT id, project_key, slot_name, host_path, created_at, updated_at
             FROM slot WHERE host_path = $1",
        )
        .bind(host_path)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(slot_from_row))
    }

    pub async fn list_slots_by_project(&self, project_key: &str) -> Result<Vec<Slot>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT id, project_key, slot_name, host_path, created_at, updated_at
             FROM slot WHERE project_key = $1 ORDER BY created_at ASC",
        )
        .bind(project_key)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(slot_from_row).collect())
    }

    /// List every available exclusive slot for a project (not linked to an active worker),
    /// oldest first.
    ///
    /// Only returns slots with numeric names (exclusive pool slots like "0", "1", "2").
    /// Shared slots (name = "shared") are excluded — they are managed separately by
    /// `acquire_shared_slot` and should never be assigned to code workers. Non-numeric
    /// names are excluded too: dotfile directories such as `.claude` live alongside the
    /// slot directories in the pool root and must never be handed to a worker.
    ///
    /// All candidates are returned rather than just the first so callers can skip slots
    /// that are mid-acquisition instead of giving up on the pool entirely.
    pub async fn find_available_slots(&self, project_key: &str) -> Result<Vec<Slot>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT s.id, s.project_key, s.slot_name, s.host_path, s.created_at, s.updated_at
             FROM slot s
             WHERE s.project_key = $1
               AND s.slot_name ~ '^[0-9]+$'
               AND s.id NOT IN (
                 SELECT ws.slot_id FROM worker_slot ws
                 INNER JOIN worker w ON w.worker_id = ws.worker_id
                 WHERE w.container_status IN ('provisioning', 'running', 'stopping')
               )
             ORDER BY s.created_at ASC",
        )
        .bind(project_key)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(slot_from_row).collect())
    }

    /// Count slots that have an active worker linked via worker_slot.
    pub async fn slots_in_use(&self, project_key: &str) -> Result<i32, sqlx::Error> {
        let count = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*)::INT4 FROM slot s
             INNER JOIN worker_slot ws ON ws.slot_id = s.id
             INNER JOIN worker w ON w.worker_id = ws.worker_id
             WHERE s.project_key = $1
               AND w.container_status IN ('provisioning', 'running', 'stopping')",
        )
        .bind(project_key)
        .fetch_one(&self.pool)
        .await?;

        Ok(count)
    }

    pub async fn delete_slot(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM slot WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // --- Worker-Slot link methods ---

    /// Link a worker to a slot via the worker_slot join table.
    pub async fn link_worker_slot(
        &self,
        worker_id: &str,
        slot_id: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO worker_slot (worker_id, slot_id, created_at) VALUES ($1, $2, $3)")
            .bind(worker_id)
            .bind(slot_id)
            .bind(&now)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Unlink a worker from its slot by removing the worker_slot row.
    pub async fn unlink_worker_slot(&self, worker_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM worker_slot WHERE worker_id = $1")
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Get the worker_slot link for a given worker, if any.
    pub async fn get_worker_slot(
        &self,
        worker_id: &str,
    ) -> Result<Option<WorkerSlot>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, String)>(
            "SELECT worker_id, slot_id, created_at FROM worker_slot WHERE worker_id = $1",
        )
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(worker_id, slot_id, created_at)| WorkerSlot {
            worker_id,
            slot_id,
            created_at,
        }))
    }

    // --- Reconciliation helpers ---

    /// List all slots across all projects.
    pub async fn list_all_slots(&self) -> Result<Vec<Slot>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT id, project_key, slot_name, host_path, created_at, updated_at
             FROM slot ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(slot_from_row).collect())
    }

    /// List all workers regardless of container_status.
    pub async fn list_all_workers(&self) -> Result<Vec<Worker>, sqlx::Error> {
        let rows = sqlx::query_as::<_, WorkerRow>(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, agent_type, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count, brain_path, runtime
             FROM worker ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(worker_from_row).collect()
    }

    /// List workers whose container_status is one of the active lifecycle states
    /// (provisioning, running, stopping).
    pub async fn list_active_workers(&self) -> Result<Vec<Worker>, sqlx::Error> {
        let rows = sqlx::query_as::<_, WorkerRow>(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, agent_type, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count, brain_path, runtime
             FROM worker WHERE container_status IN ('provisioning', 'running', 'stopping') ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(worker_from_row).collect()
    }

    /// Delete all workers that are linked to a given slot_id via worker_slot.
    pub async fn delete_workers_by_slot_id(&self, slot_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM worker WHERE worker_id IN (SELECT worker_id FROM worker_slot WHERE slot_id = $1)",
        )
        .bind(slot_id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    // --- Reconciliation methods ---

    /// Reconcile slot DB rows with on-disk directories.
    ///
    /// `project_configs` maps project_key to the pool directory path for that project
    /// (e.g., `{local_workspace}/pool/{project_key}`).
    ///
    /// `local_workspace` is the container-local workspace root used to construct the
    /// pool root (`{local_workspace}/pool/`).
    ///
    /// For each project:
    /// - Slot in DB but directory missing on disk: delete the slot row (and associated worker rows).
    /// - Directory on disk but no slot in DB: insert a new slot row.
    pub async fn reconcile_slots(
        &self,
        project_configs: &HashMap<String, std::path::PathBuf>,
        local_workspace: &Path,
        host_workspace: &Path,
    ) -> Result<SlotReconcileResult, sqlx::Error> {
        let mut result = SlotReconcileResult {
            deleted_stale: Vec::new(),
            inserted_orphaned: Vec::new(),
        };

        let all_slots = self.list_all_slots().await?;

        // Index DB slots by (project_key, slot_name) for fast lookup.
        let mut db_slots_by_project: HashMap<String, HashMap<String, Slot>> = HashMap::new();
        for slot in all_slots {
            db_slots_by_project
                .entry(slot.project_key.clone())
                .or_default()
                .insert(slot.slot_name.clone(), slot);
        }

        let local_pool_root = local_workspace.join("pool");
        let host_pool_root = host_workspace.join("pool");

        for project_key in project_configs.keys() {
            let local_pool_dir = local_pool_root.join(project_key);
            let host_pool_dir = host_pool_root.join(project_key);
            let disk_slot_names = scan_disk_slots(&local_pool_dir).await;

            let db_slots = db_slots_by_project.remove(project_key).unwrap_or_default();

            self.delete_stale_slots(&db_slots, &disk_slot_names, &mut result)
                .await?;
            self.insert_orphaned_slots(
                project_key,
                &host_pool_dir,
                &db_slots,
                &disk_slot_names,
                &mut result,
            )
            .await?;
        }

        // Handle slots in DB for projects not in project_configs (stale project).
        for db_slots in db_slots_by_project.into_values() {
            self.delete_stale_slots(&db_slots, &HashSet::new(), &mut result)
                .await?;
        }

        Ok(result)
    }

    /// Delete slot rows that exist in DB but not on disk, along with their workers.
    async fn delete_stale_slots(
        &self,
        db_slots: &HashMap<String, Slot>,
        disk_slot_names: &HashSet<String>,
        result: &mut SlotReconcileResult,
    ) -> Result<(), sqlx::Error> {
        for (slot_name, slot) in db_slots {
            if disk_slot_names.contains(slot_name) {
                continue;
            }
            self.delete_workers_by_slot_id(&slot.id).await?;
            self.delete_slot(&slot.id).await?;
            result.deleted_stale.push(slot.id.clone());
        }
        Ok(())
    }

    /// Insert slot rows for on-disk directories that have no DB row.
    async fn insert_orphaned_slots(
        &self,
        project_key: &str,
        project_pool_dir: &Path,
        db_slots: &HashMap<String, Slot>,
        disk_slot_names: &HashSet<String>,
        result: &mut SlotReconcileResult,
    ) -> Result<(), sqlx::Error> {
        for slot_name in disk_slot_names {
            if db_slots.contains_key(slot_name) {
                continue;
            }
            let now = Utc::now().to_rfc3339();
            let host_path = project_pool_dir.join(slot_name);
            let slot = Slot {
                id: Uuid::new_v4().to_string(),
                project_key: project_key.to_owned(),
                slot_name: slot_name.clone(),
                host_path: host_path.display().to_string(),
                created_at: now.clone(),
                updated_at: now,
            };
            let slot_id = slot.id.clone();
            self.insert_slot(&slot).await?;
            result.inserted_orphaned.push(slot_id);
        }
        Ok(())
    }

    /// Reconcile worker DB rows with live Docker containers.
    ///
    /// `is_container_alive` is an async function that takes a container_id string
    /// and returns whether the container is still running.
    ///
    /// Checks ALL workers (including stopped) against container liveness:
    /// - Active (provisioning/running/stopping) + alive: set container_status = "running" (reclaim).
    /// - Active (provisioning/running/stopping) + dead: set container_status = "stopped", unlink slot.
    /// - Stopped + alive: set container_status = "running" (reclaim). Does NOT modify agent_status.
    /// - Stopped + dead: no-op.
    ///
    /// `is_container_alive` returns `Err` when liveness cannot be determined
    /// (the container runtime is unreachable, for instance). Such a worker is
    /// left exactly as it is and reported in `indeterminate`: an unanswered
    /// probe must never be read as "dead", or a reconcile pass would mark
    /// every live worker stopped and release the slots they are still using.
    ///
    /// `runtime = local` rows have no container to probe and are skipped
    /// (left unchanged, not reported): the server's
    /// `LocalWorkerReconcileManager` reconciles them against builderd.
    pub async fn reconcile_workers<F, Fut>(
        &self,
        is_container_alive: F,
    ) -> Result<WorkerReconcileResult, sqlx::Error>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<bool, String>>,
    {
        let mut result = WorkerReconcileResult {
            reclaimed: Vec::new(),
            marked_stopped: Vec::new(),
            indeterminate: Vec::new(),
        };

        let all_workers = self.list_all_workers().await?;

        for worker in all_workers {
            if worker.runtime == WorkerRuntime::Local {
                continue;
            }
            match is_container_alive(worker.container_id.clone()).await {
                Ok(alive) => {
                    self.reconcile_single_worker(worker, alive, &mut result)
                        .await?;
                }
                Err(_) => result.indeterminate.push(worker.worker_id),
            }
        }

        Ok(result)
    }

    /// Delete workers where container_status='stopped' and updated_at is older than `ttl_days`.
    ///
    /// Returns the list of deleted worker IDs. Associated worker_slot rows are
    /// cascade-deleted via the foreign key constraint.
    pub async fn cleanup_stale_workers(&self, ttl_days: u64) -> Result<Vec<String>, sqlx::Error> {
        let interval = format!("{ttl_days} days");
        let rows = sqlx::query_as::<_, (String,)>(
            "DELETE FROM worker WHERE container_status = 'stopped' AND updated_at::TIMESTAMPTZ < (now() - $1::interval) RETURNING worker_id",
        )
        .bind(&interval)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Process a single worker during reconciliation.
    ///
    /// Behavior matrix:
    /// - Active + alive → reclaim (set container_status = "running")
    /// - Active + dead → mark stopped, unlink slot
    /// - Stopped + alive → reclaim (set container_status = "running", preserve agent_status)
    /// - Stopped + dead → no-op
    async fn reconcile_single_worker(
        &self,
        worker: Worker,
        alive: bool,
        result: &mut WorkerReconcileResult,
    ) -> Result<(), sqlx::Error> {
        let is_stopped = worker.container_status == "stopped";

        if is_stopped && !alive {
            // Stopped + dead: no-op.
            return Ok(());
        }

        if alive {
            // Active or stopped + alive: reclaim by setting container_status to "running".
            // Only update container_status — agent_status is preserved.
            self.update_worker_container_status(&worker.worker_id, "running")
                .await?;
            result.reclaimed.push(worker.worker_id);
        } else {
            // Active + dead: mark stopped and unlink slot.
            self.update_worker_container_status(&worker.worker_id, "stopped")
                .await?;
            self.unlink_worker_slot(&worker.worker_id).await?;
            result.marked_stopped.push(worker.worker_id);
        }
        Ok(())
    }
}

/// Scan a directory for subdirectory names, returning them as a set.
async fn scan_disk_slots(dir: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return names;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let is_dir = entry
            .file_type()
            .await
            .map(|ft| ft.is_dir())
            .unwrap_or(false);
        if is_dir && let Some(name) = entry.file_name().to_str() {
            // Skip "shared" — it is managed separately by acquire_shared_slot
            // and must never be treated as an exclusive pool slot.
            if name == "shared" {
                continue;
            }
            names.insert(name.to_owned());
        }
    }
    names
}

/// `INSERT` of a full worker row, shared by `insert_worker` and `claim_slot`.
fn insert_worker_row(
    worker: &Worker,
) -> sqlx::query::Query<'_, sqlx::Postgres, sqlx::postgres::PgArguments> {
    sqlx::query(
        "INSERT INTO worker (worker_id, process_id, project_key, container_id, worker_secret, strategy, agent_type, container_status, agent_status, workspace_path, created_at, updated_at, brain_path, runtime)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(&worker.worker_id)
    .bind(&worker.process_id)
    .bind(&worker.project_key)
    .bind(&worker.container_id)
    .bind(&worker.worker_secret)
    .bind(&worker.strategy)
    .bind(&worker.agent_type)
    .bind(&worker.container_status)
    .bind(&worker.agent_status)
    .bind(&worker.workspace_path)
    .bind(&worker.created_at)
    .bind(&worker.updated_at)
    .bind(&worker.brain_path)
    .bind(worker.runtime.as_str())
}
#[cfg(test)]
mod tests {
    use super::*;
    use ur_db_test::TestDb;

    fn test_worker(worker_id: &str, agent_type: &str) -> Worker {
        let now = Utc::now().to_rfc3339();
        Worker {
            worker_id: worker_id.to_owned(),
            process_id: format!("proc-{worker_id}"),
            project_key: "testproj".to_owned(),
            container_id: format!("container-{worker_id}"),
            worker_secret: "secret".to_owned(),
            strategy: "code".to_owned(),
            agent_type: agent_type.to_owned(),
            container_status: "running".to_owned(),
            agent_status: "starting".to_owned(),
            workspace_path: Some(String::new()),
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
            brain_path: None,
            runtime: WorkerRuntime::Container,
        }
    }

    async fn slot_row(repo: &WorkerRepo, id: &str) {
        let now = Utc::now().to_rfc3339();
        repo.insert_slot(&Slot {
            id: id.to_owned(),
            project_key: "testproj".to_owned(),
            slot_name: id.trim_start_matches("slot-").to_owned(),
            host_path: format!("/pool/testproj/{id}"),
            created_at: now.clone(),
            updated_at: now,
        })
        .await
        .unwrap();
    }

    fn provisioning(worker_id: &str) -> Worker {
        Worker {
            container_status: "provisioning".to_owned(),
            ..test_worker(worker_id, "claude")
        }
    }

    /// The slot claim is the row: of two concurrent claims of one slot
    /// exactly one wins, and the loser writes nothing.
    #[tokio::test]
    async fn concurrent_claims_of_one_slot_have_one_winner() {
        let test_db = TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());
        slot_row(&repo, "slot-0").await;
        let (a, b) = (provisioning("w-a"), provisioning("w-b"));
        let (ra, rb) = tokio::join!(repo.claim_slot(&a, "slot-0"), repo.claim_slot(&b, "slot-0"));
        let won = [ra.unwrap(), rb.unwrap()];
        assert_eq!(won.iter().filter(|w| **w).count(), 1, "{won:?}");
        let (winner, loser) = if won[0] {
            ("w-a", "w-b")
        } else {
            ("w-b", "w-a")
        };
        assert!(repo.get_worker_slot(winner).await.unwrap().is_some());
        assert!(
            repo.get_worker(loser).await.unwrap().is_none(),
            "loser's row rolled back"
        );
        assert!(
            repo.find_available_slots("testproj")
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A stopped worker's lingering link does not block the next claim.
    #[tokio::test]
    async fn claim_replaces_a_stopped_workers_link() {
        let test_db = TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());
        slot_row(&repo, "slot-0").await;
        assert!(
            repo.claim_slot(&provisioning("w-old"), "slot-0")
                .await
                .unwrap()
        );
        repo.update_worker_container_status("w-old", "stopped")
            .await
            .unwrap();
        assert!(
            repo.claim_slot(&provisioning("w-new"), "slot-0")
                .await
                .unwrap()
        );
        assert!(repo.get_worker_slot("w-old").await.unwrap().is_none());
    }

    /// Lifecycle transitions are compare-and-set.
    #[tokio::test]
    async fn lifecycle_transitions_are_conditional() {
        let test_db = TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());
        slot_row(&repo, "slot-0").await;
        assert!(
            repo.claim_slot(&provisioning("w1"), "slot-0")
                .await
                .unwrap()
        );

        assert_eq!(
            repo.begin_stopping("w1").await.unwrap().as_deref(),
            Some("provisioning")
        );
        assert!(
            !repo.finalize_worker_launch("w1", "c1").await.unwrap(),
            "stopped mid-launch"
        );
        assert_eq!(
            repo.get_worker("w1")
                .await
                .unwrap()
                .unwrap()
                .container_status,
            "stopping"
        );
        assert!(repo.finish_stop("w1").await.unwrap());
        assert!(repo.get_worker_slot("w1").await.unwrap().is_none());
        assert_eq!(
            repo.begin_stopping("w1").await.unwrap(),
            None,
            "stopped is final"
        );
        assert!(!repo.finish_stop("w1").await.unwrap());
        assert_eq!(repo.begin_stopping("nope").await.unwrap(), None);

        slot_row(&repo, "slot-1").await;
        assert!(
            repo.claim_slot(&provisioning("w2"), "slot-1")
                .await
                .unwrap()
        );
        assert!(!repo.finish_stop("w2").await.unwrap(), "only from stopping");
        assert!(repo.get_worker_slot("w2").await.unwrap().is_some());
        assert!(repo.finalize_worker_launch("w2", "c2").await.unwrap());
        assert_eq!(
            repo.begin_stopping("w2").await.unwrap().as_deref(),
            Some("running")
        );
    }

    /// Migration 005's rule: links of non-active workers go; a slot linked
    /// twice keeps the newest active worker's link.
    #[tokio::test]
    async fn one_owner_migration_resolves_stale_and_duplicate_links() {
        let test_db = TestDb::new().await;
        let pool = test_db.workflow_pool();
        let repo = WorkerRepo::new(pool.clone());
        sqlx::query("ALTER TABLE worker_slot DROP CONSTRAINT worker_slot_one_owner")
            .execute(pool)
            .await
            .unwrap();
        for slot in ["slot-0", "slot-1"] {
            slot_row(&repo, slot).await;
        }
        let at = |w: &str, created: &str, status: &str| Worker {
            created_at: created.to_owned(),
            container_status: status.to_owned(),
            ..test_worker(w, "claude")
        };
        for (w, created, status, slot) in [
            ("old", "2026-01-01T00:00:00Z", "running", "slot-0"),
            ("new", "2026-02-01T00:00:00Z", "running", "slot-0"),
            ("gone", "2026-03-01T00:00:00Z", "stopped", "slot-0"),
            ("only", "2026-01-01T00:00:00Z", "stopping", "slot-1"),
        ] {
            repo.insert_worker(&at(w, created, status)).await.unwrap();
            repo.link_worker_slot(w, slot).await.unwrap();
        }
        sqlx::raw_sql(include_str!(
            "../migrations/005_worker_slot_unique_slot.sql"
        ))
        .execute(pool)
        .await
        .unwrap();
        let owner = async |w: &str| repo.get_worker_slot(w).await.unwrap().map(|l| l.slot_id);
        assert_eq!(owner("new").await.as_deref(), Some("slot-0"));
        assert_eq!(owner("old").await, None);
        assert_eq!(owner("gone").await, None);
        assert_eq!(owner("only").await.as_deref(), Some("slot-1"));
    }

    #[tokio::test]
    async fn insert_and_get_round_trip_runtime() {
        let test_db = TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());

        let mut local = test_worker("w-local", "claude");
        local.runtime = WorkerRuntime::Local;
        repo.insert_worker(&local).await.unwrap();
        repo.insert_worker(&test_worker("w-container", "claude"))
            .await
            .unwrap();

        let fetched = repo.get_worker("w-local").await.unwrap().unwrap();
        assert_eq!(fetched.runtime, WorkerRuntime::Local);
        let fetched = repo.get_worker("w-container").await.unwrap().unwrap();
        assert_eq!(fetched.runtime, WorkerRuntime::Container);
        let all = repo.list_all_workers().await.unwrap();
        assert!(all.iter().any(|w| w.runtime == WorkerRuntime::Local));
    }

    #[tokio::test]
    async fn runtime_check_constraint_rejects_unknown_value() {
        let test_db = TestDb::new().await;
        repo_insert_raw(&test_db, "w-bad-runtime").await;
        let result = sqlx::query("UPDATE worker SET runtime = 'vm' WHERE worker_id = $1")
            .bind("w-bad-runtime")
            .execute(test_db.workflow_pool())
            .await;
        assert!(result.is_err(), "CHECK constraint must reject 'vm'");
    }

    /// Insert a row naming only the pre-migration columns, as any row written
    /// before `agent_type`/`runtime` existed would have been.
    async fn repo_insert_raw(test_db: &TestDb, worker_id: &str) {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO worker (worker_id, process_id, project_key, container_id, worker_secret, strategy, container_status, agent_status, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(worker_id)
        .bind(format!("proc-{worker_id}"))
        .bind("testproj")
        .bind(format!("container-{worker_id}"))
        .bind("secret")
        .bind("code")
        .bind("running")
        .bind("starting")
        .bind(&now)
        .bind(&now)
        .execute(test_db.workflow_pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn insert_and_get_round_trip_agent_type() {
        let test_db = TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());

        let worker = test_worker("w-agent-type", "claude");
        repo.insert_worker(&worker).await.unwrap();

        let fetched = repo.get_worker("w-agent-type").await.unwrap().unwrap();
        assert_eq!(fetched.agent_type, "claude");
    }

    #[tokio::test]
    async fn insert_and_get_round_trip_brain_path() {
        let test_db = TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());

        let mut with_brain = test_worker("w-brain", "claude");
        with_brain.brain_path = Some("/host/projects/p/brain".to_owned());
        repo.insert_worker(&with_brain).await.unwrap();
        repo.insert_worker(&test_worker("w-no-brain", "claude"))
            .await
            .unwrap();

        let fetched = repo.get_worker("w-brain").await.unwrap().unwrap();
        assert_eq!(
            fetched.brain_path.as_deref(),
            Some("/host/projects/p/brain")
        );
        let fetched = repo.get_worker("w-no-brain").await.unwrap().unwrap();
        assert_eq!(fetched.brain_path, None);
    }

    #[tokio::test]
    async fn finalize_and_delete_provisioning_worker() {
        let test_db = TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());

        let mut worker = test_worker("w-provisioning", "claude");
        worker.container_id = "ur-worker-provisioning".to_owned();
        worker.container_status = "provisioning".to_owned();
        repo.insert_worker(&worker).await.unwrap();

        repo.finalize_worker_launch("w-provisioning", "sha256:launched")
            .await
            .unwrap();
        let finalized = repo.get_worker("w-provisioning").await.unwrap().unwrap();
        assert_eq!(finalized.container_id, "sha256:launched");
        assert_eq!(finalized.container_status, "running");

        assert_eq!(repo.delete_worker("w-provisioning").await.unwrap(), 1);
        assert!(repo.get_worker("w-provisioning").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_queries_include_agent_type() {
        let test_db = TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());

        repo.insert_worker(&test_worker("w-list-1", "claude"))
            .await
            .unwrap();

        let by_status = repo
            .list_workers_by_container_status("running")
            .await
            .unwrap();
        assert!(by_status.iter().any(|w| w.agent_type == "claude"));

        let all = repo.list_all_workers().await.unwrap();
        assert!(all.iter().any(|w| w.agent_type == "claude"));

        let active = repo.list_active_workers().await.unwrap();
        assert!(active.iter().any(|w| w.agent_type == "claude"));

        let context = repo
            .get_worker_context("testproj", "")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(context.agent_type, "claude");
    }

    /// Rows written without specifying `agent_type` or `runtime` (as any row
    /// inserted before those migrations would have been) backfill to the
    /// columns' DEFAULTs.
    #[tokio::test]
    async fn rows_missing_agent_type_and_runtime_backfill_to_defaults() {
        let test_db = TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());
        repo_insert_raw(&test_db, "w-backfill").await;

        let worker = repo.get_worker("w-backfill").await.unwrap().unwrap();
        assert_eq!(worker.agent_type, "claude");
        assert_eq!(worker.runtime, WorkerRuntime::Container);
    }
}
