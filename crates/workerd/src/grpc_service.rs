use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::Mutex;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use ur_rpc::proto::core::UpdateAgentStatusRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonService;
use ur_rpc::proto::workerd::{
    CreateFeedbackRequest, CreateFeedbackResponse, ImplementRequest, ImplementResponse,
    NotifyIdleRequest, NotifyIdleResponse, SendMessageRequest, SendMessageResponse,
    StepCompleteRequest, StepCompleteResponse,
};

/// Buffer for dispatched commands that workerd drains on idle signals.
///
/// When the server dispatches work, it populates `commands` and sets `lifecycle_step`.
/// Each idle signal pops the next command. Once drained, workerd waits for the agent
/// to call step-complete before forwarding idle to the server.
pub struct DispatchBuffer {
    /// Remaining tmux commands to send to the agent session.
    pub commands: VecDeque<String>,
    /// Whether the agent has signalled that the current step is complete.
    pub step_complete: bool,
    /// The current lifecycle step name (e.g. "implementing", "feedback_creating").
    /// Empty string means no active dispatch.
    pub lifecycle_step: String,
}

#[derive(Clone)]
pub struct WorkerDaemonServiceImpl {
    pub server_addr: String,
    pub worker_id: String,
    pub worker_secret: String,
    pub dispatch_buffer: Arc<Mutex<DispatchBuffer>>,
}

impl WorkerDaemonServiceImpl {
    /// Fire-and-forget: forward idle status to the ur-server.
    fn forward_idle_to_server(&self) {
        let addr = format!("http://{}", self.server_addr);
        let worker_id = self.worker_id.clone();
        let worker_secret = self.worker_secret.clone();

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
                status: ur_rpc::agent_status::IDLE.to_string(),
                message: String::new(),
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
    }
}

#[tonic::async_trait]
impl WorkerDaemonService for WorkerDaemonServiceImpl {
    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<SendMessageResponse>, Status> {
        let message = &request.into_inner().message;
        info!(message, "SendMessage received");

        let session = tmux::Session::agent();
        match session.send_keys(message).await {
            Ok(()) => {
                info!("send-keys succeeded");
                Ok(Response::new(SendMessageResponse {
                    success: true,
                    error: String::new(),
                }))
            }
            Err(e) => {
                error!(error = %e, "tmux send-keys failed");
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
        info!("NotifyIdle received, consulting dispatch buffer");

        let mut buf = self.dispatch_buffer.lock().await;

        // Case 1: Buffer has commands — pop and send to tmux
        if let Some(command) = buf.commands.pop_front() {
            let remaining = buf.commands.len();
            info!(
                remaining,
                command = command.as_str(),
                "Dispatching buffered command"
            );
            drop(buf);

            let session = tmux::Session::agent();
            if let Err(e) = session.send_keys(&command).await {
                error!(error = %e, "tmux send-keys failed for buffered command");
            }
            return Ok(Response::new(NotifyIdleResponse {}));
        }

        // Case 2: Buffer empty + step_complete — clear state and forward idle to server
        if buf.step_complete {
            info!("Buffer drained and step complete, forwarding idle to server");
            buf.commands.clear();
            buf.step_complete = false;
            buf.lifecycle_step = String::new();
            drop(buf);

            self.forward_idle_to_server();
            return Ok(Response::new(NotifyIdleResponse {}));
        }

        // Case 3: Buffer empty + !step_complete + lifecycle_step set — nudge the agent
        if !buf.lifecycle_step.is_empty() {
            let step = buf.lifecycle_step.clone();
            info!(
                lifecycle_step = step.as_str(),
                "Buffer drained but step not complete, nudging agent"
            );
            drop(buf);

            let nudge_message = format!(
                "You have finished the '{step}' step commands. Please run `workertools step-complete` to signal completion, or `/request-human` if you need help."
            );
            let session = tmux::Session::agent();
            if let Err(e) = session.send_keys(&nudge_message).await {
                error!(error = %e, "tmux send-keys failed for nudge message");
            }
            return Ok(Response::new(NotifyIdleResponse {}));
        }

        // Case 4: No active dispatch — forward idle to server (current behavior)
        drop(buf);
        info!("No active dispatch, forwarding idle to server");
        self.forward_idle_to_server();

        Ok(Response::new(NotifyIdleResponse {}))
    }

    async fn step_complete(
        &self,
        _request: Request<StepCompleteRequest>,
    ) -> Result<Response<StepCompleteResponse>, Status> {
        info!("StepComplete received, marking step as complete");
        let mut buf = self.dispatch_buffer.lock().await;
        buf.step_complete = true;
        Ok(Response::new(StepCompleteResponse {}))
    }

    async fn implement(
        &self,
        request: Request<ImplementRequest>,
    ) -> Result<Response<ImplementResponse>, Status> {
        let ticket_id = &request.into_inner().ticket_id;
        let skill_command = format!("/implement {ticket_id}");
        info!(
            ticket_id,
            "Implement received, sending skill invocation to tmux"
        );

        let session = tmux::Session::agent();
        if let Err(e) = session.send_keys("/clear").await {
            error!(error = %e, "tmux send-keys failed for /clear before /implement");
        }
        if let Err(e) = session.send_keys(&skill_command).await {
            error!(error = %e, "tmux send-keys failed for /implement");
        }

        Ok(Response::new(ImplementResponse {}))
    }

    async fn create_feedback_tickets(
        &self,
        request: Request<CreateFeedbackRequest>,
    ) -> Result<Response<CreateFeedbackResponse>, Status> {
        let inner = request.into_inner();
        let skill_command = format!("/create-feedback {} {}", inner.ticket_id, inner.pr_number);
        info!(
            ticket_id = inner.ticket_id,
            pr_number = inner.pr_number,
            "CreateFeedbackTickets received, sending skill invocation to tmux"
        );

        let session = tmux::Session::agent();
        if let Err(e) = session.send_keys("/clear").await {
            error!(error = %e, "tmux send-keys failed for /clear before /create-feedback");
        }
        if let Err(e) = session.send_keys(&skill_command).await {
            error!(error = %e, "tmux send-keys failed for /create-feedback");
        }

        Ok(Response::new(CreateFeedbackResponse {}))
    }
}
