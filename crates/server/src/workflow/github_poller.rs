use std::time::Duration;

use remote_repo::{CheckRun, GhBackend, RemoteRepo};
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

use ur_db::TicketRepo;
use ur_db::WorkflowRepo;
use ur_db::model::{LifecycleStatus, Ticket, Workflow};
use ur_rpc::proto::builder::BuilderdClient;
use ur_rpc::stream::CompletedExec;
use ur_rpc::workflow_condition;
use ur_rpc::workflow_event::WorkflowEvent;

use super::TransitionRequest;
use super::ticket_client::{self, TicketClient};

/// Delay between individual GitHub API calls to avoid rate limiting.
const API_CALL_DELAY: Duration = Duration::from_secs(2);

/// Constants for GitHub check run status and conclusion values.
mod check_run {
    /// Check run status values (from GitHub REST and GraphQL APIs).
    pub mod status {
        pub const COMPLETED: &str = "completed";
    }

    /// Check run conclusion values (from GitHub REST and GraphQL APIs).
    pub mod conclusion {
        pub const SUCCESS: &str = "success";
        pub const FAILURE: &str = "failure";
        pub const NEUTRAL: &str = "neutral";
        pub const SKIPPED: &str = "skipped";
    }
}

/// Polls GitHub for CI status, mergeability, and review signals on tickets
/// in the `in_review` lifecycle state.
///
/// Runs as a separate tokio task from the workflow engine. Sends
/// transition requests to the WorkflowCoordinator via an mpsc channel
/// instead of directly updating lifecycle_status in the database.
#[derive(Clone)]
pub struct GithubPollerManager {
    ticket_repo: TicketRepo,
    workflow_repo: WorkflowRepo,
    builderd_client: BuilderdClient,
    scan_interval: Duration,
    transition_tx: mpsc::Sender<TransitionRequest>,
    ticket_client: TicketClient,
}

impl GithubPollerManager {
    pub fn new(
        ticket_repo: TicketRepo,
        workflow_repo: WorkflowRepo,
        builderd_client: BuilderdClient,
        scan_interval: Duration,
        transition_tx: mpsc::Sender<TransitionRequest>,
        ticket_client: TicketClient,
    ) -> Self {
        Self {
            ticket_repo,
            workflow_repo,
            builderd_client,
            scan_interval,
            transition_tx,
            ticket_client,
        }
    }

    /// Spawn the polling loop as a background tokio task.
    pub fn spawn(self, shutdown_rx: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(self.run(shutdown_rx))
    }

