use tracing::warn;
use ur_rpc::proto::workerd::worker_daemon_service_client::WorkerDaemonServiceClient;
use ur_rpc::proto::workerd::{
    AddressFeedbackRequest, DesignRequest, ImplementRequest, SendMessageRequest,
    SetStatusLeftRequest,
};
use ur_rpc::retry::{RetryChannel, RetryConfig};
use workflow_db::WorkerRepo;
use workflow_db::model::AgentStatus;

/// Thin client for sending messages to a worker's daemon.
///
/// Build it through [`crate::workerd_address::WorkerdAddressResolver`], which
/// picks the address per runtime: a container worker's own `workerd` (Docker
/// DNS + fixed port), or builderd for a local worker — in which case every call
/// carries `ur-worker-id` so builderd can route to that worker's daemon.
///
/// When `worker_repo` and `worker_id` are set, successful dispatches automatically
/// update the worker's agent status to `Working`.
#[derive(Clone)]
pub struct WorkerdClient {
    /// Lazy retry channel for connecting to the workerd daemon.
    retry_channel: RetryChannel,
    /// Optional worker repo + ID for automatic agent status updates.
    status_tracking: Option<StatusTracking>,
    /// When set, sent as `ur-worker-id` metadata on every call (local workers).
    routing_worker_id: Option<String>,
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
            routing_worker_id: None,
        }
    }

    /// Send `ur-worker-id: <worker_id>` on every call so a shared daemon host
    /// (builderd) can route the request to this worker.
    pub fn with_routing_worker_id(mut self, worker_id: String) -> Self {
        self.routing_worker_id = Some(worker_id);
        self
    }

    /// Wrap `message` in a request carrying the routing header, if any.
    pub(crate) fn request<T>(&self, message: T) -> Result<tonic::Request<T>, String> {
        let mut request = tonic::Request::new(message);
        if let Some(worker_id) = &self.routing_worker_id {
            let value = worker_id
                .parse()
                .map_err(|e| format!("invalid worker id '{worker_id}' for routing header: {e}"))?;
            request
                .metadata_mut()
                .insert(ur_config::WORKER_ID_HEADER, value);
        }
        Ok(request)
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
            routing_worker_id: None,
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
            submit: Some(submit),
        };

        let response = client
            .send_message(self.request(req)?)
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
            .implement(self.request(req)?)
            .await
            .map_err(|e| format!("workerd Implement failed: {e}"))?;

        self.mark_working().await;
        Ok(())
    }

    /// Fire-and-forget: send a /design skill invocation to the worker.
    pub async fn design(&self, ticket_id: &str) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::new(self.retry_channel.channel().clone());

        let req = DesignRequest {
            ticket_id: ticket_id.to_string(),
        };

        client
            .design(self.request(req)?)
            .await
            .map_err(|e| format!("workerd Design failed: {e}"))?;

        self.mark_working().await;
        Ok(())
    }

    /// Update the tmux status-left label for the worker's pane.
    ///
    /// Does NOT call `mark_working` — updating a label is not a dispatch
    /// and should not change the agent's status.
    pub async fn set_status_left(
        &self,
        status_left: &str,
        status_left_length: u32,
    ) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::new(self.retry_channel.channel().clone());

        let req = SetStatusLeftRequest {
            status_left: status_left.to_string(),
            status_left_length,
        };

        let response = client
            .set_status_left(self.request(req)?)
            .await
            .map_err(|e| format!("workerd SetStatusLeft failed: {e}"))?;

        let resp = response.into_inner();
        if resp.success {
            Ok(())
        } else {
            Err(resp.error)
        }
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
            .address_feedback_tickets(self.request(req)?)
            .await
            .map_err(|e| format!("workerd AddressFeedbackTickets failed: {e}"))?;

        self.mark_working().await;
        Ok(())
    }
}
