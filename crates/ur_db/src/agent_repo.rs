// AgentRepo: CRUD operations for agent and slot tables, plus startup reconciliation.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::model::{Agent, Slot};

/// Result of slot reconciliation: reports what was cleaned up or discovered.
pub struct SlotReconcileResult {
    /// Slot IDs that were deleted because their host_path no longer exists on disk.
    pub deleted_stale: Vec<String>,
    /// Slot IDs that were inserted because an on-disk directory had no DB row.
    pub inserted_orphaned: Vec<String>,
}

/// Result of agent reconciliation: reports what was reclaimed or marked dead.
pub struct AgentReconcileResult {
    /// Agent IDs whose containers are still alive (kept as running).
    pub reclaimed: Vec<String>,
    /// Agent IDs whose containers are dead (marked stopped, slots released).
    pub marked_stopped: Vec<String>,
}

#[derive(Clone)]
pub struct AgentRepo {
    pool: SqlitePool,
}

impl AgentRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // --- Agent methods ---

    pub async fn insert_agent(&self, agent: &Agent) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO agent (agent_id, process_id, project_key, slot_id, container_id, agent_secret, strategy, status, workspace_path, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&agent.agent_id)
        .bind(&agent.process_id)
        .bind(&agent.project_key)
        .bind(&agent.slot_id)
        .bind(&agent.container_id)
        .bind(&agent.agent_secret)
        .bind(&agent.strategy)
        .bind(&agent.status)
        .bind(&agent.workspace_path)
        .bind(&agent.created_at)
        .bind(&agent.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<Option<Agent>, sqlx::Error> {
        let row = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                Option<String>,
                String,
                String,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT agent_id, process_id, project_key, slot_id, container_id, agent_secret, strategy, status, workspace_path, created_at, updated_at
             FROM agent WHERE agent_id = ?",
        )
        .bind(agent_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(
                agent_id,
                process_id,
                project_key,
                slot_id,
                container_id,
                agent_secret,
                strategy,
                status,
                workspace_path,
                created_at,
                updated_at,
            )| {
                Agent {
                    agent_id,
                    process_id,
                    project_key,
                    slot_id,
                    container_id,
                    agent_secret,
                    strategy,
                    status,
                    workspace_path,
                    created_at,
                    updated_at,
                }
            },
        ))
    }

    pub async fn update_agent_status(
        &self,
        agent_id: &str,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();

        sqlx::query("UPDATE agent SET status = ?, updated_at = ? WHERE agent_id = ?")
            .bind(status)
            .bind(&now)
            .bind(agent_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn list_agents_by_status(&self, status: &str) -> Result<Vec<Agent>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                Option<String>,
                String,
                String,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT agent_id, process_id, project_key, slot_id, container_id, agent_secret, strategy, status, workspace_path, created_at, updated_at
             FROM agent WHERE status = ? ORDER BY created_at ASC",
        )
        .bind(status)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    agent_id,
                    process_id,
                    project_key,
                    slot_id,
                    container_id,
                    agent_secret,
                    strategy,
                    status,
                    workspace_path,
                    created_at,
                    updated_at,
                )| {
                    Agent {
                        agent_id,
                        process_id,
                        project_key,
                        slot_id,
                        container_id,
                        agent_secret,
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

    pub async fn verify_agent(&self, agent_id: &str, secret: &str) -> Result<bool, sqlx::Error> {
        let count = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM agent WHERE agent_id = ? AND agent_secret = ?",
        )
        .bind(agent_id)
        .bind(secret)
        .fetch_one(&self.pool)
        .await?;

        Ok(count > 0)
    }

    pub async fn get_agent_context(
        &self,
        project_key: &str,
        workspace_path: &str,
    ) -> Result<Option<Agent>, sqlx::Error> {
        let row = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                Option<String>,
                String,
                String,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT agent_id, process_id, project_key, slot_id, container_id, agent_secret, strategy, status, workspace_path, created_at, updated_at
             FROM agent WHERE project_key = ? AND workspace_path = ?",
        )
        .bind(project_key)
        .bind(workspace_path)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(
                agent_id,
                process_id,
                project_key,
                slot_id,
                container_id,
                agent_secret,
                strategy,
                status,
                workspace_path,
                created_at,
                updated_at,
            )| {
                Agent {
                    agent_id,
                    process_id,
                    project_key,
                    slot_id,
                    container_id,
                    agent_secret,
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
            "INSERT INTO slot (id, project_key, slot_name, slot_type, host_path, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&slot.id)
        .bind(&slot.project_key)
        .bind(&slot.slot_name)
        .bind(&slot.slot_type)
        .bind(&slot.host_path)
        .bind(&slot.status)
        .bind(&slot.created_at)
        .bind(&slot.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_slot(&self, id: &str) -> Result<Option<Slot>, sqlx::Error> {
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
                String,
            ),
        >(
            "SELECT id, project_key, slot_name, slot_type, host_path, status, created_at, updated_at
             FROM slot WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(id, project_key, slot_name, slot_type, host_path, status, created_at, updated_at)| {
                Slot {
                    id,
                    project_key,
                    slot_name,
                    slot_type,
                    host_path,
                    status,
                    created_at,
                    updated_at,
                }
            },
        ))
    }

    pub async fn get_slot_by_host_path(
        &self,
        host_path: &str,
    ) -> Result<Option<Slot>, sqlx::Error> {
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
                String,
            ),
        >(
            "SELECT id, project_key, slot_name, slot_type, host_path, status, created_at, updated_at
             FROM slot WHERE host_path = ?",
        )
        .bind(host_path)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(id, project_key, slot_name, slot_type, host_path, status, created_at, updated_at)| {
                Slot {
                    id,
                    project_key,
                    slot_name,
                    slot_type,
                    host_path,
                    status,
                    created_at,
                    updated_at,
                }
            },
        ))
    }

    pub async fn update_slot_status(&self, id: &str, status: &str) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();

        sqlx::query("UPDATE slot SET status = ?, updated_at = ? WHERE id = ?")
            .bind(status)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn list_slots_by_project(&self, project_key: &str) -> Result<Vec<Slot>, sqlx::Error> {
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
                String,
            ),
        >(
            "SELECT id, project_key, slot_name, slot_type, host_path, status, created_at, updated_at
             FROM slot WHERE project_key = ? ORDER BY created_at ASC",
        )
        .bind(project_key)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    project_key,
                    slot_name,
                    slot_type,
                    host_path,
                    status,
                    created_at,
                    updated_at,
                )| {
                    Slot {
                        id,
                        project_key,
                        slot_name,
                        slot_type,
                        host_path,
                        status,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect())
    }

    pub async fn exclusive_slots_in_use(&self, project_key: &str) -> Result<i32, sqlx::Error> {
        let count = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM slot WHERE project_key = ? AND slot_type = 'exclusive' AND status = 'in_use'",
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

    // --- Reconciliation helpers ---

    /// List all slots across all projects.
    pub async fn list_all_slots(&self) -> Result<Vec<Slot>, sqlx::Error> {
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
                String,
            ),
        >(
            "SELECT id, project_key, slot_name, slot_type, host_path, status, created_at, updated_at
             FROM slot ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    project_key,
                    slot_name,
                    slot_type,
                    host_path,
                    status,
                    created_at,
                    updated_at,
                )| {
                    Slot {
                        id,
                        project_key,
                        slot_name,
                        slot_type,
                        host_path,
                        status,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect())
    }

    /// List agents whose status is one of the active lifecycle states
    /// (provisioning, running, stopping).
    pub async fn list_active_agents(&self) -> Result<Vec<Agent>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                Option<String>,
                String,
                String,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT agent_id, process_id, project_key, slot_id, container_id, agent_secret, strategy, status, workspace_path, created_at, updated_at
             FROM agent WHERE status IN ('provisioning', 'running', 'stopping') ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    agent_id,
                    process_id,
                    project_key,
                    slot_id,
                    container_id,
                    agent_secret,
                    strategy,
                    status,
                    workspace_path,
                    created_at,
                    updated_at,
                )| {
                    Agent {
                        agent_id,
                        process_id,
                        project_key,
                        slot_id,
                        container_id,
                        agent_secret,
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

    /// Delete all agents that reference a given slot_id.
    pub async fn delete_agents_by_slot_id(&self, slot_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM agent WHERE slot_id = ?")
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
    /// - Slot in DB but directory missing on disk: delete the slot row (and associated agent rows).
    /// - Directory on disk but no slot in DB: insert a new slot row with status "available".
    pub async fn reconcile_slots(
        &self,
        project_configs: &HashMap<String, std::path::PathBuf>,
        local_workspace: &Path,
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

        let pool_root = local_workspace.join("pool");

        for project_key in project_configs.keys() {
            let project_pool_dir = pool_root.join(project_key);
            let disk_slot_names = scan_disk_slots(&project_pool_dir).await;

            let db_slots = db_slots_by_project.remove(project_key).unwrap_or_default();

            self.delete_stale_slots(&db_slots, &disk_slot_names, &mut result)
                .await?;
            self.insert_orphaned_slots(
                project_key,
                &project_pool_dir,
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

    /// Delete slot rows that exist in DB but not on disk, along with their agents.
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
            self.delete_agents_by_slot_id(&slot.id).await?;
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
                status: "available".to_owned(),
                created_at: now.clone(),
                updated_at: now,
            };
            let slot_id = slot.id.clone();
            self.insert_slot(&slot).await?;
            result.inserted_orphaned.push(slot_id);
        }
        Ok(())
    }

    /// Reconcile agent DB rows with live Docker containers.
    ///
    /// `is_container_alive` is an async function that takes a container_id string
    /// and returns whether the container is still running.
    ///
    /// For each agent in an active state (provisioning, running, stopping):
    /// - Container alive: update status to "running" (reclaim).
    /// - Container dead: update status to "stopped" and release its slot (set slot status to "available").
    pub async fn reconcile_agents<F, Fut>(
        &self,
        is_container_alive: F,
    ) -> Result<AgentReconcileResult, sqlx::Error>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = bool>,
    {
        let mut result = AgentReconcileResult {
            reclaimed: Vec::new(),
            marked_stopped: Vec::new(),
        };

        let active_agents = self.list_active_agents().await?;

        for agent in active_agents {
            let alive = is_container_alive(agent.container_id.clone()).await;
            self.reconcile_single_agent(agent, alive, &mut result)
                .await?;
        }

        Ok(result)
    }

    /// Process a single agent during reconciliation.
    async fn reconcile_single_agent(
        &self,
        agent: Agent,
        alive: bool,
        result: &mut AgentReconcileResult,
    ) -> Result<(), sqlx::Error> {
        if alive {
            self.update_agent_status(&agent.agent_id, "running").await?;
            result.reclaimed.push(agent.agent_id);
        } else {
            self.update_agent_status(&agent.agent_id, "stopped").await?;
            if let Some(ref slot_id) = agent.slot_id {
                self.update_slot_status(slot_id, "available").await?;
            }
            result.marked_stopped.push(agent.agent_id);
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
