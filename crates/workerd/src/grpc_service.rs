use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use ur_rpc::proto::core::UpdateAgentStatusRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonService;
use ur_rpc::proto::workerd::{
    CreateFeedbackRequest, CreateFeedbackResponse, ImplementRequest, ImplementResponse,
    NotifyIdleRequest, NotifyIdleResponse, PushRequest, PushResponse, SendMessageRequest,
    SendMessageResponse,
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
        if let Err(e) = session.send_keys(&skill_command).await {
            error!(error = %e, "tmux send-keys failed for /implement");
        }

        Ok(Response::new(ImplementResponse {}))
    }

    async fn push(&self, _request: Request<PushRequest>) -> Result<Response<PushResponse>, Status> {
        let skill_command = "/push";
        info!("Push received, sending skill invocation to tmux");

        let session = tmux::Session::agent();
        if let Err(e) = session.send_keys(skill_command).await {
            error!(error = %e, "tmux send-keys failed for /push");
        }

        Ok(Response::new(PushResponse {}))
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
        if let Err(e) = session.send_keys(&skill_command).await {
            error!(error = %e, "tmux send-keys failed for /create-feedback");
        }

        Ok(Response::new(CreateFeedbackResponse {}))
    }
}
