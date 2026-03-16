// AgentRepo: CRUD operations for agent and slot tables.

use chrono::Utc;
use sqlx::SqlitePool;

use crate::model::{Agent, Slot};

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
}
