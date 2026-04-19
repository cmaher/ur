// WorkflowRepo: CRUD operations for workflows, workflow events, intents, and comments.

use std::collections::HashMap;

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use ticket_db::{LifecycleStatus, MetadataMatchTicket, Ticket, TicketComment};

use crate::model::{Workflow, WorkflowEvent, WorkflowEventRow, WorkflowIntent};

/// A workflow with pre-joined ticket children counts, returned by paginated queries.
pub struct PaginatedWorkflow {
    pub workflow: Workflow,
    pub ticket_children_open: i64,
    pub ticket_children_closed: i64,
}

#[derive(Clone)]
pub struct WorkflowRepo {
    pool: PgPool,
}

impl WorkflowRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ============================================================
    // Workflow Event polling/deletion
    // ============================================================

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
        sqlx::query("DELETE FROM workflow_event WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Delete all workflow events for a given ticket (used by redrive to clear stale trigger events).
    pub async fn delete_workflow_events_for_ticket(
        &self,
        ticket_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM workflow_event WHERE ticket_id = $1")
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Increment the attempts counter on a workflow event (after a failed processing attempt).
    pub async fn increment_workflow_event_attempts(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow_event SET attempts = attempts + 1 WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // ============================================================
    // Workflow CRUD
    // ============================================================

    /// Create a new workflow for a ticket. Returns error if one already exists (ticket_id is UNIQUE).
    pub async fn create_workflow(
        &self,
        ticket_id: &str,
        status: LifecycleStatus,
    ) -> Result<Workflow, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO workflow (id, ticket_id, status, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(ticket_id)
        .bind(status.as_str())
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(Workflow {
            id,
            ticket_id: ticket_id.to_owned(),
            status,
            stalled: false,
            stall_reason: String::new(),
            implement_cycles: 0,
            worker_id: String::new(),
            noverify: false,
            feedback_mode: String::new(),
            ci_status: ur_rpc::workflow_condition::ci_status::PENDING.to_owned(),
            mergeable: ur_rpc::workflow_condition::mergeable::UNKNOWN.to_owned(),
            review_status: ur_rpc::workflow_condition::review_status::PENDING.to_owned(),
            created_at: now,
        })
    }

    /// Get the active (non-terminal) workflow for a ticket, if one exists.
    pub async fn get_workflow_by_ticket(
        &self,
        ticket_id: &str,
    ) -> Result<Option<Workflow>, sqlx::Error> {
        self.get_workflow_by_ticket_inner(ticket_id, true).await
    }

    /// Get the most recent workflow for a ticket regardless of status.
    pub async fn get_latest_workflow_by_ticket(
        &self,
        ticket_id: &str,
    ) -> Result<Option<Workflow>, sqlx::Error> {
        self.get_workflow_by_ticket_inner(ticket_id, false).await
    }

    async fn get_workflow_by_ticket_inner(
        &self,
        ticket_id: &str,
        active_only: bool,
    ) -> Result<Option<Workflow>, sqlx::Error> {
        let query = if active_only {
            "SELECT id, ticket_id, status, stalled, stall_reason, implement_cycles, worker_id, noverify, feedback_mode, ci_status, mergeable, review_status, created_at
             FROM workflow WHERE ticket_id = $1 AND status NOT IN ('done', 'cancelled')"
        } else {
            "SELECT id, ticket_id, status, stalled, stall_reason, implement_cycles, worker_id, noverify, feedback_mode, ci_status, mergeable, review_status, created_at
             FROM workflow WHERE ticket_id = $1 ORDER BY created_at DESC LIMIT 1"
        };
        let row = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                bool,
                String,
                i32,
                String,
                bool,
                String,
                String,
                String,
                String,
                String,
            ),
        >(query)
        .bind(ticket_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(row_to_workflow))
    }

    /// List all workflows, optionally filtered by status.
    pub async fn list_workflows(
        &self,
        status: Option<LifecycleStatus>,
    ) -> Result<Vec<Workflow>, sqlx::Error> {
        let rows = match &status {
            Some(s) => {
                sqlx::query_as::<
                    _,
                    (String, String, String, bool, String, i32, String, bool, String, String, String, String, String),
                >(
                    "SELECT id, ticket_id, status, stalled, stall_reason, implement_cycles, worker_id, noverify, feedback_mode, ci_status, mergeable, review_status, created_at
                     FROM workflow WHERE status = $1 ORDER BY created_at",
                )
                .bind(s.as_str())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<
                    _,
                    (String, String, String, bool, String, i32, String, bool, String, String, String, String, String),
                >(
                    "SELECT id, ticket_id, status, stalled, stall_reason, implement_cycles, worker_id, noverify, feedback_mode, ci_status, mergeable, review_status, created_at
                     FROM workflow WHERE status NOT IN ('done', 'cancelled') ORDER BY created_at",
                )
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows.into_iter().map(row_to_workflow).collect())
    }

    /// List workflows with pagination, optional status/project filters, and pre-joined
    /// ticket children counts. Returns (workflows, total_count).
    ///
    /// When `page_size` is `None`, all matching rows are returned.
    /// Project filtering JOINs on the ticket table via `workflow.ticket_id`.
    pub async fn list_workflows_paginated(
        &self,
        page_size: Option<i32>,
        offset: i32,
        status: Option<LifecycleStatus>,
        project: Option<&str>,
    ) -> Result<(Vec<PaginatedWorkflow>, i32), sqlx::Error> {
        let sql = build_paginated_sql(page_size, offset, status.as_ref(), project);
        let bind_values = build_paginated_binds(&status, project);

        // Safety: the SQL is built from trusted format strings with bind placeholders.
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.clone()));
        for val in &bind_values {
            query = query.bind(val);
        }

        use sqlx::Row as _;
        let rows = query.fetch_all(&self.pool).await?;

        let total_count: i32 = rows.first().map(|r| r.get("total_count")).unwrap_or(0);

        let workflows = rows.into_iter().map(|r| pg_row_to_paginated(&r)).collect();

        Ok((workflows, total_count))
    }

    /// Update the status of a workflow.
    pub async fn update_workflow_status(
        &self,
        ticket_id: &str,
        status: LifecycleStatus,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow SET status = $1 WHERE ticket_id = $2 AND status NOT IN ('done', 'cancelled')")
            .bind(status.as_str())
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Mark a workflow as stalled with a reason.
    pub async fn set_workflow_stalled(
        &self,
        ticket_id: &str,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow SET stalled = true, stall_reason = $1 WHERE ticket_id = $2 AND status NOT IN ('done', 'cancelled')")
            .bind(reason)
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Clear a workflow stall (reset stalled flag and reason).
    pub async fn clear_workflow_stall(&self, ticket_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow SET stalled = false, stall_reason = '' WHERE ticket_id = $1 AND status NOT IN ('done', 'cancelled')")
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Increment the implement_cycles counter on a workflow.
    pub async fn increment_implement_cycles(&self, ticket_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE workflow SET implement_cycles = implement_cycles + 1 WHERE ticket_id = $1 AND status NOT IN ('done', 'cancelled')",
        )
        .bind(ticket_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Reset the implement_cycles counter on a workflow to zero.
    pub async fn reset_implement_cycles(&self, ticket_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE workflow SET implement_cycles = 0 WHERE ticket_id = $1 AND status NOT IN ('done', 'cancelled')",
        )
        .bind(ticket_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Set the worker_id on a workflow.
    pub async fn set_workflow_worker_id(
        &self,
        ticket_id: &str,
        worker_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow SET worker_id = $1 WHERE ticket_id = $2 AND status NOT IN ('done', 'cancelled')")
            .bind(worker_id)
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Set the noverify flag on a workflow.
    pub async fn set_workflow_noverify(
        &self,
        ticket_id: &str,
        noverify: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow SET noverify = $1 WHERE ticket_id = $2 AND status NOT IN ('done', 'cancelled')")
            .bind(noverify)
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Set the feedback_mode on a workflow.
    pub async fn set_workflow_feedback_mode(
        &self,
        ticket_id: &str,
        feedback_mode: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow SET feedback_mode = $1 WHERE ticket_id = $2 AND status NOT IN ('done', 'cancelled')")
            .bind(feedback_mode)
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Update a single workflow condition (ci_status, mergeable, or review_status).
    ///
    /// Values should use constants from `ur_rpc::workflow_condition`.
    pub async fn update_workflow_condition(
        &self,
        ticket_id: &str,
        condition: ur_rpc::workflow_condition::WorkflowCondition,
        value: &str,
    ) -> Result<(), sqlx::Error> {
        let query = match condition {
            ur_rpc::workflow_condition::WorkflowCondition::CiStatus => {
                "UPDATE workflow SET ci_status = $1 WHERE ticket_id = $2 AND status NOT IN ('done', 'cancelled')"
            }
            ur_rpc::workflow_condition::WorkflowCondition::Mergeable => {
                "UPDATE workflow SET mergeable = $1 WHERE ticket_id = $2 AND status NOT IN ('done', 'cancelled')"
            }
            ur_rpc::workflow_condition::WorkflowCondition::ReviewStatus => {
                "UPDATE workflow SET review_status = $1 WHERE ticket_id = $2 AND status NOT IN ('done', 'cancelled')"
            }
        };

        sqlx::query(query)
            .bind(value)
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Initialize all three workflow conditions to their default values.
    /// Called when a workflow transitions to InReview.
    pub async fn initialize_workflow_conditions(&self, ticket_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE workflow SET ci_status = $1, mergeable = $2, review_status = $3 WHERE ticket_id = $4 AND status NOT IN ('done', 'cancelled')",
        )
        .bind(ur_rpc::workflow_condition::ci_status::PENDING)
        .bind(ur_rpc::workflow_condition::mergeable::UNKNOWN)
        .bind(ur_rpc::workflow_condition::review_status::PENDING)
        .bind(ticket_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Insert a workflow event into the workflow_events table.
    ///
    /// Records a `WorkflowEvent` variant with the current server timestamp
    /// for the given workflow.
    pub async fn insert_workflow_event(
        &self,
        workflow_id: &str,
        event: ur_rpc::workflow_event::WorkflowEvent,
    ) -> Result<(), sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO workflow_events (id, workflow_id, event, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(workflow_id)
        .bind(event.as_str())
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Insert a workflow event with a custom timestamp.
    ///
    /// Like `insert_workflow_event`, but uses the provided `created_at` instead
    /// of the current server time. Used for CI events where the GitHub API
    /// `completed_at` timestamp is the authoritative event time.
    pub async fn insert_workflow_event_at(
        &self,
        workflow_id: &str,
        event: ur_rpc::workflow_event::WorkflowEvent,
        created_at: &str,
    ) -> Result<(), sqlx::Error> {
        let id = Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO workflow_events (id, workflow_id, event, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(workflow_id)
        .bind(event.as_str())
        .bind(created_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get all workflow events for a given workflow, ordered by created_at ASC.
    pub async fn get_workflow_events(
        &self,
        workflow_id: &str,
    ) -> Result<Vec<WorkflowEventRow>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT event, created_at FROM workflow_events WHERE workflow_id = $1 ORDER BY created_at ASC",
        )
        .bind(workflow_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(event, created_at)| WorkflowEventRow { event, created_at })
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

    // ============================================================
    // WorkflowIntent CRUD
    // ============================================================

    /// Create a new workflow intent for a ticket.
    pub async fn create_intent(
        &self,
        ticket_id: &str,
        target_status: LifecycleStatus,
    ) -> Result<WorkflowIntent, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO workflow_intent (id, ticket_id, target_status, created_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(ticket_id)
        .bind(target_status.as_str())
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(WorkflowIntent {
            id,
            ticket_id: ticket_id.to_owned(),
            target_status,
            created_at: now,
        })
    }

    /// Poll the oldest unprocessed workflow intent.
    /// Returns `None` if no intents are pending.
    pub async fn poll_intent(&self) -> Result<Option<WorkflowIntent>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT id, ticket_id, target_status, created_at
             FROM workflow_intent
             ORDER BY created_at ASC
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(id, ticket_id, target_status_str, created_at)| WorkflowIntent {
                id,
                ticket_id,
                target_status: target_status_str
                    .parse::<LifecycleStatus>()
                    .unwrap_or_default(),
                created_at,
            },
        ))
    }

    /// List all workflow intents, ordered by creation time (oldest first).
    /// Used for startup recovery to re-spawn handlers for incomplete intents.
    pub async fn list_intents(&self) -> Result<Vec<WorkflowIntent>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT id, ticket_id, target_status, created_at
             FROM workflow_intent
             ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, ticket_id, target_status_str, created_at)| WorkflowIntent {
                    id,
                    ticket_id,
                    target_status: target_status_str
                        .parse::<LifecycleStatus>()
                        .unwrap_or_default(),
                    created_at,
                },
            )
            .collect())
    }

    /// Delete a workflow intent by ID (after successful processing).
    pub async fn delete_intent(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM workflow_intent WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Delete all workflow intents for a ticket (used when cancelling a workflow).
    pub async fn delete_intents_for_ticket(&self, ticket_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM workflow_intent WHERE ticket_id = $1")
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Batch-query active (non-terminal) workflows for a set of ticket IDs.
    /// Returns a map from ticket_id to the workflow lifecycle status string.
    /// Avoids N+1 queries when enriching ticket lists with dispatch status.
    pub async fn get_active_workflows_by_ticket_ids(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        // Fetch all active workflows in a single query and filter client-side.
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT ticket_id, status FROM workflow WHERE status NOT IN ('done', 'cancelled')",
        )
        .fetch_all(&self.pool)
        .await?;

        let id_set: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();

        Ok(rows
            .into_iter()
            .filter(|(tid, _)| id_set.contains(tid.as_str()))
            .collect())
    }

    /// Return all tickets that have a workflow with the given status.
    /// Used by GithubPoller to find tickets in pushing/in_review workflow states.
    pub async fn tickets_by_workflow_status(
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
            "SELECT t.id, t.project, t.type, t.status, t.lifecycle_status, t.lifecycle_managed, t.priority, t.parent_id, t.title, t.body, t.branch, t.created_at, t.updated_at
             FROM ticket t
             INNER JOIN workflow w ON w.ticket_id = t.id
             WHERE w.status = $1
             ORDER BY t.priority ASC, t.created_at ASC",
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

    /// Return all tickets that have a workflow with the given worker_id.
    /// Used to look up which ticket is assigned to a specific worker.
    pub async fn tickets_by_workflow_worker_id(
        &self,
        worker_id: &str,
    ) -> Result<Vec<MetadataMatchTicket>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT t.id, t.title, t.type, t.status
             FROM ticket t
             INNER JOIN workflow w ON w.ticket_id = t.id
             WHERE w.worker_id = $1
             ORDER BY t.priority ASC",
        )
        .bind(worker_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, title, type_, status)| MetadataMatchTicket {
                id,
                title,
                type_,
                status,
                key: "worker_id".to_string(),
                value: worker_id.to_string(),
            })
            .collect())
    }

    // ============================================================
    // WorkflowComments CRUD
    // ============================================================

    /// Bulk-insert comment IDs as seen for a ticket. Existing rows are ignored.
    pub async fn insert_workflow_comments(
        &self,
        ticket_id: &str,
        comment_ids: &[String],
    ) -> Result<(), sqlx::Error> {
        for comment_id in comment_ids {
            sqlx::query(
                "INSERT INTO workflow_comments (ticket_id, comment_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(ticket_id)
            .bind(comment_id)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Return comment IDs where feedback has not yet been created.
    pub async fn get_pending_feedback_comments(
        &self,
        ticket_id: &str,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT comment_id FROM workflow_comments
             WHERE ticket_id = $1 AND feedback_created = false
             ORDER BY created_at ASC",
        )
        .bind(ticket_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Mark the given comment IDs as having had feedback tickets created.
    pub async fn mark_feedback_created(
        &self,
        ticket_id: &str,
        comment_ids: &[String],
    ) -> Result<(), sqlx::Error> {
        for comment_id in comment_ids {
            sqlx::query(
                "UPDATE workflow_comments SET feedback_created = true
                 WHERE ticket_id = $1 AND comment_id = $2",
            )
            .bind(ticket_id)
            .bind(comment_id)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Return all comment IDs that have been seen for a ticket.
    pub async fn get_seen_comment_ids(&self, ticket_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT comment_id FROM workflow_comments
             WHERE ticket_id = $1
             ORDER BY created_at ASC",
        )
        .bind(ticket_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    // ============================================================
    // TicketComments CRUD
    // ============================================================

    /// Insert a ticket comment linking a PR comment to a ticket for reply tracking.
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

    /// Return all ticket comments where `reply_posted = 0`.
    pub async fn get_pending_replies(&self) -> Result<Vec<TicketComment>, sqlx::Error> {
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
                    TicketComment {
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

    /// Mark a ticket comment as having had its reply posted.
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
}

fn build_paginated_sql(
    page_size: Option<i32>,
    offset: i32,
    status: Option<&LifecycleStatus>,
    project: Option<&str>,
) -> String {
    let mut conditions = Vec::new();
    let mut param_idx = 0usize;

    if status.is_some() {
        param_idx += 1;
        conditions.push(format!("w.status = ${param_idx}"));
    } else {
        conditions.push("w.status NOT IN ('done', 'cancelled')".to_string());
    }

    let ticket_join = if project.is_some() {
        param_idx += 1;
        conditions.push(format!("t.project = ${param_idx}"));
        "INNER JOIN ticket t ON t.id = w.ticket_id"
    } else {
        ""
    };

    let _ = param_idx;

    let where_clause = format!("WHERE {}", conditions.join(" AND "));

    let limit_clause = match page_size {
        Some(ps) => format!("LIMIT {} OFFSET {}", ps, offset),
        None => String::new(),
    };

    format!(
        "SELECT w.id, w.ticket_id, w.status, w.stalled, w.stall_reason, \
         w.implement_cycles, w.worker_id, w.noverify, w.feedback_mode, \
         w.ci_status, w.mergeable, w.review_status, w.created_at, \
         (COUNT(*) OVER())::INT4 AS total_count, \
         COALESCE(tc.children_closed, 0)::INT8 AS children_closed, \
         (COALESCE(tc.children_total, 0) - COALESCE(tc.children_closed, 0))::INT8 AS children_open \
         FROM workflow w \
         {ticket_join} \
         LEFT JOIN ( \
           SELECT parent_id, \
             SUM(CASE WHEN status = 'closed' THEN 1 ELSE 0 END)::INT8 AS children_closed, \
             COUNT(*)::INT8 AS children_total \
           FROM ticket WHERE parent_id != '' GROUP BY parent_id \
         ) tc ON tc.parent_id = w.ticket_id \
         {where_clause} \
         ORDER BY w.created_at \
         {limit_clause}"
    )
}

fn build_paginated_binds(status: &Option<LifecycleStatus>, project: Option<&str>) -> Vec<String> {
    let mut binds = Vec::new();
    if let Some(s) = status {
        binds.push(s.as_str().to_string());
    }
    if let Some(p) = project {
        binds.push(p.to_string());
    }
    binds
}

fn pg_row_to_paginated(r: &sqlx::postgres::PgRow) -> PaginatedWorkflow {
    use sqlx::Row as _;
    let workflow = Workflow {
        id: r.get("id"),
        ticket_id: r.get("ticket_id"),
        status: r
            .get::<String, _>("status")
            .parse::<LifecycleStatus>()
            .unwrap_or_default(),
        stalled: r.get("stalled"),
        stall_reason: r.get("stall_reason"),
        implement_cycles: r.get("implement_cycles"),
        worker_id: r.get("worker_id"),
        noverify: r.get("noverify"),
        feedback_mode: r.get("feedback_mode"),
        ci_status: r.get("ci_status"),
        mergeable: r.get("mergeable"),
        review_status: r.get("review_status"),
        created_at: r.get("created_at"),
    };
    PaginatedWorkflow {
        workflow,
        ticket_children_open: r.get("children_open"),
        ticket_children_closed: r.get("children_closed"),
    }
}

#[allow(clippy::type_complexity)]
fn row_to_workflow(
    (
        id,
        ticket_id,
        status_str,
        stalled,
        stall_reason,
        implement_cycles,
        worker_id,
        noverify,
        feedback_mode,
        ci_status,
        mergeable,
        review_status,
        created_at,
    ): (
        String,
        String,
        String,
        bool,
        String,
        i32,
        String,
        bool,
        String,
        String,
        String,
        String,
        String,
    ),
) -> Workflow {
    Workflow {
        id,
        ticket_id,
        status: status_str.parse::<LifecycleStatus>().unwrap_or_default(),
        stalled,
        stall_reason,
        implement_cycles,
        worker_id,
        noverify,
        feedback_mode,
        ci_status,
        mergeable,
        review_status,
        created_at,
    }
}
