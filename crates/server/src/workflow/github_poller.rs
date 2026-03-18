use std::time::Duration;

use tokio::sync::watch;
use tracing::{error, info, warn};

use remote_repo::{GhBackend, RemoteRepo};
use ur_db::TicketRepo;
use ur_db::model::{LifecycleStatus, Ticket, TicketUpdate};
use ur_rpc::proto::builder::{
    BuilderExecMessage, BuilderExecRequest, BuilderdClient,
    builder_exec_message::Payload as ExecPayload,
};
use ur_rpc::stream::CompletedExec;

/// Delay between individual GitHub API calls to avoid rate limiting.
const API_CALL_DELAY: Duration = Duration::from_secs(2);

/// Delay between full polling scans.
const SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// Polls GitHub for CI status and PR review signals on tickets in
/// `pushing` and `in_review` lifecycle states.
///
/// Runs as a separate tokio task from the workflow engine. Updates
/// lifecycle_status via TicketRepo (which triggers the SQLite trigger
/// → workflow_event for downstream handlers).
#[derive(Clone)]
pub struct GithubPollerManager {
    ticket_repo: TicketRepo,
    builderd_client: BuilderdClient,
}

impl GithubPollerManager {
    pub fn new(ticket_repo: TicketRepo, builderd_client: BuilderdClient) -> Self {
        Self {
            ticket_repo,
            builderd_client,
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
                _ = tokio::time::sleep(SCAN_INTERVAL) => {}
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
        // Phase 1: Check pushing tickets for CI completion.
        match self
            .ticket_repo
            .tickets_by_lifecycle_status(LifecycleStatus::Pushing)
            .await
        {
            Ok(tickets) => {
                for ticket in &tickets {
                    self.check_pushing_ticket(ticket).await;
                    tokio::time::sleep(API_CALL_DELAY).await;
                }
            }
            Err(e) => {
                error!(error = %e, "failed to query pushing tickets");
            }
        }

        // Phase 2: Check in_review tickets for review signals.
        match self
            .ticket_repo
            .tickets_by_lifecycle_status(LifecycleStatus::InReview)
            .await
        {
            Ok(tickets) => {
                for ticket in &tickets {
                    self.check_in_review_ticket(ticket).await;
                    tokio::time::sleep(API_CALL_DELAY).await;
                }
            }
            Err(e) => {
                error!(error = %e, "failed to query in_review tickets");
            }
        }
    }

    /// For a pushing ticket: check if CI is all green, then transition to in_review.
    async fn check_pushing_ticket(&self, ticket: &Ticket) {
        let meta = match self.ticket_repo.get_meta(&ticket.id, "ticket").await {
            Ok(m) => m,
            Err(e) => {
                warn!(ticket_id = %ticket.id, error = %e, "failed to get ticket metadata");
                return;
            }
        };

        let Some(pr_number_str) = meta.get("pr_number") else {
            return;
        };

        let pr_number: i64 = match pr_number_str.parse() {
            Ok(n) => n,
            Err(e) => {
                warn!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number_str,
                    error = %e,
                    "invalid pr_number metadata — cannot parse as integer"
                );
                return;
            }
        };

        let gh_repo = match meta.get("gh_repo") {
            Some(r) => r.clone(),
            None => {
                warn!(
                    ticket_id = %ticket.id,
                    "no gh_repo metadata — cannot check CI status"
                );
                return;
            }
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
                self.transition_lifecycle(&ticket.id, LifecycleStatus::InReview)
                    .await;
            }
            Ok(CiStatus::Pending) => {
                // Still running — do nothing, will check again next scan.
            }
            Ok(CiStatus::Failed) => {
                // CI failed — the push handler's worker should fix it.
                // Leave in pushing state for now.
                warn!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number,
                    "CI has failures — staying in pushing"
                );
            }
            Ok(CiStatus::NoChecks) => {
                // No checks configured — treat as green.
                info!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number,
                    "no CI checks found — transitioning to in_review"
                );
                self.transition_lifecycle(&ticket.id, LifecycleStatus::InReview)
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

