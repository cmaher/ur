// WorkflowRepo: CRUD operations for workflows, workflow events, intents, and comments.

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::model::{
    LifecycleStatus, MetadataMatchTicket, Ticket, Workflow, WorkflowEvent, WorkflowIntent,
};

#[derive(Clone)]
pub struct WorkflowRepo {
    pool: SqlitePool,
}

impl WorkflowRepo {
    pub fn new(pool: SqlitePool) -> Self {
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
        sqlx::query("DELETE FROM workflow_event WHERE id = ?")
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
        sqlx::query("DELETE FROM workflow_event WHERE ticket_id = ?")
            .bind(ticket_id)
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

        sqlx::query("INSERT INTO workflow (id, ticket_id, status, created_at) VALUES (?, ?, ?, ?)")
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
             FROM workflow WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')"
        } else {
            "SELECT id, ticket_id, status, stalled, stall_reason, implement_cycles, worker_id, noverify, feedback_mode, ci_status, mergeable, review_status, created_at
             FROM workflow WHERE ticket_id = ? ORDER BY created_at DESC LIMIT 1"
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
                     FROM workflow WHERE status = ? ORDER BY created_at",
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

    /// Update the status of a workflow.
    pub async fn update_workflow_status(
        &self,
        ticket_id: &str,
        status: LifecycleStatus,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow SET status = ? WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')")
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
        sqlx::query("UPDATE workflow SET stalled = 1, stall_reason = ? WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')")
            .bind(reason)
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Clear a workflow stall (reset stalled flag and reason).
    pub async fn clear_workflow_stall(&self, ticket_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow SET stalled = 0, stall_reason = '' WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')")
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Increment the implement_cycles counter on a workflow.
    pub async fn increment_implement_cycles(&self, ticket_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE workflow SET implement_cycles = implement_cycles + 1 WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')",
        )
        .bind(ticket_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Reset the implement_cycles counter on a workflow to zero.
    pub async fn reset_implement_cycles(&self, ticket_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE workflow SET implement_cycles = 0 WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')",
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
        sqlx::query("UPDATE workflow SET worker_id = ? WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')")
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
        sqlx::query("UPDATE workflow SET noverify = ? WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')")
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
        sqlx::query("UPDATE workflow SET feedback_mode = ? WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')")
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
                "UPDATE workflow SET ci_status = ? WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')"
            }
            ur_rpc::workflow_condition::WorkflowCondition::Mergeable => {
                "UPDATE workflow SET mergeable = ? WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')"
            }
            ur_rpc::workflow_condition::WorkflowCondition::ReviewStatus => {
                "UPDATE workflow SET review_status = ? WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')"
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
            "UPDATE workflow SET ci_status = ?, mergeable = ?, review_status = ? WHERE ticket_id = ? AND status NOT IN ('done', 'cancelled')",
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
            "INSERT INTO workflow_events (id, workflow_id, event, created_at) VALUES (?, ?, ?, ?)",
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
            "INSERT INTO workflow_events (id, workflow_id, event, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(workflow_id)
        .bind(event.as_str())
        .bind(created_at)
        .execute(&self.pool)
        .await?;

        Ok(())
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
             VALUES (?, ?, ?, ?)",
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
        sqlx::query("DELETE FROM workflow_intent WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Delete all workflow intents for a ticket (used when cancelling a workflow).
    pub async fn delete_intents_for_ticket(&self, ticket_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM workflow_intent WHERE ticket_id = ?")
            .bind(ticket_id)
            .execute(&self.pool)
            .await?;

        Ok(())
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
             WHERE w.status = ?
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
             WHERE w.worker_id = ?
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
                "INSERT OR IGNORE INTO workflow_comments (ticket_id, comment_id) VALUES (?, ?)",
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
             WHERE ticket_id = ? AND feedback_created = 0
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
                "UPDATE workflow_comments SET feedback_created = 1
                 WHERE ticket_id = ? AND comment_id = ?",
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
             WHERE ticket_id = ?
             ORDER BY created_at ASC",
        )
        .bind(ticket_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
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