    async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        info!("github poller started");
        loop {
            self.poll_once().await;

            tokio::select! {
                _ = tokio::time::sleep(self.scan_interval) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("github poller shutting down");
                        return;
                    }
                }
            }
        }
    }

    /// Run one full scan: check all in_review tickets.
    async fn poll_once(&self) {
        match self
            .workflow_repo
            .tickets_by_workflow_status(LifecycleStatus::InReview)
            .await
        {
            Ok(tickets) => {
                for ticket in &tickets {
                    self.check_in_review_ticket(ticket).await;
                    tokio::time::sleep(API_CALL_DELAY).await;
                }
            }
            Err(e) => {
                error!(error = %e, "failed to query in_review workflows");
            }
        }
    }

    fn extract_pr_context(
        &self,
        ticket_id: &str,
        meta: &std::collections::HashMap<String, String>,
    ) -> Option<(i64, String)> {
        let pr_number_str = meta.get("pr_number")?;

        let pr_number: i64 = match pr_number_str.parse() {
            Ok(n) => n,
            Err(e) => {
                warn!(
                    ticket_id = %ticket_id,
                    pr_number = %pr_number_str,
                    error = %e,
                    "invalid pr_number metadata — cannot parse as integer"
                );
                return None;
            }
        };

        let gh_repo = match meta.get("gh_repo") {
            Some(r) => r.clone(),
            None => {
                warn!(
                    ticket_id = %ticket_id,
                    "no gh_repo metadata"
                );
                return None;
            }
        };

        Some((pr_number, gh_repo))
    }

    async fn set_feedback_mode_and_transition(
        &self,
        ticket_id: &str,
        feedback_mode: &str,
        target: LifecycleStatus,
    ) {
        if let Err(e) = self
            .workflow_repo
            .set_workflow_feedback_mode(ticket_id, feedback_mode)
            .await
        {
            error!(ticket_id = %ticket_id, error = %e, "failed to set workflow feedback_mode");
            return;
        }
        self.send_transition(ticket_id, target).await;
    }

    async fn check_in_review_ticket(&self, ticket: &Ticket) {
        let meta = match self.ticket_repo.get_meta(&ticket.id, "ticket").await {
            Ok(m) => m,
            Err(e) => {
                warn!(ticket_id = %ticket.id, error = %e, "failed to get ticket metadata");
                return;
            }
        };

        let Some((pr_number, gh_repo)) = self.extract_pr_context(&ticket.id, &meta) else {
            return;
        };

        // Check for autoapprove meta — if present, auto-advance without waiting.
        if meta.contains_key("autoapprove") {
            info!(
                ticket_id = %ticket.id,
                pr_number = pr_number,
                "autoapprove set — transitioning to feedback_creating with feedback_mode=later"
            );
            self.set_feedback_mode_and_transition(
                &ticket.id,
                ur_rpc::feedback_mode::LATER,
                LifecycleStatus::FeedbackCreating,
            )
            .await;
            return;
        }

        let backend = GhBackend {
            client: self.builderd_client.clone(),
            gh_repo: gh_repo.clone(),
        };

        let workflow = match self.workflow_repo.get_workflow_by_ticket(&ticket.id).await {
            Ok(Some(w)) => w,
            Ok(None) => {
                warn!(ticket_id = %ticket.id, "no active workflow found for in_review ticket");
                return;
            }
            Err(e) => {
                warn!(ticket_id = %ticket.id, error = %e, "failed to get workflow");
                return;
            }
        };

        info!(
            ticket_id = %ticket.id,
            pr_number = pr_number,
            "checking conditions for in_review ticket"
        );

        // Step 1: Check CI status and update condition.
        let ci_result = self
            .poll_ci_condition(&ticket.id, &workflow, &backend, pr_number)
            .await;

        // Step 2: Check mergeability and update condition.
        let mergeable_result = self
            .poll_mergeable_condition(&ticket.id, &workflow, &backend, pr_number)
            .await;

        // Step 3: Check review signals and update condition.
        let review_result = self
            .poll_review_condition(&ticket.id, &workflow, &backend, pr_number, &gh_repo)
            .await;

        // Step 4: Create failure tickets for CI failure and merge conflicts.
        let mut has_failures = false;
        if ci_result == workflow_condition::ci_status::FAILED {
            let failing_checks = collect_failing_checks(&backend, pr_number).await;
            if let Err(e) = self
                .ticket_client
                .create_workflow_issue_ticket(
                    &ticket.id,
                    ticket_client::issue_type::CI_FAILURE,
                    &format!("CI failure on PR #{pr_number}"),
                    &failing_checks,
                )
                .await
            {
                error!(ticket_id = %ticket.id, error = %e, "failed to create CI failure ticket");
            }
            has_failures = true;
        }

        if mergeable_result == workflow_condition::mergeable::CONFLICT {
            if let Err(e) = self
                .ticket_client
                .create_workflow_issue_ticket(
                    &ticket.id,
                    ticket_client::issue_type::MERGE_CONFLICT,
                    &format!("Merge conflict on PR #{pr_number}"),
                    "The PR has merge conflicts that must be resolved.",
                )
                .await
            {
                error!(ticket_id = %ticket.id, error = %e, "failed to create merge conflict ticket");
            }
            has_failures = true;
        }

        // Step 5: Evaluate transition.
        self.evaluate_transition(
            &ticket.id,
            &workflow.id,
            &backend,
            pr_number,
            ci_result,
            mergeable_result,
            &review_result,
            has_failures,
        )
        .await;
    }

    /// Poll CI status, update the condition column, and emit events on change.
    /// Returns the new ci_status value.
    async fn poll_ci_condition(
        &self,
        ticket_id: &str,
        workflow: &Workflow,
        backend: &GhBackend,
        pr_number: i64,
    ) -> &'static str {
        let (new_status, ci_completed_at) = match check_ci_status(backend, pr_number).await {
            Ok(result) => result,
            Err(e) => {
                warn!(ticket_id = %ticket_id, error = %e, "failed to check CI status");
                return to_ci_status_const(&workflow.ci_status);
            }
        };

        if new_status != workflow.ci_status {
            if let Err(e) = self
                .workflow_repo
                .update_workflow_condition(
                    ticket_id,
                    workflow_condition::WorkflowCondition::CiStatus,
                    new_status,
                )
                .await
            {
                error!(ticket_id = %ticket_id, error = %e, "failed to update ci_status");
                return to_ci_status_const(&workflow.ci_status);
            }

            let event = match new_status {
                s if s == workflow_condition::ci_status::SUCCEEDED => {
                    Some(WorkflowEvent::CiSucceeded)
                }
                s if s == workflow_condition::ci_status::FAILED => Some(WorkflowEvent::CiFailed),
                _ => None,
            };

            if let Some(event) = event {
                self.emit_workflow_event_at(&workflow.id, event, &ci_completed_at)
                    .await;
            }
        }

        new_status
    }

    /// Poll mergeability, update the condition column, and emit events on change.
    /// Returns the new mergeable value.
    async fn poll_mergeable_condition(
        &self,
        ticket_id: &str,
        workflow: &Workflow,
        backend: &GhBackend,
        pr_number: i64,
    ) -> &'static str {
        let new_status = backend.check_mergeable(pr_number).await;

        if new_status != workflow.mergeable {
            if let Err(e) = self
                .workflow_repo
                .update_workflow_condition(
                    ticket_id,
                    workflow_condition::WorkflowCondition::Mergeable,
                    new_status,
                )
                .await
            {
                error!(ticket_id = %ticket_id, error = %e, "failed to update mergeable");
                return to_mergeable_const(&workflow.mergeable);
            }

            if new_status == workflow_condition::mergeable::CONFLICT {
                self.emit_workflow_event(&workflow.id, WorkflowEvent::MergeConflictDetected)
                    .await;
            }
        }

        new_status
    }

    /// Poll review signals, update the condition column, and emit events on change.
    /// Returns the new review_status value and the review check result.
    async fn poll_review_condition(
        &self,
        ticket_id: &str,
        workflow: &Workflow,
        backend: &GhBackend,
        pr_number: i64,
        gh_repo: &str,
    ) -> ReviewResult {
        let current_status = to_review_status_const(&workflow.review_status);

        let seen_comment_ids = match self.workflow_repo.get_seen_comment_ids(ticket_id).await {
            Ok(ids) => ids,
            Err(e) => {
                warn!(ticket_id = %ticket_id, error = %e, "failed to get seen comment IDs");
                return ReviewResult {
                    status: current_status,
                    signal: ReviewSignal::Pending,
                    unseen_count: 0,
                };
            }
        };

        let signal_result = check_review_signal(
            backend,
            &self.builderd_client,
            gh_repo,
            pr_number,
            &seen_comment_ids,
        )
        .await;

        let result = match signal_result {
            Ok(r) => r,
            Err(e) => {
                warn!(ticket_id = %ticket_id, error = %e, "failed to check review signal");
                return ReviewResult {
                    status: current_status,
                    signal: ReviewSignal::Pending,
                    unseen_count: 0,
                };
            }
        };

        let new_status = match result.signal {
            ReviewSignal::Approve => workflow_condition::review_status::APPROVED,
            ReviewSignal::RequestChanges => workflow_condition::review_status::CHANGES_REQUESTED,
            ReviewSignal::Merged | ReviewSignal::Closed | ReviewSignal::Pending => {
                // Merged/Closed are handled separately in evaluate_transition.
                // Pending means no change.
                return ReviewResult {
                    status: current_status,
                    signal: result.signal,
                    unseen_count: result.unseen_count,
                };
            }
        };

        if new_status != workflow.review_status {
            if let Err(e) = self
                .workflow_repo
                .update_workflow_condition(
                    ticket_id,
                    workflow_condition::WorkflowCondition::ReviewStatus,
                    new_status,
                )
                .await
            {
                error!(ticket_id = %ticket_id, error = %e, "failed to update review_status");
                return ReviewResult {
                    status: current_status,
                    signal: result.signal,
                    unseen_count: result.unseen_count,
                };
            }

            let event = match new_status {
                s if s == workflow_condition::review_status::APPROVED => {
                    Some(WorkflowEvent::ReviewApproved)
                }
                s if s == workflow_condition::review_status::CHANGES_REQUESTED => {
                    Some(WorkflowEvent::ReviewChangesRequested)
                }
                _ => None,
            };

            if let Some(event) = event {
                self.emit_workflow_event(&workflow.id, event).await;
            }
        }

        ReviewResult {
            status: new_status,
            signal: result.signal,
            unseen_count: result.unseen_count,
        }
    }

    /// Evaluate the combined condition state and decide on a transition.
    #[allow(clippy::too_many_arguments)]
    async fn evaluate_transition(
        &self,
        ticket_id: &str,
        workflow_id: &str,
        backend: &GhBackend,
        pr_number: i64,
        ci_status: &str,
        mergeable: &str,
        review_result: &ReviewResult,
        has_failures: bool,
    ) {
        // Handle PR merged/closed signals first — these override condition-based logic.
        match review_result.signal {
            ReviewSignal::Merged => {
                self.handle_manual_merge(ticket_id, workflow_id, backend, pr_number)
                    .await;
                return;
            }
            ReviewSignal::Closed => {
                info!(
                    ticket_id = %ticket_id,
                    pr_number = %pr_number,
                    "PR closed without merge — deleting workflow and reverting ticket to open"
                );
                self.cancel_workflow_and_revert(ticket_id).await;
                return;
            }
            _ => {}
        }

        // Changes requested takes priority — create failure tickets first, then go to FeedbackCreating.
        if review_result.status == workflow_condition::review_status::CHANGES_REQUESTED {
            if let Err(e) = self.workflow_repo.reset_implement_cycles(ticket_id).await {
                warn!(ticket_id = %ticket_id, error = %e, "failed to reset implement_cycles");
            }
            self.record_comments_and_transition(
                ticket_id,
                backend,
                pr_number,
                ur_rpc::feedback_mode::NOW,
                LifecycleStatus::FeedbackCreating,
                "changes requested — transitioning to feedback_creating (mode=now)",
            )
            .await;
            return;
        }

        // Failures (CI or merge conflict) without review feedback → Implementing.
        if has_failures {
            info!(
                ticket_id = %ticket_id,
                pr_number = %pr_number,
                "failures detected (ci={ci_status}, mergeable={mergeable}) — transitioning to implementing"
            );
            self.send_transition(ticket_id, LifecycleStatus::Implementing)
                .await;
            return;
        }

        // All green: approved + CI succeeded + mergeable → FeedbackCreating (mode=later).
        let is_approved = review_result.status == workflow_condition::review_status::APPROVED;
        let ci_succeeded = ci_status == workflow_condition::ci_status::SUCCEEDED;
        let is_mergeable = mergeable == workflow_condition::mergeable::MERGEABLE;

        if is_approved && ci_succeeded && is_mergeable {
            // If approve-only (1 or fewer unseen comments), skip feedback → merging.
            if review_result.unseen_count <= 1 {
                self.record_comments_and_transition(
                    ticket_id,
                    backend,
                    pr_number,
                    ur_rpc::feedback_mode::LATER,
                    LifecycleStatus::Merging,
                    "approval (approve-only) + all green — skipping feedback, transitioning to merging",
                )
                .await;
            } else {
                self.record_comments_and_transition(
                    ticket_id,
                    backend,
                    pr_number,
                    ur_rpc::feedback_mode::LATER,
                    LifecycleStatus::FeedbackCreating,
                    "approval + all green — transitioning to feedback_creating (mode=later)",
                )
                .await;
            }
        }

        // Otherwise, stay in InReview — conditions not yet met.
    }

    /// Handle a manually-merged PR: treat as approval with the same short-circuit
    /// logic as `ur approve` (skip feedback if no unseen comments to address).
    async fn handle_manual_merge(
        &self,
        ticket_id: &str,
        workflow_id: &str,
        backend: &GhBackend,
        pr_number: i64,
    ) {
        info!(
            ticket_id = %ticket_id,
            pr_number = %pr_number,
            "PR merged by human — treating as approval"
        );

        // Set all three conditions to passing — the merge proves they were satisfied.
        for (condition, value) in [
            (
                workflow_condition::WorkflowCondition::ReviewStatus,
                workflow_condition::review_status::APPROVED,
            ),
            (
                workflow_condition::WorkflowCondition::CiStatus,
                workflow_condition::ci_status::SUCCEEDED,
            ),
            (
                workflow_condition::WorkflowCondition::Mergeable,
                workflow_condition::mergeable::MERGEABLE,
            ),
        ] {
            if let Err(e) = self
                .workflow_repo
                .update_workflow_condition(ticket_id, condition, value)
                .await
            {
                error!(
                    ticket_id = %ticket_id,
                    error = %e,
                    "failed to update workflow condition on manual merge"
                );
            }
        }
        self.emit_workflow_event(workflow_id, WorkflowEvent::ReviewApproved)
            .await;

        // Count unseen comments to decide whether to short-circuit feedback.
        let unseen_count = self
            .count_unseen_comments(ticket_id, backend, pr_number)
            .await;

        if unseen_count <= 1 {
            self.record_comments_and_transition(
                ticket_id,
                backend,
                pr_number,
                ur_rpc::feedback_mode::LATER,
                LifecycleStatus::Merging,
                "manual merge (approve-only) — skipping feedback, transitioning to merging",
            )
            .await;
        } else {
            self.record_comments_and_transition(
                ticket_id,
                backend,
                pr_number,
                ur_rpc::feedback_mode::LATER,
                LifecycleStatus::FeedbackCreating,
                "manual merge with unseen comments — transitioning to feedback_creating (mode=later)",
            )
            .await;
        }
    }

    /// Count conversation comments on a PR that haven't been seen by the workflow yet.
    async fn count_unseen_comments(
        &self,
        ticket_id: &str,
        backend: &GhBackend,
        pr_number: i64,
    ) -> usize {
        let seen_ids = match self.workflow_repo.get_seen_comment_ids(ticket_id).await {
            Ok(ids) => ids,
            Err(e) => {
                warn!(ticket_id = %ticket_id, error = %e, "failed to get seen comment IDs");
                return 0;
            }
        };
        let comments = match backend.get_conversation_comments(pr_number).await {
            Ok(c) => c,
            Err(e) => {
                warn!(ticket_id = %ticket_id, error = %e, "failed to fetch comments for unseen count");
                return 0;
            }
        };
        comments
            .iter()
            .filter(|c| !seen_ids.contains(&c.id.to_string()))
            .count()
    }

    /// Fetch all current comment IDs, insert them as seen, then trigger a transition.
    async fn record_comments_and_transition(
        &self,
        ticket_id: &str,
        backend: &GhBackend,
        pr_number: i64,
        feedback_mode: &str,
        target: LifecycleStatus,
        log_message: &str,
    ) {
        info!(ticket_id = %ticket_id, pr_number = %pr_number, "{}", log_message);

        // Fetch all current comment IDs and record them as seen before
        // triggering the transition, so subsequent polls won't re-trigger.
        let comment_ids = match backend.get_conversation_comments(pr_number).await {
            Ok(comments) => comments
                .iter()
                .map(|c| c.id.to_string())
                .collect::<Vec<_>>(),
            Err(e) => {
                warn!(
                    ticket_id = %ticket_id,
                    error = %e,
                    "failed to fetch comments for dedup recording — proceeding with transition"
                );
                Vec::new()
            }
        };
        if let Err(e) = self
            .workflow_repo
            .insert_workflow_comments(ticket_id, &comment_ids)
            .await
        {
            error!(
                ticket_id = %ticket_id,
                error = %e,
                "failed to insert workflow comments"
            );
        }

        self.set_feedback_mode_and_transition(ticket_id, feedback_mode, target)
            .await;
    }

    /// Send a transition request to the WorkflowCoordinator.
    async fn send_transition(&self, ticket_id: &str, to: LifecycleStatus) {
        let request = TransitionRequest {
            ticket_id: ticket_id.to_owned(),
            target_status: to,
        };
        if let Err(e) = self.transition_tx.send(request).await {
            error!(
                ticket_id = %ticket_id,
                target = %to,
                error = %e,
                "failed to send transition request to coordinator"
            );
        }
    }

    /// Cancel the workflow for a ticket and revert the ticket status to open.
    /// Used when a PR is closed without merge.
    async fn cancel_workflow_and_revert(&self, ticket_id: &str) {
        if let Err(e) = self
            .workflow_repo
            .update_workflow_status(ticket_id, LifecycleStatus::Cancelled)
            .await
        {
            error!(
                ticket_id = %ticket_id,
                error = %e,
                "failed to cancel workflow for closed PR"
            );
            return;
        }
        let update = ur_db::model::TicketUpdate {
            status: Some("open".to_string()),
            ..Default::default()
        };
        if let Err(e) = self.ticket_repo.update_ticket(ticket_id, &update).await {
            error!(
                ticket_id = %ticket_id,
                error = %e,
                "failed to revert ticket to open after PR close"
            );
        }
    }

    /// Emit a workflow event with a custom timestamp (for CI events).
    async fn emit_workflow_event_at(
        &self,
        workflow_id: &str,
        event: WorkflowEvent,
        created_at: &str,
    ) {
        // Use the custom timestamp if non-empty, otherwise fall back to server time.
        let result = if created_at.is_empty() {
            self.workflow_repo
                .insert_workflow_event(workflow_id, event)
                .await
        } else {
            self.workflow_repo
                .insert_workflow_event_at(workflow_id, event, created_at)
                .await
        };
        if let Err(e) = result {
            error!(
                workflow_id = %workflow_id,
                event = %event,
                error = %e,
                "failed to insert workflow event"
            );
        }
    }

    /// Emit a workflow event with the current server timestamp.
    async fn emit_workflow_event(&self, workflow_id: &str, event: WorkflowEvent) {
        if let Err(e) = self
            .workflow_repo
            .insert_workflow_event(workflow_id, event)
            .await
        {
            error!(
                workflow_id = %workflow_id,
                event = %event,
                error = %e,
                "failed to insert workflow event"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Map a ci_status string from the database to its `&'static str` constant.
fn to_ci_status_const(value: &str) -> &'static str {
    match value {
        s if s == workflow_condition::ci_status::SUCCEEDED => {
            workflow_condition::ci_status::SUCCEEDED
        }
        s if s == workflow_condition::ci_status::FAILED => workflow_condition::ci_status::FAILED,
        _ => workflow_condition::ci_status::PENDING,
    }
}

/// Map a mergeable string from the database to its `&'static str` constant.
fn to_mergeable_const(value: &str) -> &'static str {
    match value {
        s if s == workflow_condition::mergeable::MERGEABLE => {
            workflow_condition::mergeable::MERGEABLE
        }
        s if s == workflow_condition::mergeable::CONFLICT => {
            workflow_condition::mergeable::CONFLICT
        }
        _ => workflow_condition::mergeable::UNKNOWN,
    }
}

/// Map a review_status string from the database to its `&'static str` constant.
fn to_review_status_const(value: &str) -> &'static str {
    match value {
        s if s == workflow_condition::review_status::APPROVED => {
            workflow_condition::review_status::APPROVED
        }
        s if s == workflow_condition::review_status::CHANGES_REQUESTED => {
            workflow_condition::review_status::CHANGES_REQUESTED
        }
        _ => workflow_condition::review_status::PENDING,
    }
}

