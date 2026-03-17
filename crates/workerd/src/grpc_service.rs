use tonic::{Request, Response, Status};
use tracing::{error, info};

use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonService;
use ur_rpc::proto::workerd::{
    NotifyIdleRequest, NotifyIdleResponse, SendMessageRequest, SendMessageResponse,
};

#[derive(Clone)]
pub struct WorkerDaemonServiceImpl;

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
        info!("NotifyIdle received");
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
