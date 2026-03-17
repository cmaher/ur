use ur_rpc::proto::workerd::SendMessageRequest;
use ur_rpc::proto::workerd::worker_daemon_service_client::WorkerDaemonServiceClient;

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
}