    /// For an in_review ticket: check for review emoji signals or merge/close.
    async fn check_in_review_ticket(&self, ticket: &Ticket) {
        let meta = match self.ticket_repo.get_meta(&ticket.id, "ticket").await {
            Ok(m) => m,
            Err(e) => {
                warn!(ticket_id = %ticket.id, error = %e, "failed to get ticket metadata");
                return;
            }
        };

        let Some(pr_number_str) = meta.get("pr_number") else {
            return;
        };

        let pr_number: i64 = match pr_number_str.parse() {
            Ok(n) => n,
            Err(e) => {
                warn!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number_str,
                    error = %e,
                    "invalid pr_number metadata — cannot parse as integer"
                );
                return;
            }
        };

        let gh_repo = match meta.get("gh_repo") {
            Some(r) => r.clone(),
            None => {
                warn!(
                    ticket_id = %ticket.id,
                    "no gh_repo metadata — cannot check review status"
                );
                return;
            }
        };

        // Check for autoapprove meta — if present, auto-advance without waiting.
        if meta.contains_key("autoapprove") {
            info!(
                ticket_id = %ticket.id,
                pr_number = pr_number,
                "autoapprove set — transitioning to feedback_creating with feedback_mode=later"
            );
            if let Err(e) = self
                .ticket_repo
                .set_meta(&ticket.id, "ticket", "feedback_mode", "later")
                .await
            {
                error!(ticket_id = %ticket.id, error = %e, "failed to set feedback_mode");
                return;
            }
            self.transition_lifecycle(&ticket.id, LifecycleStatus::FeedbackCreating)
                .await;
            return;
        }

        info!(
            ticket_id = %ticket.id,
            pr_number = pr_number,
            "checking review status for in_review ticket"
        );

