// TicketRepo: CRUD operations for tickets, activities, and metadata.

use std::collections::HashMap;

use chrono::Utc;
use rand::Rng;
use sqlx::PgPool;
use uuid::Uuid;

use crate::graph::GraphManager;
use crate::model::{
    Activity, DispatchableTicket, Edge, EdgeKind, ImportError, LifecycleStatus,
    MetadataMatchTicket, NewTicket, Ticket, TicketFilter, TicketUpdate,
};

type TicketRow = (
    String,
    String,
    String,
    String,
    String,
    bool,
    i32,
    Option<String>,
    String,
    String,
    Option<String>,
    String,
    String,
    i32,
    i32,
);

fn ticket_from_row(row: TicketRow) -> Ticket {
    Ticket {
        id: row.0,
        project: row.1,
        type_: row.2,
        status: row.3,
        lifecycle_status: row.4.parse::<LifecycleStatus>().unwrap_or_default(),
        lifecycle_managed: row.5,
        priority: row.6,
        parent_id: row.7,
        title: row.8,
        body: row.9,
        branch: row.10,
        created_at: row.11,
        updated_at: row.12,
        children_completed: row.13,
        children_total: row.14,
    }
}

#[derive(Clone)]
pub struct TicketRepo {
    pool: PgPool,
    graph_manager: GraphManager,
}

impl TicketRepo {
    pub fn new(pool: PgPool, graph_manager: GraphManager) -> Self {
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

        let id = match &ticket.id {
            Some(provided) => provided.clone(),
            None => {
                self.insert_with_generated_id(ticket, status, lifecycle_status, &now)
                    .await?
            }
        };

        if ticket.id.is_some() {
            self.insert_ticket(&id, ticket, status, lifecycle_status, &now)
                .await?;
        }

        Ok(Ticket {
            id,
            project: ticket.project.clone(),
            type_: ticket.type_.clone(),
            status: status.to_owned(),
            lifecycle_status,
            lifecycle_managed: false,
            priority: ticket.priority,
            parent_id: ticket.parent_id.clone(),
            title: ticket.title.clone(),
            body: ticket.body.clone(),
            branch: ticket.branch.clone(),
            created_at: now.clone(),
            updated_at: now,
            children_completed: 0,
            children_total: 0,
        })
    }

    /// Generate a base-36 ID and insert, retrying with longer hashes on collision.
    async fn insert_with_generated_id(
        &self,
        ticket: &NewTicket,
        status: &str,
        lifecycle_status: LifecycleStatus,
        now: &str,
    ) -> Result<String, sqlx::Error> {
        const BASE_LEN: usize = 5;
        const MAX_EXTRA: usize = 5;

        let mut hash = generate_base36(BASE_LEN);

        for _ in 0..=MAX_EXTRA {
            let id = format!("{}-{}", ticket.project, hash);
            match self
                .insert_ticket(&id, ticket, status, lifecycle_status, now)
                .await
            {
                Ok(()) => return Ok(id),
                Err(sqlx::Error::Database(ref db_err)) if is_unique_violation(db_err.as_ref()) => {
                    hash.push(random_base36_char());
                }
                Err(e) => return Err(e),
            }
        }

        Err(sqlx::Error::Protocol(
            "failed to generate unique ticket ID after maximum retries".into(),
        ))
    }

