// WorkerRepo: CRUD operations for worker and slot tables, plus startup reconciliation.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::model::{AgentStatus, Slot, Worker, WorkerSlot};

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
    Option<String>,
    String,
    String,
    i32,
);

fn worker_from_row(row: WorkerRow) -> Worker {
    Worker {
        worker_id: row.0,
        process_id: row.1,
        project_key: row.2,
        container_id: row.3,
        worker_secret: row.4,
        strategy: row.5,
        container_status: row.6,
        agent_status: row.7,
        workspace_path: row.8,
        created_at: row.9,
        updated_at: row.10,
        idle_redispatch_count: row.11,
    }
}

impl WorkerRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // --- Worker methods ---

    pub async fn insert_worker(&self, worker: &Worker) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO worker (worker_id, process_id, project_key, container_id, worker_secret, strategy, container_status, agent_status, workspace_path, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&worker.worker_id)
        .bind(&worker.process_id)
        .bind(&worker.project_key)
        .bind(&worker.container_id)
        .bind(&worker.worker_secret)
        .bind(&worker.strategy)
        .bind(&worker.container_status)
        .bind(&worker.agent_status)
        .bind(&worker.workspace_path)
        .bind(&worker.created_at)
        .bind(&worker.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_worker(&self, worker_id: &str) -> Result<Option<Worker>, sqlx::Error> {
        let row = sqlx::query_as::<_, WorkerRow>(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count
             FROM worker WHERE worker_id = $1",
        )
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(worker_from_row))
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
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count
             FROM worker WHERE container_status = $1 ORDER BY created_at ASC",
        )
        .bind(container_status)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(worker_from_row).collect())
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
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count
             FROM worker WHERE project_key = $1 AND workspace_path = $2",
        )
        .bind(project_key)
        .bind(workspace_path)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(worker_from_row))
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

        Ok(row.map(
            |(id, project_key, slot_name, host_path, created_at, updated_at)| Slot {
                id,
                project_key,
                slot_name,
                host_path,
                created_at,
                updated_at,
            },
        ))
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

        Ok(row.map(
            |(id, project_key, slot_name, host_path, created_at, updated_at)| Slot {
                id,
                project_key,
                slot_name,
                host_path,
                created_at,
                updated_at,
            },
        ))
    }

    pub async fn list_slots_by_project(&self, project_key: &str) -> Result<Vec<Slot>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT id, project_key, slot_name, host_path, created_at, updated_at
             FROM slot WHERE project_key = $1 ORDER BY created_at ASC",
        )
        .bind(project_key)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, project_key, slot_name, host_path, created_at, updated_at)| Slot {
                    id,
                    project_key,
                    slot_name,
                    host_path,
                    created_at,
                    updated_at,
                },
            )
            .collect())
    }

    /// Find the first available exclusive slot for a project (not linked to an active worker).
    ///
    /// Only returns slots with numeric names (exclusive pool slots like "0", "1", "2").
    /// Shared slots (name = "shared") are excluded — they are managed separately by
    /// `acquire_shared_slot` and should never be assigned to code workers.
    pub async fn find_available_slot(
        &self,
        project_key: &str,
    ) -> Result<Option<Slot>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT s.id, s.project_key, s.slot_name, s.host_path, s.created_at, s.updated_at
             FROM slot s
             WHERE s.project_key = $1
               AND s.slot_name != 'shared'
               AND s.id NOT IN (
                 SELECT ws.slot_id FROM worker_slot ws
                 INNER JOIN worker w ON w.worker_id = ws.worker_id
                 WHERE w.container_status IN ('provisioning', 'running', 'stopping')
               )
             ORDER BY s.created_at ASC
             LIMIT 1",
        )
        .bind(project_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(id, project_key, slot_name, host_path, created_at, updated_at)| Slot {
                id,
                project_key,
                slot_name,
                host_path,
                created_at,
                updated_at,
            },
        ))
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

        Ok(rows
            .into_iter()
            .map(
                |(id, project_key, slot_name, host_path, created_at, updated_at)| Slot {
                    id,
                    project_key,
                    slot_name,
                    host_path,
                    created_at,
                    updated_at,
                },
            )
            .collect())
    }

    /// List all workers regardless of container_status.
    pub async fn list_all_workers(&self) -> Result<Vec<Worker>, sqlx::Error> {
        let rows = sqlx::query_as::<_, WorkerRow>(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count
             FROM worker ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(worker_from_row).collect())
    }

    /// List workers whose container_status is one of the active lifecycle states
    /// (provisioning, running, stopping).
    pub async fn list_active_workers(&self) -> Result<Vec<Worker>, sqlx::Error> {
        let rows = sqlx::query_as::<_, WorkerRow>(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count
             FROM worker WHERE container_status IN ('provisioning', 'running', 'stopping') ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(worker_from_row).collect())
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
    pub async fn reconcile_workers<F, Fut>(
        &self,
        is_container_alive: F,
    ) -> Result<WorkerReconcileResult, sqlx::Error>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = bool>,
    {
        let mut result = WorkerReconcileResult {
            reclaimed: Vec::new(),
            marked_stopped: Vec::new(),
        };

        let all_workers = self.list_all_workers().await?;

        for worker in all_workers {
            let alive = is_container_alive(worker.container_id.clone()).await;
            self.reconcile_single_worker(worker, alive, &mut result)
                .await?;
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
