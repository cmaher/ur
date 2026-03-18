use ur_rpc::proto::workerd::worker_daemon_service_client::WorkerDaemonServiceClient;
use ur_rpc::proto::workerd::{
    CreateFeedbackRequest, FixRequest, ImplementRequest, PushRequest, SendMessageRequest,
};

/// Thin client for sending messages to a workerd instance inside a worker container.
///
/// Each worker container runs a `workerd` gRPC daemon on a fixed port.
/// The server derives the workerd address from the container name (Docker DNS)
/// and the fixed gRPC port.
#[derive(Clone)]
pub struct WorkerdClient {
    /// gRPC address of the workerd daemon (e.g., `http://ur-worker-alice:9120`).
    addr: String,
}

impl WorkerdClient {
    pub fn new(addr: String) -> Self {
        Self { addr }
    }

    /// Send a message to the workerd instance and return the result.
    /// Returns `Ok(())` on success, or `Err(reason)` if workerd reports failure
    /// or is unreachable.
    pub async fn send_message(&self, message: &str) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::connect(self.addr.clone())
            .await
            .map_err(|e| format!("workerd unavailable at {}: {e}", self.addr))?;

        let req = SendMessageRequest {
            message: message.to_string(),
        };

        let response = client
            .send_message(req)
            .await
            .map_err(|e| format!("workerd SendMessage failed: {e}"))?;

        let resp = response.into_inner();
        if resp.success {
            Ok(())
        } else {
            Err(resp.error)
        }
    }

    /// Fire-and-forget: send an /implement skill invocation to the worker.
    pub async fn implement(&self, ticket_id: &str) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::connect(self.addr.clone())
            .await
            .map_err(|e| format!("workerd unavailable at {}: {e}", self.addr))?;

        let req = ImplementRequest {
            ticket_id: ticket_id.to_string(),
        };

        client
            .implement(req)
            .await
            .map_err(|e| format!("workerd Implement failed: {e}"))?;

        Ok(())
    }

    /// Fire-and-forget: send a /push skill invocation to the worker.
    pub async fn push(&self) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::connect(self.addr.clone())
            .await
            .map_err(|e| format!("workerd unavailable at {}: {e}", self.addr))?;

        let req = PushRequest {};

        client
            .push(req)
            .await
            .map_err(|e| format!("workerd Push failed: {e}"))?;

        Ok(())
    }

    /// Fire-and-forget: send a /fix skill invocation to the worker.
    pub async fn fix(&self, ticket_id: &str, fix_phase: &str) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::connect(self.addr.clone())
            .await
            .map_err(|e| format!("workerd unavailable at {}: {e}", self.addr))?;

        let req = FixRequest {
            ticket_id: ticket_id.to_string(),
            fix_phase: fix_phase.to_string(),
        };

        client
            .fix(req)
            .await
            .map_err(|e| format!("workerd Fix failed: {e}"))?;

        Ok(())
    }

    /// Fire-and-forget: send a /create-feedback skill invocation to the worker.
    pub async fn create_feedback_tickets(
        &self,
        ticket_id: &str,
        pr_number: u32,
    ) -> Result<(), String> {
        let mut client = WorkerDaemonServiceClient::connect(self.addr.clone())
            .await
            .map_err(|e| format!("workerd unavailable at {}: {e}", self.addr))?;

        let req = CreateFeedbackRequest {
            ticket_id: ticket_id.to_string(),
            pr_number,
        };

        client
            .create_feedback_tickets(req)
            .await
            .map_err(|e| format!("workerd CreateFeedbackTickets failed: {e}"))?;

        Ok(())
    }
}