    async fn insert_ticket(
        &self,
        id: &str,
        ticket: &NewTicket,
        status: &str,
        lifecycle_status: LifecycleStatus,
        now: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO ticket (id, project, type, status, lifecycle_status, priority, parent_id, title, body, branch, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(id)
        .bind(&ticket.project)
        .bind(&ticket.type_)
        .bind(status)
        .bind(lifecycle_status.as_str())
        .bind(ticket.priority)
        .bind(&ticket.parent_id)
        .bind(&ticket.title)
        .bind(&ticket.body)
        .bind(&ticket.branch)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fetch a single ticket by its ID. Returns `None` if no ticket exists with that ID.
    /// This is the primary lookup method for the GetTicket RPC.
    pub async fn get_ticket_by_id(&self, id: &str) -> Result<Option<Ticket>, sqlx::Error> {
        self.get_ticket(id).await
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
                bool,
                i32,
                Option<String>,
                String,
                String,
                Option<String>,
                String,
                String,
                i32,
                i32,
            ),
        >(
            "SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at, \
             (SELECT COUNT(*)::INT4 FROM ticket c WHERE c.parent_id = ticket.id AND c.status = 'closed') AS children_completed, \
             (SELECT COUNT(*)::INT4 FROM ticket c WHERE c.parent_id = ticket.id) AS children_total \
             FROM ticket WHERE id = $1",
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
                lifecycle_managed,
                priority,
                parent_id,
                title,
                body,
                branch,
                created_at,
                updated_at,
                children_completed,
                children_total,
            )| {
                Ticket {
                    id,
                    project,
                    type_,
                    status,
                    lifecycle_status: lifecycle_status_str
                        .parse::<LifecycleStatus>()
                        .unwrap_or_default(),
                    lifecycle_managed,
                    priority,
                    parent_id,
                    title,
                    body,
                    branch,
                    created_at,
                    updated_at,
                    children_completed,
                    children_total,
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
        let lifecycle_managed = update
            .lifecycle_managed
            .unwrap_or(existing.lifecycle_managed);
        let type_ = update.type_.as_deref().unwrap_or(&existing.type_);
        let priority = update.priority.unwrap_or(existing.priority);
        let title = update.title.as_deref().unwrap_or(&existing.title);
        let body = update.body.as_deref().unwrap_or(&existing.body);
        let project = update.project.as_deref().unwrap_or(&existing.project);
        let parent_id = match &update.parent_id {
            Some(p) => p.as_deref(),
            None => existing.parent_id.as_deref(),
        };
        let branch = match &update.branch {
            Some(b) => b.as_deref(),
            None => existing.branch.as_deref(),
        };
        let now = Utc::now().to_rfc3339();

        // When the project changes, re-key the ticket ID to match the new prefix.
        let project_changed = project != existing.project;
        let new_id = if project_changed {
            let hash = extract_hash(id);
            self.rekey_ticket_id(id, project, &hash).await?
        } else {
            id.to_owned()
        };

        sqlx::query(
            "UPDATE ticket SET id = $1, type = $2, status = $3, lifecycle_status = $4, lifecycle_managed = $5, priority = $6, title = $7, body = $8, parent_id = $9, branch = $10, project = $11, updated_at = $12
             WHERE id = $13",
        )
        .bind(&new_id)
        .bind(type_)
        .bind(status)
        .bind(lifecycle_status.as_str())
        .bind(lifecycle_managed)
        .bind(priority)
        .bind(title)
        .bind(body)
        .bind(parent_id)
        .bind(branch)
        .bind(project)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(Ticket {
            id: new_id,
            project: project.to_owned(),
            type_: type_.to_owned(),
            status: status.to_owned(),
            lifecycle_status,
            lifecycle_managed,
            priority,
            parent_id: parent_id.map(|s| s.to_owned()),
            title: title.to_owned(),
            body: body.to_owned(),
            branch: branch.map(|s| s.to_owned()),
            created_at: existing.created_at,
            updated_at: now,
            children_completed: 0,
            children_total: 0,
        })
    }

    /// Re-key a ticket's ID from the old project prefix to a new one.
    /// Updates all foreign key references in a transaction, then returns
    /// the new ID so the caller can update the ticket row itself.
    async fn rekey_ticket_id(
        &self,
        old_id: &str,
        new_project: &str,
        hash: &str,
    ) -> Result<String, sqlx::Error> {
        const MAX_EXTRA: usize = 5;
        let mut candidate_hash = hash.to_owned();

        for _ in 0..=MAX_EXTRA {
            let new_id = format!("{new_project}-{candidate_hash}");

            // Check for collision with an existing ticket.
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ticket WHERE id = $1)")
                    .bind(&new_id)
                    .fetch_one(&self.pool)
                    .await?;

            if exists {
                candidate_hash.push(random_base36_char());
                continue;
            }

            // Update all tables that reference ticket.id via foreign keys.
            self.update_ticket_references(old_id, &new_id).await?;
            return Ok(new_id);
        }

        Err(sqlx::Error::Protocol(
            "failed to generate unique ticket ID after maximum retries".into(),
        ))
    }

    /// Update all foreign key references from `old_id` to `new_id` across
    /// every table that references `ticket(id)`.
    async fn update_ticket_references(
        &self,
        old_id: &str,
        new_id: &str,
    ) -> Result<(), sqlx::Error> {
        // ticket.parent_id (children pointing to this ticket as parent)
        sqlx::query("UPDATE ticket SET parent_id = $1 WHERE parent_id = $2")
            .bind(new_id)
            .bind(old_id)
            .execute(&self.pool)
            .await?;

        // edge
        sqlx::query("UPDATE edge SET source_id = $1 WHERE source_id = $2")
            .bind(new_id)
            .bind(old_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE edge SET target_id = $1 WHERE target_id = $2")
            .bind(new_id)
            .bind(old_id)
            .execute(&self.pool)
            .await?;

        // activity
        sqlx::query("UPDATE activity SET ticket_id = $1 WHERE ticket_id = $2")
            .bind(new_id)
            .bind(old_id)
            .execute(&self.pool)
            .await?;

        // meta (entity_type = 'ticket')
        sqlx::query(
            "UPDATE meta SET entity_id = $1 WHERE entity_id = $2 AND entity_type = 'ticket'",
        )
        .bind(new_id)
        .bind(old_id)
        .execute(&self.pool)
        .await?;

        // ticket_comments
        sqlx::query("UPDATE ticket_comments SET ticket_id = $1 WHERE ticket_id = $2")
            .bind(new_id)
            .bind(old_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Paginated ticket query with total_count.
    ///
    /// - `page_size`: when `None`, returns all matching results (no LIMIT/OFFSET).
    /// - `offset`: number of rows to skip (only used when `page_size` is `Some`).
    /// - `include_children`: when `false`, adds `WHERE parent_id IS NULL` to return
    ///   only top-level tickets; when `true`, returns tickets at all levels.
    ///
    /// Returns `(tickets, total_count)` where `total_count` is the total number of
    /// matching rows before pagination.
    pub async fn list_tickets_paginated(
        &self,
        filter: &TicketFilter,
        page_size: Option<i32>,
        offset: i32,
        include_children: bool,
    ) -> Result<(Vec<Ticket>, i32), sqlx::Error> {
        let (where_clause, binds) = Self::build_ticket_filter_clause(filter, include_children);

        let total_count = self.count_filtered_tickets(&where_clause, &binds).await?;

        let tickets = self
            .fetch_filtered_tickets(&where_clause, &binds, page_size, offset)
            .await?;

        Ok((tickets, total_count))
    }

    /// Build the WHERE clause and bind values for ticket filters.
    fn build_ticket_filter_clause(
        filter: &TicketFilter,
        include_children: bool,
    ) -> (String, Vec<String>) {
        let mut where_clause = String::from(" WHERE 1=1");
        let mut binds: Vec<String> = Vec::new();
        let mut param_idx = 0usize;

        if !include_children && filter.parent_id.is_none() {
            where_clause.push_str(" AND parent_id IS NULL");
        }
        if let Some(ref project) = filter.project {
            param_idx += 1;
            where_clause.push_str(&format!(" AND project = ${param_idx}"));
            binds.push(project.clone());
        }
        if !filter.statuses.is_empty() {
            let placeholders: Vec<String> = filter
                .statuses
                .iter()
                .map(|_| {
                    param_idx += 1;
                    format!("${param_idx}")
                })
                .collect();
            where_clause.push_str(&format!(" AND status IN ({})", placeholders.join(",")));
            binds.extend(filter.statuses.iter().cloned());
        }
        if let Some(ref type_) = filter.type_ {
            param_idx += 1;
            where_clause.push_str(&format!(" AND type = ${param_idx}"));
            binds.push(type_.clone());
        }
        if let Some(ref parent_id) = filter.parent_id {
            param_idx += 1;
            where_clause.push_str(&format!(" AND parent_id = ${param_idx}"));
            binds.push(parent_id.clone());
        }
        if let Some(ref lifecycle_status) = filter.lifecycle_status {
            param_idx += 1;
            where_clause.push_str(&format!(" AND lifecycle_status = ${param_idx}"));
            binds.push(lifecycle_status.as_str().to_owned());
        }

        let _ = param_idx;
        (where_clause, binds)
    }

    /// Count the total matching rows for a given WHERE clause.
    async fn count_filtered_tickets(
        &self,
        where_clause: &str,
        binds: &[String],
    ) -> Result<i32, sqlx::Error> {
        let count_query = format!("SELECT COUNT(*)::INT4 FROM ticket{where_clause}");
        let mut q = sqlx::query_scalar::<_, i32>(sqlx::AssertSqlSafe(count_query));
        for bind in binds {
            q = q.bind(bind);
        }
        q.fetch_one(&self.pool).await
    }

    /// Fetch ticket rows for a given WHERE clause with optional pagination.
    async fn fetch_filtered_tickets(
        &self,
        where_clause: &str,
        binds: &[String],
        page_size: Option<i32>,
        offset: i32,
    ) -> Result<Vec<Ticket>, sqlx::Error> {
        let mut query = format!(
            "SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at, \
             (SELECT COUNT(*)::INT4 FROM ticket c WHERE c.parent_id = ticket.id AND c.status = 'closed') AS children_completed, \
             (SELECT COUNT(*)::INT4 FROM ticket c WHERE c.parent_id = ticket.id) AS children_total \
             FROM ticket{where_clause} ORDER BY priority ASC, created_at ASC"
        );

        if let Some(limit) = page_size {
            query.push_str(&format!(" LIMIT {limit} OFFSET {offset}"));
        }

        let mut q = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                String,
                bool,
                i32,
                Option<String>,
                String,
                String,
                Option<String>,
                String,
                String,
                i32,
                i32,
            ),
        >(sqlx::AssertSqlSafe(query));
        for bind in binds {
            q = q.bind(bind);
        }

        let rows = q.fetch_all(&self.pool).await?;
        Ok(Self::rows_to_tickets(rows))
    }

    #[allow(clippy::type_complexity)]
    fn rows_to_tickets(
        rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            bool,
            i32,
            Option<String>,
            String,
            String,
            Option<String>,
            String,
            String,
            i32,
            i32,
        )>,
    ) -> Vec<Ticket> {
        rows.into_iter()
            .map(
                |(
                    id,
                    project,
                    type_,
                    status,
                    lifecycle_status_str,
                    lifecycle_managed,
                    priority,
                    parent_id,
                    title,
                    body,
                    branch,
                    created_at,
                    updated_at,
                    children_completed,
                    children_total,
                )| {
                    Ticket {
                        id,
                        project,
                        type_,
                        status,
                        lifecycle_status: lifecycle_status_str
                            .parse::<LifecycleStatus>()
                            .unwrap_or_default(),
                        lifecycle_managed,
                        priority,
                        parent_id,
                        title,
                        body,
                        branch,
                        created_at,
                        updated_at,
                        children_completed,
                        children_total,
                    }
                },
            )
            .collect()
    }

    pub async fn list_tickets(&self, filter: &TicketFilter) -> Result<Vec<Ticket>, sqlx::Error> {
        let (query, binds) = Self::build_list_tickets_query(filter);

        let mut q = sqlx::query_as::<_, TicketRow>(sqlx::AssertSqlSafe(query));
        for bind in &binds {
            q = q.bind(bind);
        }

        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(ticket_from_row).collect())
    }

    fn build_list_tickets_query(filter: &TicketFilter) -> (String, Vec<String>) {
        let mut query = String::from(
            "SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at, \
             (SELECT COUNT(*)::INT4 FROM ticket c WHERE c.parent_id = ticket.id AND c.status = 'closed') AS children_completed, \
             (SELECT COUNT(*)::INT4 FROM ticket c WHERE c.parent_id = ticket.id) AS children_total \
             FROM ticket WHERE 1=1",
        );
        let mut binds: Vec<String> = Vec::new();
        let mut param_idx = 0usize;

        if let Some(ref project) = filter.project {
            param_idx += 1;
            query.push_str(&format!(" AND project = ${param_idx}"));
            binds.push(project.clone());
        }
        if !filter.statuses.is_empty() {
            let placeholders: Vec<String> = filter
                .statuses
                .iter()
                .map(|_| {
                    param_idx += 1;
                    format!("${param_idx}")
                })
                .collect();
            query.push_str(&format!(" AND status IN ({})", placeholders.join(",")));
            binds.extend(filter.statuses.iter().cloned());
        }
        if let Some(ref type_) = filter.type_ {
            param_idx += 1;
            query.push_str(&format!(" AND type = ${param_idx}"));
            binds.push(type_.clone());
        }
        if let Some(ref parent_id) = filter.parent_id {
            param_idx += 1;
            query.push_str(&format!(" AND parent_id = ${param_idx}"));
            binds.push(parent_id.clone());
        }
        if let Some(ref lifecycle_status) = filter.lifecycle_status {
            param_idx += 1;
            query.push_str(&format!(" AND lifecycle_status = ${param_idx}"));
            binds.push(lifecycle_status.as_str().to_owned());
        }

        let _ = param_idx;
        query.push_str(" ORDER BY priority ASC, created_at ASC");

        (query, binds)
    }

    /// List a ticket tree: root ticket + all descendants via recursive CTE.
    /// Returns tickets paired with their depth in the tree (0 = root).
    /// An optional status filter applies to descendants only (root is always included).
    pub async fn list_ticket_tree(
        &self,
        root_id: &str,
        status_filter: Option<&str>,
    ) -> Result<Vec<(Ticket, i32)>, sqlx::Error> {
        if let Some(status) = status_filter {
            self.list_ticket_tree_with_status(root_id, status).await
        } else {
            self.list_ticket_tree_unfiltered(root_id).await
        }
    }

    async fn list_ticket_tree_unfiltered(
        &self,
        root_id: &str,
    ) -> Result<Vec<(Ticket, i32)>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
                String, String, String, String, String, bool, i32,
                Option<String>, String, String, Option<String>,
                String, String, i32,
            ),
        >(
            "WITH RECURSIVE tree(id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at, depth) AS (
                SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at, 0
                FROM ticket WHERE id = $1
                UNION ALL
                SELECT t.id, t.project, t.type, t.status, t.lifecycle_status, t.lifecycle_managed, t.priority, t.parent_id, t.title, t.body, t.branch, t.created_at, t.updated_at, tree.depth + 1
                FROM ticket t JOIN tree ON t.parent_id = tree.id
            )
            SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at, depth
            FROM tree ORDER BY depth ASC, priority ASC, created_at ASC",
        )
        .bind(root_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(Self::rows_to_ticket_depth(rows))
    }

