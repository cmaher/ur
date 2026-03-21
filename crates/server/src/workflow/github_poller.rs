use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

use remote_repo::{GhBackend, RemoteRepo};
use ur_db::TicketRepo;
use ur_db::model::{LifecycleStatus, Ticket};
use ur_rpc::proto::builder::BuilderdClient;
use ur_rpc::stream::CompletedExec;

use super::TransitionRequest;

/// Delay between individual GitHub API calls to avoid rate limiting.
const API_CALL_DELAY: Duration = Duration::from_secs(2);

/// Polls GitHub for CI status and PR review signals on tickets in
/// `pushing` and `in_review` lifecycle states.
///
/// Runs as a separate tokio task from the workflow engine. Sends
/// transition requests to the WorkflowCoordinator via an mpsc channel
/// instead of directly updating lifecycle_status in the database.
#[derive(Clone)]
pub struct GithubPollerManager {
    ticket_repo: TicketRepo,
    builderd_client: BuilderdClient,
    scan_interval: Duration,
    transition_tx: mpsc::Sender<TransitionRequest>,
}

impl GithubPollerManager {
    pub fn new(
        ticket_repo: TicketRepo,
        builderd_client: BuilderdClient,
        scan_interval: Duration,
        transition_tx: mpsc::Sender<TransitionRequest>,
    ) -> Self {
        Self {
            ticket_repo,
            builderd_client,
            scan_interval,
            transition_tx,
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

    /// Run one full scan: check all pushing and in_review tickets.
    async fn poll_once(&self) {
        // Phase 1: Check pushing tickets (by workflow status) for CI completion.
        match self
            .ticket_repo
            .tickets_by_workflow_status(LifecycleStatus::Pushing)
            .await
        {
            Ok(tickets) => {
                for ticket in &tickets {
                    self.check_pushing_ticket(ticket).await;
                    tokio::time::sleep(API_CALL_DELAY).await;
                }
            }
            Err(e) => {
                error!(error = %e, "failed to query pushing workflows");
            }
        }

        // Phase 2: Check in_review tickets (by workflow status) for review signals.
        match self
            .ticket_repo
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
            .ticket_repo
            .set_workflow_feedback_mode(ticket_id, feedback_mode)
            .await
        {
            error!(ticket_id = %ticket_id, error = %e, "failed to set workflow feedback_mode");
            return;
        }
        self.send_transition(ticket_id, target).await;
    }

    async fn check_pushing_ticket(&self, ticket: &Ticket) {
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

        info!(
            ticket_id = %ticket.id,
            pr_number = pr_number,
            "checking CI status for pushing ticket"
        );

        let backend = GhBackend {
            client: self.builderd_client.clone(),
            gh_repo,
        };

        match check_ci_status(&backend, pr_number).await {
            Ok(CiStatus::AllGreen) => {
                info!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number,
                    "CI all green — transitioning to in_review"
                );
                self.send_transition(&ticket.id, LifecycleStatus::InReview)
                    .await;
            }
            Ok(CiStatus::Pending) => {
                // Still running — do nothing, will check again next scan.
            }
            Ok(CiStatus::Failed) => {
                // CI failed — collect failing check names and transition to implementing.
                let failing_checks = collect_failing_checks(&backend, pr_number).await;

                warn!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number,
                    failing_checks = %failing_checks,
                    "CI has failures — transitioning to implementing"
                );

                // Add activity with failing check details.
                let message = format!(
                    "[workflow] CI failure detected\n\
                     source: workflow\n\
                     result: fail\n\
                     ---\n\
                     {failing_checks}"
                );
                if let Err(e) = self
                    .ticket_repo
                    .add_activity(&ticket.id, "workflow", &message)
                    .await
                {
                    error!(ticket_id = %ticket.id, error = %e, "failed to add CI failure activity");
                    return;
                }

                self.send_transition(&ticket.id, LifecycleStatus::Implementing)
                    .await;
            }
            Ok(CiStatus::NoChecks) => {
                // No checks configured — treat as green.
                info!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number,
                    "no CI checks found — transitioning to in_review"
                );
                self.send_transition(&ticket.id, LifecycleStatus::InReview)
                    .await;
            }
            Err(e) => {
                warn!(
                    ticket_id = %ticket.id,
                    error = %e,
                    "failed to check CI status"
                );
            }
        }
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

