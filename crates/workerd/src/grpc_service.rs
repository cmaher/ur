use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use ur_rpc::proto::core::UpdateAgentStatusRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonService;
use ur_rpc::proto::workerd::{
    NotifyIdleRequest, NotifyIdleResponse, SendMessageRequest, SendMessageResponse,
};

#[derive(Clone)]
pub struct WorkerDaemonServiceImpl {
    pub server_addr: String,
    pub worker_id: String,
    pub worker_secret: String,
}

#[tonic::async_trait]
impl WorkerDaemonService for WorkerDaemonServiceImpl {
    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<SendMessageResponse>, Status> {
        let message = &request.into_inner().message;
        info!(message, "SendMessage received");

        let escaped = escape_for_tmux(message);

        let output = tokio::process::Command::new("tmux")
            .args(["send-keys", "-t", "agent", &escaped, "Enter"])
            .output()
            .await;

        match output {
            Ok(result) if result.status.success() => {
                info!("send-keys succeeded");
                Ok(Response::new(SendMessageResponse {
                    success: true,
                    error: String::new(),
                }))
            }
            Ok(result) => {
                let stderr = String::from_utf8_lossy(&result.stderr).to_string();
                error!(stderr, "tmux send-keys failed");
                Ok(Response::new(SendMessageResponse {
                    success: false,
                    error: stderr,
                }))
            }
            Err(e) => {
                error!(error = %e, "failed to execute tmux");
                Ok(Response::new(SendMessageResponse {
                    success: false,
                    error: e.to_string(),
                }))
            }
        }
    }

    async fn notify_idle(
        &self,
        _request: Request<NotifyIdleRequest>,
    ) -> Result<Response<NotifyIdleResponse>, Status> {
        info!("NotifyIdle received, updating agent status to idle on server");

        let addr = format!("http://{}", self.server_addr);
        let worker_id = self.worker_id.clone();
        let worker_secret = self.worker_secret.clone();

        // Fire-and-forget: spawn a task so we don't block the response to workertools.
        tokio::spawn(async move {
            let channel = match tonic::transport::Endpoint::try_from(addr.clone())
                .map(|ep| ep.connect_lazy())
            {
                Ok(ch) => ch,
                Err(e) => {
                    warn!(error = %e, "failed to create channel for UpdateAgentStatus");
                    return;
                }
            };

            let mut client = CoreServiceClient::new(channel);

            let mut request = tonic::Request::new(UpdateAgentStatusRequest {
                worker_id: worker_id.clone(),
                status: "idle".to_string(),
            });

            // Inject auth headers
            if let Ok(val) =
                worker_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
            {
                request
                    .metadata_mut()
                    .insert(ur_config::WORKER_ID_HEADER, val);
            }
            if let Ok(val) =
                worker_secret.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
            {
                request
                    .metadata_mut()
                    .insert(ur_config::WORKER_SECRET_HEADER, val);
            }

            match client.update_agent_status(request).await {
                Ok(_) => info!("agent status updated to idle on server"),
                Err(e) => warn!(error = %e, "failed to update agent status on server"),
            }
        });

        Ok(Response::new(NotifyIdleResponse {}))
    }
}

/// Escape special characters for tmux send-keys.
/// Wraps the message in single quotes and escapes any embedded single quotes.
fn escape_for_tmux(message: &str) -> String {
    // For tmux send-keys, we pass the message as a literal string.
    // Escape single quotes by ending the quote, adding escaped quote, restarting quote.
    let escaped = message.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_simple_message() {
        assert_eq!(escape_for_tmux("hello world"), "'hello world'");
    }

    #[test]
    fn test_escape_message_with_single_quotes() {
        assert_eq!(escape_for_tmux("it's here"), "'it'\\''s here'");
    }

    #[test]
    fn test_escape_empty_message() {
        assert_eq!(escape_for_tmux(""), "''");
    }

    #[test]
    fn test_escape_message_with_special_chars() {
        assert_eq!(
            escape_for_tmux("echo $HOME && rm -rf /"),
            "'echo $HOME && rm -rf /'"
        );
    }
}