        let backend = GhBackend {
            client: self.builderd_client.clone(),
            gh_repo: gh_repo.clone(),
        };
        match check_review_signal(&backend, &self.builderd_client, &gh_repo, pr_number).await {
            Ok(ReviewSignal::Approve) => {
                info!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number,
                    "approval signal — transitioning to feedback_creating (mode=later)"
                );
                if let Err(e) = self
                    .ticket_repo
                    .set_meta(&ticket.id, "ticket", "feedback_mode", "later")
                    .await
                {
                    error!(ticket_id = %ticket.id, error = %e, "failed to set feedback_mode");
                    return;
                }
                self.transition_lifecycle(&ticket.id, LifecycleStatus::FeedbackCreating)
                    .await;
            }
            Ok(ReviewSignal::RequestChanges) => {
                info!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number,
                    "changes requested — transitioning to feedback_creating (mode=now)"
                );
                if let Err(e) = self
                    .ticket_repo
                    .set_meta(&ticket.id, "ticket", "feedback_mode", "now")
                    .await
                {
                    error!(ticket_id = %ticket.id, error = %e, "failed to set feedback_mode");
                    return;
                }
                self.transition_lifecycle(&ticket.id, LifecycleStatus::FeedbackCreating)
                    .await;
            }
            Ok(ReviewSignal::Merged) => {
                info!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number,
                    "PR merged by human — transitioning to feedback_creating (mode=later)"
                );
                if let Err(e) = self
                    .ticket_repo
                    .set_meta(&ticket.id, "ticket", "feedback_mode", "later")
                    .await
                {
                    error!(ticket_id = %ticket.id, error = %e, "failed to set feedback_mode");
                    return;
                }
                self.transition_lifecycle(&ticket.id, LifecycleStatus::FeedbackCreating)
                    .await;
            }
            Ok(ReviewSignal::Closed) => {
                info!(
                    ticket_id = %ticket.id,
                    pr_number = %pr_number,
                    "PR closed without merge — stalling ticket"
                );
                self.transition_lifecycle(&ticket.id, LifecycleStatus::Stalled)
                    .await;
            }
            Ok(ReviewSignal::Pending) => {
                // No actionable signal yet — will check again next scan.
            }
            Err(e) => {
                warn!(
                    ticket_id = %ticket.id,
                    error = %e,
                    "failed to check review signal"
                );
            }
        }
    }

    /// Transition a ticket's lifecycle_status, which fires the SQLite trigger.
    async fn transition_lifecycle(&self, ticket_id: &str, to: LifecycleStatus) {
        let update = TicketUpdate {
            lifecycle_status: Some(to),
            lifecycle_managed: None,
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        if let Err(e) = self.ticket_repo.update_ticket(ticket_id, &update).await {
            error!(
                ticket_id = %ticket_id,
                target = %to,
                error = %e,
                "failed to transition lifecycle status"
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

#[derive(Debug, PartialEq, Eq)]
enum ReviewSignal {
    /// Approval emoji (checkmark, rocket, ship, :shipit:)
    Approve,
    /// Changes requested emoji (construction)
    RequestChanges,
    /// PR was merged (by a human, not by us)
    Merged,
    /// PR was closed without merge
    Closed,
    /// No actionable signal yet
    Pending,
}

/// Check for review signals on a PR: latest comment emoji, merge status, close status.
async fn check_review_signal(
    backend: &GhBackend,
    builderd_client: &BuilderdClient,
    gh_repo: &str,
    pr_number: i64,
) -> Result<ReviewSignal, anyhow::Error> {
    // First, check PR state (merged/closed).
    let pr = backend.get_pr(pr_number).await?;

    if pr.state == "closed" || pr.state == "CLOSED" {
        // Check if merged by looking at the raw API response (PullRequest
        // type doesn't carry a "merged" boolean, so a closed PR that was
        // merged has state "MERGED" in GraphQL or we fall back to the REST
        // API check).
        if pr.state == "MERGED" {
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
        if merged {
            return Ok(ReviewSignal::Merged);
        }
        return Ok(ReviewSignal::Closed);
    }

    // PR is still open — check latest comment for emoji signal.
    // Only the latest comment counts, and only if no commits since that comment.
    let comments = backend.get_conversation_comments(pr_number).await?;

    let latest_comment = match comments.last() {
        Some(c) => c,
        None => return Ok(ReviewSignal::Pending),
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
        return Ok(ReviewSignal::Pending);
    }

    // Parse the comment body for emoji signals.
    let trimmed = comment_body.trim();
    if contains_approval_signal(trimmed) {
        Ok(ReviewSignal::Approve)
    } else if contains_changes_requested_signal(trimmed) {
        Ok(ReviewSignal::RequestChanges)
    } else {
        Ok(ReviewSignal::Pending)
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

/// Check if text contains an approval emoji/keyword.
fn contains_approval_signal(text: &str) -> bool {
    // Unicode emoji: check mark ✅, rocket 🚀, ship 🚢
    // Text form: :shipit:
    text.contains('\u{2705}')       // ✅
        || text.contains('\u{1F680}') // 🚀
        || text.contains('\u{1F6A2}') // 🚢
        || text.contains(":shipit:")
}

/// Check if text contains a changes-requested signal.
fn contains_changes_requested_signal(text: &str) -> bool {
    // Construction emoji: 🚧
    text.contains('\u{1F6A7}')
}

/// Execute a `gh` command via a pre-connected builderd client.
async fn exec_gh_via_builderd(
    client: &BuilderdClient,
    args: &[&str],
) -> Result<CompletedExec, anyhow::Error> {
    let mut client = client.clone();
    let req = BuilderExecRequest {
        command: "gh".into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        working_dir: "/tmp".into(),
        env: std::collections::HashMap::new(),
        long_lived: false,
    };
    let start_msg = BuilderExecMessage {
        payload: Some(ExecPayload::Start(req)),
    };
    let response = client
        .exec(tokio_stream::once(start_msg))
        .await
        .map_err(|e| anyhow::anyhow!("builderd exec failed: {e}"))?;
    let completed = CompletedExec::collect(response.into_inner())
        .await
        .map_err(|e| anyhow::anyhow!("stream error: {e}"))?;
    Ok(completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_signals_detected() {
        assert!(contains_approval_signal("\u{2705}"));
        assert!(contains_approval_signal("looks good \u{1F680}"));
        assert!(contains_approval_signal("\u{1F6A2} ship it"));
        assert!(contains_approval_signal(":shipit:"));
        assert!(!contains_approval_signal("needs work"));
    }

    #[test]
    fn changes_requested_signal_detected() {
        assert!(contains_changes_requested_signal("\u{1F6A7}"));
        assert!(contains_changes_requested_signal("not ready \u{1F6A7} yet"));
        assert!(!contains_changes_requested_signal("looks good"));
    }

    #[test]
    fn no_signal_in_empty_text() {
        assert!(!contains_approval_signal(""));
        assert!(!contains_changes_requested_signal(""));
    }
}