    async fn list_ticket_tree_with_status(
        &self,
        root_id: &str,
        status: &str,
    ) -> Result<Vec<(Ticket, i32)>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
                String, String, String, String, String, bool, i32,
                Option<String>, String, String, Option<String>,
                String, String, i32,
            ),
        >(
            "WITH RECURSIVE tree(id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at, depth) AS (
                SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at, 0
                FROM ticket WHERE id = $1
                UNION ALL
                SELECT t.id, t.project, t.type, t.status, t.lifecycle_status, t.lifecycle_managed, t.priority, t.parent_id, t.title, t.body, t.branch, t.created_at, t.updated_at, tree.depth + 1
                FROM ticket t JOIN tree ON t.parent_id = tree.id
                WHERE t.status = $2
            )
            SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at, depth
            FROM tree ORDER BY depth ASC, priority ASC, created_at ASC",
        )
        .bind(root_id)
        .bind(status)
        .fetch_all(&self.pool)
        .await?;

        Ok(Self::rows_to_ticket_depth(rows))
    }

    #[allow(clippy::type_complexity)]
    fn rows_to_ticket_depth(
        rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            bool,
            i32,
            Option<String>,
            String,
            String,
            Option<String>,
            String,
            String,
            i32,
        )>,
    ) -> Vec<(Ticket, i32)> {
        rows.into_iter()
            .map(
                |(
                    id,
                    project,
                    type_,
                    status,
                    lifecycle_status_str,
                    lifecycle_managed,
                    priority,
                    parent_id,
                    title,
                    body,
                    branch,
                    created_at,
                    updated_at,
                    depth,
                )| {
                    (
                        Ticket {
                            id,
                            project,
                            type_,
                            status,
                            lifecycle_status: lifecycle_status_str
                                .parse::<LifecycleStatus>()
                                .unwrap_or_default(),
                            lifecycle_managed,
                            priority,
                            parent_id,
                            title,
                            body,
                            branch,
                            created_at,
                            updated_at,
                            children_completed: 0,
                            children_total: 0,
                        },
                        depth,
                    )
                },
            )
            .collect()
    }

    pub async fn set_meta(
        &self,
        entity_id: &str,
        entity_type: &str,
        key: &str,
        value: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO meta (entity_id, entity_type, key, value) VALUES ($1, $2, $3, $4)
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
            "SELECT key, value FROM meta WHERE entity_id = $1 AND entity_type = $2",
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
        sqlx::query("INSERT INTO edge (source_id, target_id, kind) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
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
        sqlx::query("DELETE FROM edge WHERE source_id = $1 AND target_id = $2 AND kind = $3")
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
                     WHERE (source_id = $1 OR target_id = $2) AND kind = $3",
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
                     WHERE source_id = $1 OR target_id = $2",
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
            "INSERT INTO activity (id, ticket_id, timestamp, author, message) VALUES ($1, $2, $3, $4, $5)",
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

    pub async fn get_activities_by_author(
        &self,
        ticket_id: &str,
        author: &str,
    ) -> Result<Vec<Activity>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT id, ticket_id, timestamp, author, message FROM activity
             WHERE ticket_id = $1 AND author = $2 ORDER BY timestamp ASC",
        )
        .bind(ticket_id)
        .bind(author)
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

    pub async fn get_activities(&self, ticket_id: &str) -> Result<Vec<Activity>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT id, ticket_id, timestamp, author, message FROM activity
             WHERE ticket_id = $1 ORDER BY timestamp ASC",
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
             WHERE m.key = $1 AND m.value = $2
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
             WHERE m.key = $1
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
        let placeholders: String = blockers
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT COUNT(*)::INT4 FROM ticket WHERE id IN ({placeholders}) AND status != 'closed'"
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
                "WITH RECURSIVE descendants(id, title, priority, type) AS (
                    SELECT id, title, priority, type FROM ticket WHERE parent_id = $1
                    UNION ALL
                    SELECT t.id, t.title, t.priority, t.type
                    FROM ticket t JOIN descendants d ON t.parent_id = d.id
                )
                SELECT id, title, priority, type FROM descendants
                WHERE id IN (SELECT id FROM ticket WHERE status = 'open' AND project = $2)
                ORDER BY priority ASC"
                    .to_owned(),
                vec![epic_id.to_owned(), p.to_owned()],
            ),
            None => (
                "WITH RECURSIVE descendants(id, title, priority, type) AS (
                    SELECT id, title, priority, type FROM ticket WHERE parent_id = $1
                    UNION ALL
                    SELECT t.id, t.title, t.priority, t.type
                    FROM ticket t JOIN descendants d ON t.parent_id = d.id
                )
                SELECT id, title, priority, type FROM descendants
                WHERE id IN (SELECT id FROM ticket WHERE status = 'open')
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
        sqlx::query("DELETE FROM meta WHERE entity_id = $1 AND entity_type = $2 AND key = $3")
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
            "SELECT COUNT(*)::INT4 FROM ticket WHERE parent_id = $1 AND status != 'closed'",
        )
        .bind(epic_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(count == 0)
    }

    /// Close all open children of the given epic.
    pub async fn close_open_children(&self, epic_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE ticket SET status = 'closed' WHERE parent_id = $1 AND status != 'closed'",
        )
        .bind(epic_id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Return all tickets ordered by id for deterministic export.
    pub async fn export_tickets(&self) -> Result<Vec<crate::model::ExportTicket>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                String,
                bool,
                i32,
                Option<String>,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, \
             parent_id, title, body, branch, created_at, updated_at \
             FROM ticket ORDER BY id ASC",
        )
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
                    lifecycle_status,
                    lifecycle_managed,
                    priority,
                    parent_id,
                    title,
                    body,
                    branch,
                    created_at,
                    updated_at,
                )| crate::model::ExportTicket {
                    id,
                    project,
                    type_,
                    status,
                    lifecycle_status,
                    lifecycle_managed,
                    priority,
                    parent_id,
                    title,
                    body,
                    branch,
                    created_at,
                    updated_at,
                },
            )
            .collect())
    }

    /// Return all edges ordered by (source_id, target_id, kind) for deterministic export.
    pub async fn export_edges(&self) -> Result<Vec<crate::model::ExportEdge>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            "SELECT source_id, target_id, kind FROM edge ORDER BY source_id ASC, target_id ASC, kind ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(source_id, target_id, kind)| crate::model::ExportEdge {
                source_id,
                target_id,
                kind,
            })
            .collect())
    }

    /// Return all meta rows ordered by (entity_type, entity_id, key) for deterministic export.
    pub async fn export_meta(&self) -> Result<Vec<crate::model::ExportMeta>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT entity_id, entity_type, key, value FROM meta \
             ORDER BY entity_type ASC, entity_id ASC, key ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(entity_id, entity_type, key, value)| crate::model::ExportMeta {
                    entity_id,
                    entity_type,
                    key,
                    value,
                },
            )
            .collect())
    }

    /// Return all activities ordered by (ticket_id, timestamp) for deterministic export.
    pub async fn export_activities(
        &self,
    ) -> Result<Vec<crate::model::ExportActivity>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT id, ticket_id, timestamp, author, message FROM activity \
             ORDER BY ticket_id ASC, timestamp ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, ticket_id, timestamp, author, message)| crate::model::ExportActivity {
                    id,
                    ticket_id,
                    timestamp,
                    author,
                    message,
                },
            )
            .collect())
    }

    pub async fn insert_ticket_comment(
        &self,
        comment_id: &str,
        ticket_id: &str,
        pr_number: i64,
        gh_repo: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO ticket_comments (comment_id, ticket_id, pr_number, gh_repo) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(comment_id)
        .bind(ticket_id)
        .bind(pr_number)
        .bind(gh_repo)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_pending_replies(
        &self,
    ) -> Result<Vec<crate::model::TicketComment>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, i64, String, bool, String)>(
            "SELECT comment_id, ticket_id, pr_number, gh_repo, reply_posted, created_at \
             FROM ticket_comments WHERE reply_posted = false \
             ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(comment_id, ticket_id, pr_number, gh_repo, reply_posted, created_at)| {
                    crate::model::TicketComment {
                        comment_id,
                        ticket_id,
                        pr_number,
                        gh_repo,
                        reply_posted,
                        created_at,
                    }
                },
            )
            .collect())
    }

    pub async fn mark_reply_posted(
        &self,
        comment_id: &str,
        ticket_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE ticket_comments SET reply_posted = true \
             WHERE comment_id = $1 AND ticket_id = $2",
        )
        .bind(comment_id)
        .bind(ticket_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return all ticket_comments rows ordered by (ticket_id, comment_id) for deterministic export.
    pub async fn export_ticket_comments(
        &self,
    ) -> Result<Vec<crate::model::ExportTicketComment>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, i64, String, bool, String)>(
            "SELECT comment_id, ticket_id, pr_number, gh_repo, reply_posted, created_at \
             FROM ticket_comments ORDER BY ticket_id ASC, comment_id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(comment_id, ticket_id, pr_number, gh_repo, reply_posted, created_at)| {
                    crate::model::ExportTicketComment {
                        comment_id,
                        ticket_id,
                        pr_number,
                        gh_repo,
                        reply_posted,
                        created_at,
                    }
                },
            )
            .collect())
    }

    /// Fetch multiple tickets by their IDs in a single query.
    ///
    /// Returns only tickets that exist. Callers should treat missing IDs as
    /// dropped (e.g., deleted while a workflow still references them).
    pub async fn get_tickets_by_ids(&self, ids: &[String]) -> Result<Vec<Ticket>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                String,
                bool,
                i32,
                Option<String>,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at
             FROM ticket
             WHERE id = ANY($1)
             ORDER BY priority ASC, created_at ASC",
        )
        .bind(ids)
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
                    lifecycle_managed,
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
                        lifecycle_managed,
                        priority,
                        parent_id,
                        title,
                        body,
                        branch,
                        created_at,
                        updated_at,
                        children_completed: 0,
                        children_total: 0,
                    }
                },
            )
            .collect())
    }

    /// Get the open and closed children counts for a ticket.
    /// Returns (open_count, closed_count).
    pub async fn get_ticket_children_counts(
        &self,
        ticket_id: &str,
    ) -> Result<(i64, i64), sqlx::Error> {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ticket WHERE parent_id = $1")
            .bind(ticket_id)
            .fetch_one(&self.pool)
            .await?;

        let closed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ticket WHERE parent_id = $1 AND status = 'closed'",
        )
        .bind(ticket_id)
        .fetch_one(&self.pool)
        .await?;

        Ok((total - closed, closed))
    }

    /// Import a batch of rows into the database within a single transaction.
    ///
    /// Tickets are inserted first, then edges, meta, activities, and
    /// ticket_comments.  If any ticket id already exists the transaction is
    /// rolled back and an error is returned listing the conflicting ids.
    ///
    /// Returns the total number of rows inserted across all tables.
    pub async fn import_records(
        &self,
        tickets: Vec<crate::model::ExportTicket>,
        edges: Vec<crate::model::ExportEdge>,
        meta: Vec<crate::model::ExportMeta>,
        activities: Vec<crate::model::ExportActivity>,
        comments: Vec<crate::model::ExportTicketComment>,
    ) -> Result<i64, ImportError> {
        self.check_ticket_id_collisions(&tickets).await?;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| ImportError::Db(e.to_string()))?;

        let count = insert_import_rows(&mut tx, tickets, edges, meta, activities, comments).await?;

        tx.commit()
            .await
            .map_err(|e| ImportError::Db(e.to_string()))?;

        Ok(count)
    }

    /// Check whether any of the incoming ticket ids already exist in the DB.
    async fn check_ticket_id_collisions(
        &self,
        tickets: &[crate::model::ExportTicket],
    ) -> Result<(), ImportError> {
        if tickets.is_empty() {
            return Ok(());
        }
        let ids: Vec<&str> = tickets.iter().map(|t| t.id.as_str()).collect();
        let existing: Vec<(String,)> = sqlx::query_as("SELECT id FROM ticket WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| ImportError::Db(e.to_string()))?;

        if existing.is_empty() {
            Ok(())
        } else {
            let conflicting: Vec<String> = existing.into_iter().map(|(id,)| id).collect();
            Err(ImportError::IdCollision(conflicting))
        }
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
                bool,
                i32,
                Option<String>,
                String,
                String,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT id, project, type, status, lifecycle_status, lifecycle_managed, priority, parent_id, title, body, branch, created_at, updated_at
             FROM ticket
             WHERE lifecycle_status = $1
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
                    lifecycle_managed,
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
                        lifecycle_managed,
                        priority,
                        parent_id,
                        title,
                        body,
                        branch,
                        created_at,
                        updated_at,
                        children_completed: 0,
                        children_total: 0,
                    }
                },
            )
            .collect())
    }
}

