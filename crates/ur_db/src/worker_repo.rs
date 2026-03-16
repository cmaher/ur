// WorkerRepo: CRUD operations for worker and slot tables, plus startup reconciliation.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::model::{Slot, Worker, WorkerSlot};

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
    pool: SqlitePool,
}

impl WorkerRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // --- Worker methods ---

    pub async fn insert_worker(&self, worker: &Worker) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO worker (worker_id, process_id, project_key, container_id, worker_secret, strategy, status, workspace_path, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&worker.worker_id)
        .bind(&worker.process_id)
        .bind(&worker.project_key)
        .bind(&worker.container_id)
        .bind(&worker.worker_secret)
        .bind(&worker.strategy)
        .bind(&worker.status)
        .bind(&worker.workspace_path)
        .bind(&worker.created_at)
        .bind(&worker.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_worker(&self, worker_id: &str) -> Result<Option<Worker>, sqlx::Error> {
        let row = sqlx::query_as::<
            _,
            (
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
            ),
        >(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, status, workspace_path, created_at, updated_at
             FROM worker WHERE worker_id = ?",
        )
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(
                worker_id,
                process_id,
                project_key,
                container_id,
                worker_secret,
                strategy,
                status,
                workspace_path,
                created_at,
                updated_at,
            )| {
                Worker {
                    worker_id,
                    process_id,
                    project_key,
                    container_id,
                    worker_secret,
                    strategy,
                    status,
                    workspace_path,
                    created_at,
                    updated_at,
                }
            },
        ))
    }

    pub async fn update_worker_status(
        &self,
        worker_id: &str,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();

        sqlx::query("UPDATE worker SET status = ?, updated_at = ? WHERE worker_id = ?")
            .bind(status)
            .bind(&now)
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn list_workers_by_status(&self, status: &str) -> Result<Vec<Worker>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
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
            ),
        >(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, status, workspace_path, created_at, updated_at
             FROM worker WHERE status = ? ORDER BY created_at ASC",
        )
        .bind(status)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    worker_id,
                    process_id,
                    project_key,
                    container_id,
                    worker_secret,
                    strategy,
                    status,
                    workspace_path,
                    created_at,
                    updated_at,
                )| {
                    Worker {
                        worker_id,
                        process_id,
                        project_key,
                        container_id,
                        worker_secret,
                        strategy,
                        status,
                        workspace_path,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect())
    }

    pub async fn verify_worker(&self, worker_id: &str, secret: &str) -> Result<bool, sqlx::Error> {
        let count = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM worker WHERE worker_id = ? AND worker_secret = ?",
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
        let row = sqlx::query_as::<
            _,
            (
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
            ),
        >(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, status, workspace_path, created_at, updated_at
             FROM worker WHERE project_key = ? AND workspace_path = ?",
        )
        .bind(project_key)
        .bind(workspace_path)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(
                worker_id,
                process_id,
                project_key,
                container_id,
                worker_secret,
                strategy,
                status,
                workspace_path,
                created_at,
                updated_at,
            )| {
                Worker {
                    worker_id,
                    process_id,
                    project_key,
                    container_id,
                    worker_secret,
                    strategy,
                    status,
                    workspace_path,
                    created_at,
                    updated_at,
                }
            },
        ))
    }

    // --- Slot methods ---

    pub async fn insert_slot(&self, slot: &Slot) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO slot (id, project_key, slot_name, slot_type, host_path, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&slot.id)
        .bind(&slot.project_key)
        .bind(&slot.slot_name)
        .bind(&slot.slot_type)
        .bind(&slot.host_path)
        .bind(&slot.created_at)
        .bind(&slot.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_slot(&self, id: &str) -> Result<Option<Slot>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
            "SELECT id, project_key, slot_name, slot_type, host_path, created_at, updated_at
             FROM slot WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(id, project_key, slot_name, slot_type, host_path, created_at, updated_at)| Slot {
                id,
                project_key,
                slot_name,
                slot_type,
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
        let row = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
            "SELECT id, project_key, slot_name, slot_type, host_path, created_at, updated_at
             FROM slot WHERE host_path = ?",
        )
        .bind(host_path)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(id, project_key, slot_name, slot_type, host_path, created_at, updated_at)| Slot {
                id,
                project_key,
                slot_name,
                slot_type,
                host_path,
                created_at,
                updated_at,
            },
        ))
    }

    pub async fn list_slots_by_project(&self, project_key: &str) -> Result<Vec<Slot>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
            "SELECT id, project_key, slot_name, slot_type, host_path, created_at, updated_at
             FROM slot WHERE project_key = ? ORDER BY created_at ASC",
        )
        .bind(project_key)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, project_key, slot_name, slot_type, host_path, created_at, updated_at)| Slot {
                    id,
                    project_key,
                    slot_name,
                    slot_type,
                    host_path,
                    created_at,
                    updated_at,
                },
            )
            .collect())
    }

    /// Find the first available exclusive slot for a project (not linked to an active worker).
    pub async fn find_available_exclusive_slot(
        &self,
        project_key: &str,
    ) -> Result<Option<Slot>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
            "SELECT s.id, s.project_key, s.slot_name, s.slot_type, s.host_path, s.created_at, s.updated_at
             FROM slot s
             WHERE s.project_key = ? AND s.slot_type = 'exclusive'
               AND s.id NOT IN (
                 SELECT ws.slot_id FROM worker_slot ws
                 INNER JOIN worker w ON w.worker_id = ws.worker_id
                 WHERE w.status IN ('provisioning', 'running', 'stopping')
               )
             ORDER BY s.created_at ASC
             LIMIT 1",
        )
        .bind(project_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(id, project_key, slot_name, slot_type, host_path, created_at, updated_at)| Slot {
                id,
                project_key,
                slot_name,
                slot_type,
                host_path,
                created_at,
                updated_at,
            },
        ))
    }

    /// Count exclusive slots that have a running worker linked via worker_slot.
    pub async fn exclusive_slots_in_use(&self, project_key: &str) -> Result<i32, sqlx::Error> {
        let count = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM slot s
             INNER JOIN worker_slot ws ON ws.slot_id = s.id
             INNER JOIN worker w ON w.worker_id = ws.worker_id
             WHERE s.project_key = ? AND s.slot_type = 'exclusive'
               AND w.status IN ('provisioning', 'running', 'stopping')",
        )
        .bind(project_key)
        .fetch_one(&self.pool)
        .await?;

        Ok(count)
    }

    pub async fn delete_slot(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM slot WHERE id = ?")
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
        sqlx::query("INSERT INTO worker_slot (worker_id, slot_id, created_at) VALUES (?, ?, ?)")
            .bind(worker_id)
            .bind(slot_id)
            .bind(&now)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Unlink a worker from its slot by removing the worker_slot row.
    pub async fn unlink_worker_slot(&self, worker_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM worker_slot WHERE worker_id = ?")
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
            "SELECT worker_id, slot_id, created_at FROM worker_slot WHERE worker_id = ?",
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
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
            "SELECT id, project_key, slot_name, slot_type, host_path, created_at, updated_at
             FROM slot ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, project_key, slot_name, slot_type, host_path, created_at, updated_at)| Slot {
                    id,
                    project_key,
                    slot_name,
                    slot_type,
                    host_path,
                    created_at,
                    updated_at,
                },
            )
            .collect())
    }

    /// List workers whose status is one of the active lifecycle states
    /// (provisioning, running, stopping).
    pub async fn list_active_workers(&self) -> Result<Vec<Worker>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
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
            ),
        >(
            "SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, status, workspace_path, created_at, updated_at
             FROM worker WHERE status IN ('provisioning', 'running', 'stopping') ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    worker_id,
                    process_id,
                    project_key,
                    container_id,
                    worker_secret,
                    strategy,
                    status,
                    workspace_path,
                    created_at,
                    updated_at,
                )| {
                    Worker {
                        worker_id,
                        process_id,
                        project_key,
                        container_id,
                        worker_secret,
                        strategy,
                        status,
                        workspace_path,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect())
    }

    /// Delete all workers that are linked to a given slot_id via worker_slot.
    pub async fn delete_workers_by_slot_id(&self, slot_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM worker WHERE worker_id IN (SELECT worker_id FROM worker_slot WHERE slot_id = ?)",
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
            let slot_type = if slot_name.parse::<u32>().is_ok() {
                "exclusive"
            } else {
                "shared"
            };
            let host_path = project_pool_dir.join(slot_name);
            let slot = Slot {
                id: Uuid::new_v4().to_string(),
                project_key: project_key.to_owned(),
                slot_name: slot_name.clone(),
                slot_type: slot_type.to_owned(),
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
    /// For each worker in an active state (provisioning, running, stopping):
    /// - Container alive: update status to "running" (reclaim).
    /// - Container dead: update status to "stopped" and unlink its slot.
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

        let active_workers = self.list_active_workers().await?;

        for worker in active_workers {
            let alive = is_container_alive(worker.container_id.clone()).await;
            self.reconcile_single_worker(worker, alive, &mut result)
                .await?;
        }

        Ok(result)
    }

    /// Process a single worker during reconciliation.
    async fn reconcile_single_worker(
        &self,
        worker: Worker,
        alive: bool,
        result: &mut WorkerReconcileResult,
    ) -> Result<(), sqlx::Error> {
        if alive {
            self.update_worker_status(&worker.worker_id, "running")
                .await?;
            result.reclaimed.push(worker.worker_id);
        } else {
            self.update_worker_status(&worker.worker_id, "stopped")
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
            names.insert(name.to_owned());
        }
    }
    names
}
