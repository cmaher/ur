// TicketRepo: CRUD operations for tickets, activities, and metadata.

use std::collections::HashMap;

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::graph::GraphManager;
use crate::model::{
    Activity, DispatchableTicket, Edge, EdgeKind, LifecycleStatus, MetadataMatchTicket, NewTicket,
    Ticket, TicketFilter, TicketUpdate, WorkflowEvent,
};

#[derive(Clone)]
pub struct TicketRepo {
    pool: SqlitePool,
    graph_manager: GraphManager,
}

impl TicketRepo {
    pub fn new(pool: SqlitePool, graph_manager: GraphManager) -> Self {
        Self {
            pool,
            graph_manager,
        }
    }

    pub async fn create_ticket(&self, ticket: &NewTicket) -> Result<Ticket, sqlx::Error> {
        let now = ticket
            .created_at
            .clone()
            .unwrap_or_else(|| Utc::now().to_rfc3339());
        let status = ticket.status.as_deref().unwrap_or("open");
        let lifecycle_status = ticket.lifecycle_status.unwrap_or_default();

        sqlx::query(
            "INSERT INTO ticket (id, project, type, status, lifecycle_status, priority, parent_id, title, body, branch, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&ticket.id)
        .bind(&ticket.project)
        .bind(&ticket.type_)
        .bind(status)
        .bind(lifecycle_status.as_str())
        .bind(ticket.priority)
        .bind(&ticket.parent_id)
        .bind(&ticket.title)
        .bind(&ticket.body)
        .bind(&ticket.branch)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(Ticket {
            id: ticket.id.clone(),
            project: ticket.project.clone(),
            type_: ticket.type_.clone(),
            status: status.to_owned(),
            lifecycle_status,
            priority: ticket.priority,
            parent_id: ticket.parent_id.clone(),
            title: ticket.title.clone(),
            body: ticket.body.clone(),
            branch: ticket.branch.clone(),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub async fn get_ticket(&self, id: &str) -> Result<Option<Ticket>, sqlx::Error> {
        let row = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                String,
                i32,
                Option<String>,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT id, project, type, status, lifecycle_status, priority, parent_id, title, body, branch, created_at, updated_at
             FROM ticket WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(
                id,
                project,
                type_,
                status,
                lifecycle_status_str,
                priority,
                parent_id,
                title,
                body,
                branch,
                created_at,
                updated_at,
            )| {
                Ticket {
                    id,
                    project,
                    type_,
                    status,
                    lifecycle_status: lifecycle_status_str
                        .parse::<LifecycleStatus>()
                        .unwrap_or_default(),
                    priority,
                    parent_id,
                    title,
                    body,
                    branch,
                    created_at,
                    updated_at,
                }
            },
        ))
    }

    pub async fn update_ticket(
        &self,
        id: &str,
        update: &TicketUpdate,
    ) -> Result<Ticket, sqlx::Error> {
        let existing = self
            .get_ticket(id)
            .await?
            .ok_or_else(|| sqlx::Error::RowNotFound)?;

        let status = update.status.as_deref().unwrap_or(&existing.status);
        let lifecycle_status = update.lifecycle_status.unwrap_or(existing.lifecycle_status);
        let type_ = update.type_.as_deref().unwrap_or(&existing.type_);
        let priority = update.priority.unwrap_or(existing.priority);
        let title = update.title.as_deref().unwrap_or(&existing.title);
        let body = update.body.as_deref().unwrap_or(&existing.body);
        let parent_id = match &update.parent_id {
            Some(p) => p.as_deref(),
            None => existing.parent_id.as_deref(),
        };
        let branch = match &update.branch {
            Some(b) => b.as_deref(),
            None => existing.branch.as_deref(),
        };
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE ticket SET type = ?, status = ?, lifecycle_status = ?, priority = ?, title = ?, body = ?, parent_id = ?, branch = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(type_)
        .bind(status)
        .bind(lifecycle_status.as_str())
        .bind(priority)
        .bind(title)
        .bind(body)
        .bind(parent_id)
        .bind(branch)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(Ticket {
            id: existing.id,
            project: existing.project,
            type_: type_.to_owned(),
            status: status.to_owned(),
            lifecycle_status,
            priority,
            parent_id: parent_id.map(|s| s.to_owned()),
            title: title.to_owned(),
            body: body.to_owned(),
            branch: branch.map(|s| s.to_owned()),
            created_at: existing.created_at,
            updated_at: now,
        })
    }

    pub async fn list_tickets(&self, filter: &TicketFilter) -> Result<Vec<Ticket>, sqlx::Error> {
        let mut query = String::from(
            "SELECT id, project, type, status, lifecycle_status, priority, parent_id, title, body, branch, created_at, updated_at FROM ticket WHERE 1=1",
        );
        let mut binds: Vec<String> = Vec::new();

        if let Some(ref project) = filter.project {
            query.push_str(" AND project = ?");
            binds.push(project.clone());
        }
        if let Some(ref status) = filter.status {
            query.push_str(" AND status = ?");
            binds.push(status.clone());
        }
        if let Some(ref type_) = filter.type_ {
            query.push_str(" AND type = ?");
            binds.push(type_.clone());
        }
        if let Some(ref parent_id) = filter.parent_id {
            query.push_str(" AND parent_id = ?");
            binds.push(parent_id.clone());
        }

        query.push_str(" ORDER BY priority ASC, created_at ASC");

        let mut q = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                String,
                i32,
                Option<String>,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(sqlx::AssertSqlSafe(query));
        for bind in &binds {
            q = q.bind(bind);
        }

        let rows = q.fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    project,
                    type_,
                    status,
                    lifecycle_status_str,
                    priority,
                    parent_id,
                    title,
                    body,
                    branch,
                    created_at,
                    updated_at,
                )| {
                    Ticket {
                        id,
                        project,
                        type_,
                        status,
                        lifecycle_status: lifecycle_status_str
                            .parse::<LifecycleStatus>()
                            .unwrap_or_default(),
                        priority,
                        parent_id,
                        title,
                        body,
                        branch,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect())
    }

    pub async fn set_meta(
        &self,
        entity_id: &str,
        entity_type: &str,
        key: &str,
        value: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO meta (entity_id, entity_type, key, value) VALUES (?, ?, ?, ?)
             ON CONFLICT (entity_id, entity_type, key) DO UPDATE SET value = excluded.value",
        )
        .bind(entity_id)
        .bind(entity_type)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_meta(
        &self,
        entity_id: &str,
        entity_type: &str,
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value FROM meta WHERE entity_id = ? AND entity_type = ?",
        )
        .bind(entity_id)
        .bind(entity_type)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().collect())
    }