/// Aggregated result of the review condition poll.
struct ReviewResult {
    /// The current review_status value (may be unchanged from workflow).
    status: &'static str,
    /// The raw review signal detected this cycle.
    signal: ReviewSignal,
    /// Number of unseen comments on the PR.
    unseen_count: usize,
}

// ---------------------------------------------------------------------------
// GitHub API helpers (via GhBackend through builderd)
// ---------------------------------------------------------------------------

/// Result of checking CI runs: the condition value and the latest completed_at timestamp.
struct CiCheckResult {
    status: &'static str,
    /// The latest `completed_at` timestamp from check runs, for event recording.
    completed_at: String,
}

/// Check CI status for a PR via `GhBackend::check_runs`.
/// Returns the condition value and the latest completed_at timestamp.
async fn check_ci_status(
    backend: &GhBackend,
    pr_number: i64,
) -> Result<(&'static str, String), anyhow::Error> {
    let runs = backend.check_runs(pr_number).await?;

    if runs.is_empty() {
        return Ok((workflow_condition::ci_status::SUCCEEDED, String::new()));
    }

    let result = evaluate_ci_runs(&runs);
    Ok((result.status, result.completed_at))
}

/// Evaluate check runs and produce a CI condition result.
fn evaluate_ci_runs(runs: &[CheckRun]) -> CiCheckResult {
    let mut all_completed = true;
    let mut any_failed = false;
    let mut latest_completed_at = String::new();

    for run in runs {
        let status = run.status.as_str();
        let conclusion = run.conclusion.as_str();

        let is_completed = is_check_completed(status, conclusion);

        if !is_completed {
            all_completed = false;
        } else {
            if is_check_failed(conclusion) {
                any_failed = true;
            }
            if !run.completed_at.is_empty() && run.completed_at > latest_completed_at {
                latest_completed_at.clone_from(&run.completed_at);
            }
        }
    }

    let status = if any_failed {
        workflow_condition::ci_status::FAILED
    } else if all_completed {
        workflow_condition::ci_status::SUCCEEDED
    } else {
        workflow_condition::ci_status::PENDING
    };

    CiCheckResult {
        status,
        completed_at: latest_completed_at,
    }
}

