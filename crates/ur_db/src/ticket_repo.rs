// TicketRepo: CRUD operations for tickets, activities, and metadata.

use std::collections::HashMap;

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::graph::GraphManager;
use crate::model::{
    Activity, DispatchableTicket, Edge, EdgeKind, MetadataMatchTicket, NewTicket, Ticket,
    TicketFilter, TicketUpdate,
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
        let now = Utc::now().to_rfc3339();
        let status = "open";

        sqlx::query(
            "INSERT INTO ticket (id, type, status, priority, parent_id, title, body, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&ticket.id)
        .bind(&ticket.type_)
        .bind(status)
        .bind(ticket.priority)
        .bind(&ticket.parent_id)
        .bind(&ticket.title)
        .bind(&ticket.body)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(Ticket {
            id: ticket.id.clone(),
            type_: ticket.type_.clone(),
            status: status.to_owned(),
            priority: ticket.priority,
            parent_id: ticket.parent_id.clone(),
            title: ticket.title.clone(),
            body: ticket.body.clone(),
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
                i32,
                Option<String>,
                String,
                String,
                String,
                String,
            ),
        >(
            "SELECT id, type, status, priority, parent_id, title, body, created_at, updated_at
             FROM ticket WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(id, type_, status, priority, parent_id, title, body, created_at, updated_at)| {
                Ticket {
                    id,
                    type_,
                    status,
                    priority,
                    parent_id,
                    title,
                    body,
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
        let priority = update.priority.unwrap_or(existing.priority);
        let title = update.title.as_deref().unwrap_or(&existing.title);
        let body = update.body.as_deref().unwrap_or(&existing.body);
        let parent_id = match &update.parent_id {
            Some(p) => p.as_deref(),
            None => existing.parent_id.as_deref(),
        };
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE ticket SET status = ?, priority = ?, title = ?, body = ?, parent_id = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(status)
        .bind(priority)
        .bind(title)
        .bind(body)
        .bind(parent_id)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(Ticket {
            id: existing.id,
            type_: existing.type_,
            status: status.to_owned(),
            priority,
            parent_id: parent_id.map(|s| s.to_owned()),
            title: title.to_owned(),
            body: body.to_owned(),
            created_at: existing.created_at,
            updated_at: now,
        })
    }

    pub async fn list_tickets(&self, filter: &TicketFilter) -> Result<Vec<Ticket>, sqlx::Error> {
        let mut query = String::from(
            "SELECT id, type, status, priority, parent_id, title, body, created_at, updated_at FROM ticket WHERE 1=1",
        );
        let mut binds: Vec<String> = Vec::new();

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
                i32,
                Option<String>,
                String,
                String,
                String,
                String,
            ),
        >(&query);
        for bind in &binds {
            q = q.bind(bind);
        }

        let rows = q.fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, type_, status, priority, parent_id, title, body, created_at, updated_at)| {
                    Ticket {
                        id,
                        type_,
                        status,
                        priority,
                        parent_id,
                        title,
                        body,
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
        let mut q = sqlx::query_scalar::<_, i32>(&query);
        for blocker_id in &blockers {
            q = q.bind(blocker_id);
        }
        let count = q.fetch_one(&self.pool).await?;
        Ok(count > 0)
    }

    /// Returns open children of the given epic that have no open blockers.
    /// Uses GraphManager to compute transitive blockers, then filters out
    /// any ticket that has at least one open blocker.
    pub async fn dispatchable_tickets(
        &self,
        epic_id: &str,
    ) -> Result<Vec<DispatchableTicket>, sqlx::Error> {
        let children = sqlx::query_as::<_, (String, String, i32, String)>(
            "SELECT id, title, priority, type FROM ticket
             WHERE parent_id = ? AND status = 'open'
             ORDER BY priority ASC",
        )
        .bind(epic_id)
        .fetch_all(&self.pool)
        .await?;

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
}

fn edge_kind_to_str(kind: &EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Blocks => "blocks",
        EdgeKind::RelatesTo => "relates_to",
    }
}

fn edge_kind_from_str(s: &str) -> EdgeKind {
    match s {
        "blocks" => EdgeKind::Blocks,
        _ => EdgeKind::RelatesTo,
    }
}
