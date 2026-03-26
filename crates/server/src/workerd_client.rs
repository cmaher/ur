use tracing::warn;
use ur_db::WorkerRepo;
use ur_db::model::AgentStatus;
use ur_rpc::proto::workerd::worker_daemon_service_client::WorkerDaemonServiceClient;
use ur_rpc::proto::workerd::{AddressFeedbackRequest, ImplementRequest, SendMessageRequest};
use ur_rpc::retry::{RetryChannel, RetryConfig};

/// Thin client for sending messages to a workerd instance inside a worker container.
///
/// Each worker container runs a `workerd` gRPC daemon on a fixed port.
/// The server derives the workerd address from the container name (Docker DNS)
/// and the fixed gRPC port.
///
/// When `worker_repo` and `worker_id` are set, successful dispatches automatically
/// update the worker's agent status to `Working`.
#[derive(Clone)]
pub struct WorkerdClient {
    /// Lazy retry channel for connecting to the workerd daemon.
    retry_channel: RetryChannel,
    /// Optional worker repo + ID for automatic agent status updates.
    status_tracking: Option<StatusTracking>,
}

#[derive(Clone)]
struct StatusTracking {
    worker_repo: WorkerRepo,
    worker_id: String,
}

impl WorkerdClient {
    pub fn new(addr: String) -> Self {
        let retry_channel =
            RetryChannel::new(&addr, RetryConfig::default()).expect("invalid workerd address");
        Self {
            retry_channel,
            status_tracking: None,
        }
    }

    /// Create a client that automatically sets agent status to Working after
    /// each successful dispatch.
    pub fn with_status_tracking(addr: String, worker_repo: WorkerRepo, worker_id: String) -> Self {
        let retry_channel =
            RetryChannel::new(&addr, RetryConfig::default()).expect("invalid workerd address");
        Self {
            retry_channel,
            status_tracking: Some(StatusTracking {
                worker_repo,
                worker_id,
            }),
        }
    }

    /// Mark the worker as Working after a successful dispatch.
    /// Logs a warning on failure but does not propagate the error —
    /// a failed status update should not fail the dispatch itself.
    async fn mark_working(&self) {
        if let Some(ref tracking) = self.status_tracking
            && let Err(e) = tracking
                .worker_repo
                .update_worker_agent_status(&tracking.worker_id, AgentStatus::Working)
                .await
        {
            warn!(
                worker_id = %tracking.worker_id,
                error = %e,
                "failed to update agent status to working after dispatch"
            );
        }
    }

    /// Send a message to the workerd instance and return the result.
    /// Returns `Ok(())` on success, or `Err(reason)` if workerd reports failure
    /// or is unreachable.
    pub async fn send_message(&self, message: &str, submit: bool) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::new(self.retry_channel.channel().clone());

        let req = SendMessageRequest {
            message: message.to_string(),
            submit,
        };

        let response = client
            .send_message(req)
            .await
            .map_err(|e| format!("workerd SendMessage failed: {e}"))?;

        let resp = response.into_inner();
        if resp.success {
            self.mark_working().await;
            Ok(())
        } else {
            Err(resp.error)
        }
    }

    /// Fire-and-forget: send an /implement skill invocation to the worker.
    pub async fn implement(&self, ticket_id: &str) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::new(self.retry_channel.channel().clone());

        let req = ImplementRequest {
            ticket_id: ticket_id.to_string(),
        };

        client
            .implement(req)
            .await
            .map_err(|e| format!("workerd Implement failed: {e}"))?;

        self.mark_working().await;
        Ok(())
    }

    /// Fire-and-forget: send an /address-feedback skill invocation to the worker.
    ///
    /// `handled_comment_ids` lists comment IDs that already have feedback tickets,
    /// so the worker can skip them when processing PR comments.
    pub async fn address_feedback_tickets(
        &self,
        ticket_id: &str,
        pr_number: u32,
        handled_comment_ids: Vec<String>,
    ) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::new(self.retry_channel.channel().clone());

        let req = AddressFeedbackRequest {
            ticket_id: ticket_id.to_string(),
            pr_number,
            handled_comment_ids,
        };

        client
            .address_feedback_tickets(req)
            .await
            .map_err(|e| format!("workerd AddressFeedbackTickets failed: {e}"))?;

        self.mark_working().await;
        Ok(())
    }
}