        // Fetch seen comment IDs to avoid re-triggering on already-handled comments.
        let seen_comment_ids = match self.ticket_repo.get_seen_comment_ids(&ticket.id).await {
            Ok(ids) => ids,
            Err(e) => {
                warn!(ticket_id = %ticket.id, error = %e, "failed to get seen comment IDs");
                return;
            }
        };

        info!(
            ticket_id = %ticket.id,
            pr_number = pr_number,
            "checking review status for in_review ticket"
        );

        let backend = GhBackend {
            client: self.builderd_client.clone(),
            gh_repo: gh_repo.clone(),
        };
        let signal_result = check_review_signal(
            &backend,
            &self.builderd_client,
            &gh_repo,
            pr_number,
            &seen_comment_ids,
        )
        .await;

        self.handle_review_signal(&ticket.id, signal_result, &backend, pr_number)
            .await;
    }

    /// Dispatch a review signal to the appropriate handler.
    async fn handle_review_signal(
        &self,
        ticket_id: &str,
        signal_result: Result<ReviewCheckResult, anyhow::Error>,
        backend: &GhBackend,
        pr_number: i64,
    ) {
        let result = match signal_result {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    ticket_id = %ticket_id,
                    error = %e,
                    "failed to check review signal"
                );
                return;
            }
        };

        match result.signal {
            ReviewSignal::Approve => {
                // If the approve command is the only unseen comment, skip
                // feedback creation and go directly to merging.
                if result.unseen_count <= 1 {
                    self.record_comments_and_transition(
                        ticket_id,
                        backend,
                        pr_number,
                        ur_rpc::feedback_mode::LATER,
                        LifecycleStatus::Merging,
                        "approval signal (approve-only) — skipping feedback, transitioning to merging",
                    )
                    .await;
                } else {
                    self.record_comments_and_transition(
                        ticket_id,
                        backend,
                        pr_number,
                        ur_rpc::feedback_mode::LATER,
                        LifecycleStatus::FeedbackCreating,
                        "approval signal — transitioning to feedback_creating (mode=later)",
                    )
                    .await;
                }
            }
            ReviewSignal::RequestChanges => {
                if let Err(e) = self.ticket_repo.reset_implement_cycles(ticket_id).await {
                    warn!(
                        ticket_id = %ticket_id,
                        error = %e,
                        "failed to reset implement_cycles"
                    );
                }
                self.record_comments_and_transition(
                    ticket_id,
                    backend,
                    pr_number,
                    ur_rpc::feedback_mode::NOW,
                    LifecycleStatus::FeedbackCreating,
                    "changes requested — resetting cycles and transitioning to feedback_creating (mode=now)",
                )
                .await;
            }
            ReviewSignal::Merged => {
                info!(
                    ticket_id = %ticket_id,
                    pr_number = %pr_number,
                    "PR merged by human — transitioning to feedback_creating (mode=later)"
                );
                self.set_feedback_mode_and_transition(
                    ticket_id,
                    ur_rpc::feedback_mode::LATER,
                    LifecycleStatus::FeedbackCreating,
                )
                .await;
            }
            ReviewSignal::Closed => {
                info!(
                    ticket_id = %ticket_id,
                    pr_number = %pr_number,
                    "PR closed without merge — deleting workflow and reverting ticket to open"
                );
                self.cancel_workflow_and_revert(ticket_id).await;
            }
            ReviewSignal::Pending => {}
        }
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
            .ticket_repo
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
            .ticket_repo
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
}