/// Determine whether a check run is completed based on its status and conclusion fields.
fn is_check_completed(status: &str, conclusion: &str) -> bool {
    status.is_empty()
        || status.eq_ignore_ascii_case(check_run::status::COMPLETED)
        || status.eq_ignore_ascii_case(check_run::conclusion::SUCCESS)
        || status.eq_ignore_ascii_case(check_run::conclusion::FAILURE)
        || status.eq_ignore_ascii_case(check_run::conclusion::NEUTRAL)
        || status.eq_ignore_ascii_case(check_run::conclusion::SKIPPED)
        || !conclusion.is_empty()
}

/// Determine whether a completed check run has failed.
fn is_check_failed(conclusion: &str) -> bool {
    !conclusion.is_empty()
        && !conclusion.eq_ignore_ascii_case(check_run::conclusion::SUCCESS)
        && !conclusion.eq_ignore_ascii_case(check_run::conclusion::SKIPPED)
        && !conclusion.eq_ignore_ascii_case(check_run::conclusion::NEUTRAL)
}

/// Collect a summary string of failing check runs for ticket body.
async fn collect_failing_checks(backend: &GhBackend, pr_number: i64) -> String {
    let runs = match backend.check_runs(pr_number).await {
        Ok(r) => r,
        Err(e) => return format!("(failed to fetch check runs: {e})"),
    };

    let mut failures: Vec<String> = Vec::new();
    for run in &runs {
        if is_check_failed(&run.conclusion) {
            failures.push(format!("{}: {}", run.name, run.conclusion));
        }
    }

    if failures.is_empty() {
        "CI failed (no specific failing checks identified)".to_string()
    } else {
        format!("Failing checks:\n{}", failures.join("\n"))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReviewSignal {
    /// `ur approve` — ship it
    Approve,
    /// `ur respond` — changes requested
    RequestChanges,
    /// PR was merged (by a human, not by us)
    Merged,
    /// PR was closed without merge
    Closed,
    /// No actionable signal yet
    Pending,
}

/// Result of checking a PR for review signals.
struct ReviewCheckResult {
    signal: ReviewSignal,
    /// Number of unseen (not-yet-recorded) comments on the PR.
    unseen_count: usize,
}

/// Check for review signals on a PR: latest unseen comment, merge status, close status.
async fn check_review_signal(
    backend: &GhBackend,
    builderd_client: &BuilderdClient,
    gh_repo: &str,
    pr_number: i64,
    seen_comment_ids: &[String],
) -> Result<ReviewCheckResult, anyhow::Error> {
    // First, check PR state (merged/closed).
    let pr = backend.get_pr(pr_number).await?;

    if pr.state == "closed" || pr.state == "CLOSED" || pr.state == "MERGED" {
        let signal =
            determine_closed_pr_signal(&pr.state, builderd_client, gh_repo, pr_number).await?;
        return Ok(ReviewCheckResult {
            signal,
            unseen_count: 0,
        });
    }

    // PR is still open — check latest unseen comment for review command.
    check_open_pr_comments(
        backend,
        builderd_client,
        gh_repo,
        pr_number,
        seen_comment_ids,
    )
    .await
}

/// Determine the signal for a closed/merged PR.
async fn determine_closed_pr_signal(
    state: &str,
    builderd_client: &BuilderdClient,
    gh_repo: &str,
    pr_number: i64,
) -> Result<ReviewSignal, anyhow::Error> {
    if state == "MERGED" {
        return Ok(ReviewSignal::Merged);
    }
    // Use REST API to distinguish merged vs closed-without-merge.
    let endpoint = format!("repos/{gh_repo}/pulls/{pr_number}");
    let completed = exec_gh_via_builderd(builderd_client, &["api", &endpoint]).await?;
    let completed = completed
        .check()
        .map_err(|e| anyhow::anyhow!("gh api PR state failed: {e}"))?;
    let pr_json: serde_json::Value = serde_json::from_str(&completed.stdout_text())
        .map_err(|e| anyhow::anyhow!("failed to parse PR JSON: {e}"))?;
    let merged = pr_json
        .get("merged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(if merged {
        ReviewSignal::Merged
    } else {
        ReviewSignal::Closed
    })
}

/// Check unseen comments on an open PR for review commands.
async fn check_open_pr_comments(
    backend: &GhBackend,
    builderd_client: &BuilderdClient,
    gh_repo: &str,
    pr_number: i64,
    seen_comment_ids: &[String],
) -> Result<ReviewCheckResult, anyhow::Error> {
    let comments = backend.get_conversation_comments(pr_number).await?;

    let unseen: Vec<_> = comments
        .iter()
        .filter(|c| !seen_comment_ids.contains(&c.id.to_string()))
        .collect();

    let unseen_count = unseen.len();

    let latest_comment = match unseen.last() {
        Some(c) => *c,
        None => {
            return Ok(ReviewCheckResult {
                signal: ReviewSignal::Pending,
                unseen_count: 0,
            });
        }
    };

    let comment_body = &latest_comment.body;
    let comment_created_at = &latest_comment.created_at;

    // Check if there are commits after the latest comment.
    let commits_endpoint =
        format!("repos/{gh_repo}/pulls/{pr_number}/commits?per_page=1&sort=created&direction=desc");
    let commits_result = exec_gh_via_builderd(builderd_client, &["api", &commits_endpoint]).await;

    if let Ok(completed) = commits_result
        && completed.exit_code == 0
        && has_commits_after_comment(completed.stdout_text().as_bytes(), comment_created_at)
    {
        return Ok(ReviewCheckResult {
            signal: ReviewSignal::Pending,
            unseen_count,
        });
    }

    let trimmed = comment_body.trim();
    let signal = parse_review_command(trimmed).unwrap_or(ReviewSignal::Pending);
    Ok(ReviewCheckResult {
        signal,
        unseen_count,
    })
}

/// Check if there are commits after a given comment timestamp.
fn has_commits_after_comment(commits_stdout: &[u8], comment_created_at: &str) -> bool {
    let commits: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(commits_stdout))
        .unwrap_or(serde_json::Value::Array(vec![]));

    let latest_commit = commits.as_array().and_then(|arr| arr.last());

    let Some(latest_commit) = latest_commit else {
        return false;
    };

    let commit_date = latest_commit
        .get("commit")
        .and_then(|c| c.get("committer"))
        .and_then(|c| c.get("date"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    !comment_created_at.is_empty() && !commit_date.is_empty() && commit_date > comment_created_at
}

/// Parse a PR comment as a review command.
/// The comment must be exactly the command text (after trimming whitespace).
/// Returns `None` if the comment is not a recognized command.
fn parse_review_command(text: &str) -> Option<ReviewSignal> {
    match text {
        "ur approve" => Some(ReviewSignal::Approve),
        "ur respond" => Some(ReviewSignal::RequestChanges),
        _ => None,
    }
}

/// Execute a `gh` command via a pre-connected builderd client.
async fn exec_gh_via_builderd(
    client: &BuilderdClient,
    args: &[&str],
) -> Result<CompletedExec, anyhow::Error> {
    client
        .exec_collect("gh", args, "/tmp")
        .await
        .map_err(|e| anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_review_commands() {
        assert_eq!(
            parse_review_command("ur approve"),
            Some(ReviewSignal::Approve)
        );
        assert_eq!(
            parse_review_command("ur respond"),
            Some(ReviewSignal::RequestChanges)
        );
    }

    #[test]
    fn parse_review_command_rejects_non_commands() {
        assert_eq!(parse_review_command(""), None);
        assert_eq!(parse_review_command("ur approve please"), None);
        assert_eq!(parse_review_command("looks good"), None);
        assert_eq!(parse_review_command(":ship:"), None);
        assert_eq!(parse_review_command("ur"), None);
    }

    #[test]
    fn evaluate_ci_runs_all_green() {
        let runs = vec![
            CheckRun {
                name: "build".to_string(),
                status: "completed".to_string(),
                conclusion: "success".to_string(),
                details_url: String::new(),
                completed_at: "2026-01-01T00:00:00Z".to_string(),
            },
            CheckRun {
                name: "test".to_string(),
                status: "".to_string(),
                conclusion: "success".to_string(),
                details_url: String::new(),
                completed_at: "2026-01-01T00:01:00Z".to_string(),
            },
        ];

        let result = evaluate_ci_runs(&runs);
        assert_eq!(result.status, workflow_condition::ci_status::SUCCEEDED);
        assert_eq!(result.completed_at, "2026-01-01T00:01:00Z");
    }

    #[test]
    fn evaluate_ci_runs_with_failure() {
        let runs = vec![
            CheckRun {
                name: "build".to_string(),
                status: "completed".to_string(),
                conclusion: "success".to_string(),
                details_url: String::new(),
                completed_at: "2026-01-01T00:00:00Z".to_string(),
            },
            CheckRun {
                name: "test".to_string(),
                status: "FAILURE".to_string(),
                conclusion: "failure".to_string(),
                details_url: String::new(),
                completed_at: "2026-01-01T00:02:00Z".to_string(),
            },
        ];

        let result = evaluate_ci_runs(&runs);
        assert_eq!(result.status, workflow_condition::ci_status::FAILED);
        assert_eq!(result.completed_at, "2026-01-01T00:02:00Z");
    }

    #[test]
    fn evaluate_ci_runs_pending() {
        let runs = vec![
            CheckRun {
                name: "build".to_string(),
                status: "completed".to_string(),
                conclusion: "success".to_string(),
                details_url: String::new(),
                completed_at: "2026-01-01T00:00:00Z".to_string(),
            },
            CheckRun {
                name: "test".to_string(),
                status: "in_progress".to_string(),
                conclusion: "".to_string(),
                details_url: String::new(),
                completed_at: String::new(),
            },
        ];

        let result = evaluate_ci_runs(&runs);
        assert_eq!(result.status, workflow_condition::ci_status::PENDING);
    }

    #[test]
    fn evaluate_ci_runs_empty_is_succeeded() {
        // No checks = succeeded (handled by check_ci_status, not evaluate_ci_runs).
        // But test with a skipped check.
        let runs = vec![CheckRun {
            name: "lint".to_string(),
            status: "completed".to_string(),
            conclusion: "skipped".to_string(),
            details_url: String::new(),
            completed_at: "2026-01-01T00:00:00Z".to_string(),
        }];

        let result = evaluate_ci_runs(&runs);
        assert_eq!(result.status, workflow_condition::ci_status::SUCCEEDED);
    }

    #[test]
    fn is_check_completed_cases() {
        assert!(is_check_completed("", "success"));
        assert!(is_check_completed("completed", "success"));
        assert!(is_check_completed("SUCCESS", ""));
        assert!(is_check_completed("FAILURE", ""));
        assert!(!is_check_completed("pending", ""));
        assert!(!is_check_completed("in_progress", ""));
        assert!(!is_check_completed("queued", ""));
    }

    #[test]
    fn is_check_failed_cases() {
        assert!(is_check_failed("failure"));
        assert!(is_check_failed("FAILURE"));
        assert!(is_check_failed("cancelled"));
        assert!(!is_check_failed("success"));
        assert!(!is_check_failed("SUCCESS"));
        assert!(!is_check_failed("skipped"));
        assert!(!is_check_failed("SKIPPED"));
        assert!(!is_check_failed("neutral"));
        assert!(!is_check_failed("NEUTRAL"));
        assert!(!is_check_failed(""));
    }
}