/// Insert tickets, edges, meta, activities, and comments into a transaction.
///
/// Returns the total number of rows inserted.
async fn insert_import_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tickets: Vec<crate::model::ExportTicket>,
    edges: Vec<crate::model::ExportEdge>,
    meta: Vec<crate::model::ExportMeta>,
    activities: Vec<crate::model::ExportActivity>,
    comments: Vec<crate::model::ExportTicketComment>,
) -> Result<i64, ImportError> {
    let mut count: i64 = 0;
    count += insert_tickets(tx, tickets).await?;
    count += insert_edges(tx, edges).await?;
    count += insert_meta(tx, meta).await?;
    count += insert_activities(tx, activities).await?;
    count += insert_ticket_comments(tx, comments).await?;
    Ok(count)
}

async fn insert_tickets(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tickets: Vec<crate::model::ExportTicket>,
) -> Result<i64, ImportError> {
    for t in &tickets {
        sqlx::query(
            "INSERT INTO ticket \
             (id, project, type, status, lifecycle_status, lifecycle_managed, priority, \
              parent_id, title, body, branch, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(&t.id)
        .bind(&t.project)
        .bind(&t.type_)
        .bind(&t.status)
        .bind(&t.lifecycle_status)
        .bind(t.lifecycle_managed)
        .bind(t.priority)
        .bind(&t.parent_id)
        .bind(&t.title)
        .bind(&t.body)
        .bind(&t.branch)
        .bind(&t.created_at)
        .bind(&t.updated_at)
        .execute(&mut **tx)
        .await
        .map_err(|e| ImportError::Db(e.to_string()))?;
    }
    Ok(tickets.len() as i64)
}

async fn insert_edges(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    edges: Vec<crate::model::ExportEdge>,
) -> Result<i64, ImportError> {
    for e in &edges {
        sqlx::query(
            "INSERT INTO edge (source_id, target_id, kind) VALUES ($1,$2,$3) \
             ON CONFLICT DO NOTHING",
        )
        .bind(&e.source_id)
        .bind(&e.target_id)
        .bind(&e.kind)
        .execute(&mut **tx)
        .await
        .map_err(|e| ImportError::Db(e.to_string()))?;
    }
    Ok(edges.len() as i64)
}

async fn insert_meta(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    meta: Vec<crate::model::ExportMeta>,
) -> Result<i64, ImportError> {
    for m in &meta {
        sqlx::query(
            "INSERT INTO meta (entity_id, entity_type, key, value) VALUES ($1,$2,$3,$4) \
             ON CONFLICT DO NOTHING",
        )
        .bind(&m.entity_id)
        .bind(&m.entity_type)
        .bind(&m.key)
        .bind(&m.value)
        .execute(&mut **tx)
        .await
        .map_err(|e| ImportError::Db(e.to_string()))?;
    }
    Ok(meta.len() as i64)
}

async fn insert_activities(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    activities: Vec<crate::model::ExportActivity>,
) -> Result<i64, ImportError> {
    for a in &activities {
        sqlx::query(
            "INSERT INTO activity (id, ticket_id, timestamp, author, message) \
             VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
        )
        .bind(&a.id)
        .bind(&a.ticket_id)
        .bind(&a.timestamp)
        .bind(&a.author)
        .bind(&a.message)
        .execute(&mut **tx)
        .await
        .map_err(|e| ImportError::Db(e.to_string()))?;
    }
    Ok(activities.len() as i64)
}

async fn insert_ticket_comments(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    comments: Vec<crate::model::ExportTicketComment>,
) -> Result<i64, ImportError> {
    for c in &comments {
        sqlx::query(
            "INSERT INTO ticket_comments \
             (comment_id, ticket_id, pr_number, gh_repo, reply_posted, created_at) \
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
        )
        .bind(&c.comment_id)
        .bind(&c.ticket_id)
        .bind(c.pr_number)
        .bind(&c.gh_repo)
        .bind(c.reply_posted)
        .bind(&c.created_at)
        .execute(&mut **tx)
        .await
        .map_err(|e| ImportError::Db(e.to_string()))?;
    }
    Ok(comments.len() as i64)
}

const BASE36_CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

fn generate_base36(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| BASE36_CHARS[rng.gen_range(0..36)] as char)
        .collect()
}

fn random_base36_char() -> char {
    let mut rng = rand::thread_rng();
    BASE36_CHARS[rng.gen_range(0..36)] as char
}

/// Extract the hash portion from a ticket ID (everything after the first `-`).
fn extract_hash(id: &str) -> String {
    id.split_once('-').map(|x| x.1).unwrap_or(id).to_owned()
}

fn is_unique_violation(err: &dyn sqlx::error::DatabaseError) -> bool {
    // Postgres unique violation error code is 23505
    err.code().is_some_and(|code| code == "23505")
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