    pub async fn add_edge(
        &self,
        source: &str,
        target: &str,
        kind: EdgeKind,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT OR IGNORE INTO edge (source_id, target_id, kind) VALUES (?, ?, ?)")
            .bind(source)
            .bind(target)
            .bind(edge_kind_to_str(&kind))
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn remove_edge(
        &self,
        source: &str,
        target: &str,
        kind: EdgeKind,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM edge WHERE source_id = ? AND target_id = ? AND kind = ?")
            .bind(source)
            .bind(target)
            .bind(edge_kind_to_str(&kind))
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn edges_for(
        &self,
        ticket_id: &str,
        kind: Option<EdgeKind>,
    ) -> Result<Vec<Edge>, sqlx::Error> {
        let rows = match kind {
            Some(ref k) => {
                let kind_str = edge_kind_to_str(k);
                sqlx::query_as::<_, (String, String, String)>(
                    "SELECT source_id, target_id, kind FROM edge
                     WHERE (source_id = ? OR target_id = ?) AND kind = ?",
                )
                .bind(ticket_id)
                .bind(ticket_id)
                .bind(kind_str)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, (String, String, String)>(
                    "SELECT source_id, target_id, kind FROM edge
                     WHERE source_id = ? OR target_id = ?",
                )
                .bind(ticket_id)
                .bind(ticket_id)
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows
            .into_iter()
            .map(|(source_id, target_id, kind_str)| Edge {
                source_id,
                target_id,
                kind: edge_kind_from_str(&kind_str),
            })
            .collect())
    }

    pub async fn add_activity(
        &self,
        ticket_id: &str,
        author: &str,
        message: &str,
    ) -> Result<Activity, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let timestamp = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO activity (id, ticket_id, timestamp, author, message) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(ticket_id)
        .bind(&timestamp)
        .bind(author)
        .bind(message)
        .execute(&self.pool)
        .await?;

        Ok(Activity {
            id,
            ticket_id: ticket_id.to_owned(),
            timestamp,
            author: author.to_owned(),
            message: message.to_owned(),
        })
    }

    pub async fn get_activities(&self, ticket_id: &str) -> Result<Vec<Activity>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT id, ticket_id, timestamp, author, message FROM activity
             WHERE ticket_id = ? ORDER BY timestamp ASC",
        )
        .bind(ticket_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, ticket_id, timestamp, author, message)| Activity {
                id,
                ticket_id,
                timestamp,
                author,
                message,
            })
            .collect())
    }

    pub async fn tickets_by_metadata(
        &self,
        key: &str,
        value: &str,
    ) -> Result<Vec<MetadataMatchTicket>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT t.id, t.title, t.type, t.status, m.key, m.value
             FROM ticket t
             JOIN meta m ON m.entity_id = t.id AND m.entity_type = 'ticket'
             WHERE m.key = ? AND m.value = ?
             ORDER BY t.priority ASC",
        )
        .bind(key)
        .bind(value)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, title, type_, status, key, value)| MetadataMatchTicket {
                    id,
                    title,
                    type_,
                    status,
                    key,
                    value,
                },
            )
            .collect())
    }

    pub async fn tickets_with_metadata_key(
        &self,
        key: &str,
    ) -> Result<Vec<MetadataMatchTicket>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT t.id, t.title, t.type, t.status, m.key, m.value
             FROM ticket t
             JOIN meta m ON m.entity_id = t.id AND m.entity_type = 'ticket'
             WHERE m.key = ?
             ORDER BY t.priority ASC",
        )
        .bind(key)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, title, type_, status, key, value)| MetadataMatchTicket {
                    id,
                    title,
                    type_,
                    status,
                    key,
                    value,
                },
            )
            .collect())
    }

    /// Returns true if the ticket has any transitive blocker that is not closed.
    async fn has_open_blockers(&self, ticket_id: &str) -> Result<bool, sqlx::Error> {
        let blockers = self.graph_manager.transitive_blockers(ticket_id).await?;
        if blockers.is_empty() {
            return Ok(false);
        }
        let placeholders: String = blockers.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let query = format!(
            "SELECT COUNT(*) FROM ticket WHERE id IN ({placeholders}) AND status != 'closed'"
        );
        let mut q = sqlx::query_scalar::<_, i32>(sqlx::AssertSqlSafe(query));
        for blocker_id in &blockers {
            q = q.bind(blocker_id);
        }
        let count = q.fetch_one(&self.pool).await?;
        Ok(count > 0)
    }

    /// Returns open children of the given epic that have no open blockers.
    /// Uses GraphManager to compute transitive blockers, then filters out
    /// any ticket that has at least one open blocker.
    ///
    /// If `project` is provided, only tickets belonging to that project are returned.
    pub async fn dispatchable_tickets(
        &self,
        epic_id: &str,
        project: Option<&str>,
    ) -> Result<Vec<DispatchableTicket>, sqlx::Error> {
        let (query, binds): (String, Vec<String>) = match project {
            Some(p) => (
                "SELECT id, title, priority, type FROM ticket
                 WHERE parent_id = ? AND status = 'open' AND project = ?
                 ORDER BY priority ASC"
                    .to_owned(),
                vec![epic_id.to_owned(), p.to_owned()],
            ),
            None => (
                "SELECT id, title, priority, type FROM ticket
                 WHERE parent_id = ? AND status = 'open'
                 ORDER BY priority ASC"
                    .to_owned(),
                vec![epic_id.to_owned()],
            ),
        };

        let mut q = sqlx::query_as::<_, (String, String, i32, String)>(sqlx::AssertSqlSafe(query));
        for bind in &binds {
            q = q.bind(bind);
        }
        let children = q.fetch_all(&self.pool).await?;

        let mut result = Vec::new();

        for (id, title, priority, type_) in children {
            let has_open_blocker = self.has_open_blockers(&id).await?;
            if !has_open_blocker {
                result.push(DispatchableTicket {
                    id,
                    title,
                    priority,
                    type_,
                });
            }
        }

        Ok(result)
    }

    pub async fn delete_meta(
        &self,
        entity_id: &str,
        entity_type: &str,
        key: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM meta WHERE entity_id = ? AND entity_type = ? AND key = ?")
            .bind(entity_id)
            .bind(entity_type)
            .bind(key)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Returns true if all children of the given epic are in 'closed' status.
    /// Returns true if the epic has no children.
    pub async fn epic_all_children_closed(&self, epic_id: &str) -> Result<bool, sqlx::Error> {
        let count = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM ticket WHERE parent_id = ? AND status != 'closed'",
        )
        .bind(epic_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(count == 0)
    }

    /// Close all open children of the given epic.
    pub async fn close_open_children(&self, epic_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE ticket SET status = 'closed' WHERE parent_id = ? AND status != 'closed'",
        )
        .bind(epic_id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Poll the oldest unprocessed workflow event.
    /// Returns `None` if no events are pending.
    pub async fn poll_workflow_event(&self) -> Result<Option<WorkflowEvent>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, String, String, i32, String)>(
            "SELECT id, ticket_id, old_lifecycle_status, new_lifecycle_status, attempts, created_at
             FROM workflow_event
             ORDER BY id ASC
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(id, ticket_id, old_status_str, new_status_str, attempts, created_at)| WorkflowEvent {
                id,
                ticket_id,
                old_lifecycle_status: old_status_str
                    .parse::<LifecycleStatus>()
                    .unwrap_or_default(),
                new_lifecycle_status: new_status_str
                    .parse::<LifecycleStatus>()
                    .unwrap_or_default(),
                attempts,
                created_at,
            },
        ))
    }

    /// Delete a workflow event by ID (after successful processing).
    pub async fn delete_workflow_event(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM workflow_event WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Increment the attempts counter on a workflow event (after a failed processing attempt).
    pub async fn increment_workflow_event_attempts(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow_event SET attempts = attempts + 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Return all tickets with the given lifecycle status.
    /// Used by GithubPoller to find tickets in pushing/in_review states.
    pub async fn tickets_by_lifecycle_status(
        &self,
        status: LifecycleStatus,
    ) -> Result<Vec<Ticket>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                String,
                i32,
                Option<String>,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT id, project, type, status, lifecycle_status, priority, parent_id, title, body, branch, created_at, updated_at
             FROM ticket
             WHERE lifecycle_status = ?
             ORDER BY priority ASC, created_at ASC",
        )
        .bind(status.as_str())
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    project,
                    type_,
                    status,
                    lifecycle_status_str,
                    priority,
                    parent_id,
                    title,
                    body,
                    branch,
                    created_at,
                    updated_at,
                )| {
                    Ticket {
                        id,
                        project,
                        type_,
                        status,
                        lifecycle_status: lifecycle_status_str
                            .parse::<LifecycleStatus>()
                            .unwrap_or_default(),
                        priority,
                        parent_id,
                        title,
                        body,
                        branch,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect())
    }
}

fn edge_kind_to_str(kind: &EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Blocks => "blocks",
        EdgeKind::RelatesTo => "relates_to",
        EdgeKind::FollowUp => "follow_up",
    }
}

fn edge_kind_from_str(s: &str) -> EdgeKind {
    match s {
        "blocks" => EdgeKind::Blocks,
        "follow_up" => EdgeKind::FollowUp,
        _ => EdgeKind::RelatesTo,
    }
}