// ---------------------------------------------------------------------------
// GitHub API helpers (via GhBackend through builderd)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum CiStatus {
    AllGreen,
    Pending,
    Failed,
    NoChecks,
}

/// Check CI status for a PR via `GhBackend::check_runs`.
async fn check_ci_status(backend: &GhBackend, pr_number: i64) -> Result<CiStatus, anyhow::Error> {
    let runs = backend.check_runs(pr_number).await?;

    if runs.is_empty() {
        return Ok(CiStatus::NoChecks);
    }

    let mut all_completed = true;
    let mut any_failed = false;

    for run in &runs {
        // GhBackend uses `gh pr checks --json state,conclusion` where "state"
        // maps to the check status. Completed checks have state="" or
        // conclusion set; pending checks have state that isn't empty/completed.
        let status = run.status.as_str();
        let conclusion = run.conclusion.as_str();

        // gh pr checks returns state as "" when completed, or status strings
        // like "pending", "queued", "in_progress" when not.
        let is_completed = status.is_empty()
            || status == "completed"
            || status == "SUCCESS"
            || status == "FAILURE"
            || status == "NEUTRAL"
            || status == "SKIPPED"
            || !conclusion.is_empty();

        if !is_completed {
            all_completed = false;
        } else if !conclusion.is_empty()
            && conclusion != "success"
            && conclusion != "SUCCESS"
            && conclusion != "skipped"
            && conclusion != "SKIPPED"
            && conclusion != "neutral"
            && conclusion != "NEUTRAL"
        {
            any_failed = true;
        }
    }

    if any_failed {
        Ok(CiStatus::Failed)
    } else if all_completed {
        Ok(CiStatus::AllGreen)
    } else {
        Ok(CiStatus::Pending)
    }
}

/// Collect a summary string of failing check runs for activity recording.
async fn collect_failing_checks(backend: &GhBackend, pr_number: i64) -> String {
    let runs = match backend.check_runs(pr_number).await {
        Ok(r) => r,
        Err(e) => return format!("(failed to fetch check runs: {e})"),
    };

    let mut failures: Vec<String> = Vec::new();
    for run in &runs {
        let conclusion = run.conclusion.as_str();
        if !conclusion.is_empty()
            && conclusion != "success"
            && conclusion != "SUCCESS"
            && conclusion != "skipped"
            && conclusion != "SKIPPED"
            && conclusion != "neutral"
            && conclusion != "NEUTRAL"
        {
            failures.push(format!("{}: {}", run.name, conclusion));
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
#[allow(dead_code)]
struct ReviewCheckResult {
    signal: ReviewSignal,
    /// The comment ID that triggered the signal, if it originated from a PR comment.
    comment_id: Option<String>,
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

    if pr.state == "closed" || pr.state == "CLOSED" {
        if pr.state == "MERGED" {
            return Ok(ReviewCheckResult {
                signal: ReviewSignal::Merged,
                comment_id: None,
                unseen_count: 0,
            });
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
        let signal = if merged {
            ReviewSignal::Merged
        } else {
            ReviewSignal::Closed
        };
        return Ok(ReviewCheckResult {
            signal,
            comment_id: None,
            unseen_count: 0,
        });
    }

    // PR is still open — check latest unseen comment for review command.
    let comments = backend.get_conversation_comments(pr_number).await?;

    // Filter out already-seen comments.
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
                comment_id: None,
                unseen_count: 0,
            });
        }
    };

    let comment_id_str = latest_comment.id.to_string();
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
            comment_id: None,
            unseen_count,
        });
    }

    // Parse the comment body for review commands.
    // The comment must be exactly the command text (whitespace-trimmed).
    let trimmed = comment_body.trim();
    match parse_review_command(trimmed) {
        Some(signal) => Ok(ReviewCheckResult {
            signal,
            comment_id: Some(comment_id_str),
            unseen_count,
        }),
        None => Ok(ReviewCheckResult {
            signal: ReviewSignal::Pending,
            comment_id: None,
            unseen_count,
        }),
    }
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
}
