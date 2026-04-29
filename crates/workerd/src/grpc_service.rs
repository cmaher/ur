use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use ur_rpc::proto::core::UpdateAgentStatusRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonService;
use ur_rpc::proto::workerd::{
    AddressFeedbackRequest, AddressFeedbackResponse, DesignRequest, DesignResponse,
    DispatchTicketRequest, DispatchTicketResponse, ImplementRequest, ImplementResponse,
    NotifyIdleRequest, NotifyIdleResponse, PauseNudgeRequest, PauseNudgeResponse,
    SendMessageRequest, SendMessageResponse, SetStatusLeftRequest, SetStatusLeftResponse,
    SetTicketRequest, SetTicketResponse, StepCompleteRequest, StepCompleteResponse,
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
    /// The current lifecycle step name (e.g. "implementing", "addressing_feedback").
    /// Empty string means no active dispatch.
    pub lifecycle_step: String,
    /// When set, nudges are suppressed until this instant.
    /// `pause_nudge` sets this to 5 minutes from now; `step_complete` clears it.
    pub nudge_suppressed_until: Option<Instant>,
}

#[derive(Clone)]
pub struct WorkerDaemonServiceImpl {
    pub server_addr: String,
    pub worker_id: String,
    pub worker_secret: String,
    pub dispatch_buffer: Arc<Mutex<DispatchBuffer>>,
    pub dispatch_ticket_id: Arc<Mutex<Option<String>>>,
    pub is_design_worker: bool,
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

            inject_auth_headers(request.metadata_mut(), &worker_id, &worker_secret);

            match client.update_agent_status(request).await {
                Ok(_) => info!("agent status updated to idle on server"),
                Err(e) => warn!(error = %e, "failed to update agent status on server"),
            }
        });
    }

    /// Send CreateWorkflow + WorkerLaunch RPCs to the server for a dispatched ticket.
    async fn dispatch_ticket_to_server(&self, ticket_id: &str) -> Result<(), String> {
        let addr = format!("http://{}", self.server_addr);
        let worker_id = self.worker_id.clone();
        let worker_secret = self.worker_secret.clone();
        let ticket_id = ticket_id.to_owned();

        // 1. CreateWorkflow
        let channel = tonic::transport::Endpoint::try_from(addr.clone())
            .map_err(|e| e.to_string())?
            .connect()
            .await
            .map_err(|e| format!("failed to connect for CreateWorkflow: {e}"))?;

        let mut ticket_client =
            ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient::new(channel);

        let mut request = tonic::Request::new(ur_rpc::proto::ticket::CreateWorkflowRequest {
            ticket_id: ticket_id.clone(),
            status: ur_rpc::lifecycle::AWAITING_DISPATCH.to_owned(),
        });
        inject_auth_headers(request.metadata_mut(), &worker_id, &worker_secret);

        ticket_client
            .create_workflow(request)
            .await
            .map_err(|e| format!("CreateWorkflow failed: {e}"))?;

        info!(ticket_id = %ticket_id, "CreateWorkflow sent successfully");

        // 2. WorkerLaunch
        let channel = tonic::transport::Endpoint::try_from(addr)
            .map_err(|e| e.to_string())?
            .connect()
            .await
            .map_err(|e| format!("failed to connect for WorkerLaunch: {e}"))?;

        let mut core_client = CoreServiceClient::new(channel);

        let mut request = tonic::Request::new(ur_rpc::proto::core::WorkerLaunchRequest {
            worker_id: ticket_id.clone(),
            ..Default::default()
        });
        inject_auth_headers(request.metadata_mut(), &worker_id, &worker_secret);

        core_client
            .worker_launch(request)
            .await
            .map_err(|e| format!("WorkerLaunch failed: {e}"))?;

        info!(ticket_id = %ticket_id, "WorkerLaunch sent successfully");

        Ok(())
    }

    /// Fire-and-forget: send WorkflowStepComplete RPC to the ur-server.
    fn send_workflow_step_complete(&self) {
        let addr = format!("http://{}", self.server_addr);
        let worker_id = self.worker_id.clone();
        let worker_secret = self.worker_secret.clone();

        tokio::spawn(async move {
            let channel = match tonic::transport::Endpoint::try_from(addr.clone())
                .map(|ep| ep.connect_lazy())
            {
                Ok(ch) => ch,
                Err(e) => {
                    warn!(error = %e, "failed to create channel for WorkflowStepComplete");
                    return;
                }
            };

            let mut client = CoreServiceClient::new(channel);

            let mut request =
                tonic::Request::new(ur_rpc::proto::core::WorkflowStepCompleteRequest {
                    worker_id: worker_id.clone(),
                });

            inject_auth_headers(request.metadata_mut(), &worker_id, &worker_secret);

            match client.workflow_step_complete(request).await {
                Ok(_) => info!("workflow step complete sent to server"),
                Err(e) => warn!(error = %e, "failed to send workflow step complete to server"),
            }
        });
    }
}

/// Inject worker auth headers into gRPC request metadata.
fn inject_auth_headers(
    metadata: &mut tonic::metadata::MetadataMap,
    worker_id: &str,
    worker_secret: &str,
) {
    if let Ok(val) = worker_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>() {
        metadata.insert(ur_config::WORKER_ID_HEADER, val);
    }
    if let Ok(val) = worker_secret.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        metadata.insert(ur_config::WORKER_SECRET_HEADER, val);
    }
}

#[tonic::async_trait]
impl WorkerDaemonService for WorkerDaemonServiceImpl {
    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<SendMessageResponse>, Status> {
        let req = request.into_inner();
        let message = &req.message;
        let submit = req.submit;
        info!(message, submit, "SendMessage received");

        let session = tmux::Session::agent();
        let result = if submit {
            session.send_keys(message).await
        } else {
            session.send_keys_no_enter(message).await
        };
        match result {
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

        // Case 2: Buffer empty + step_complete — clear state and send WorkflowStepComplete RPC
        if buf.step_complete {
            info!("Buffer drained and step complete, sending WorkflowStepComplete to server");
            buf.commands.clear();
            buf.step_complete = false;
            buf.lifecycle_step = String::new();
            drop(buf);

            self.send_workflow_step_complete();
            return Ok(Response::new(NotifyIdleResponse {}));
        }

        // Case 3: Buffer empty + !step_complete + lifecycle_step set — nudge the agent.
        // Design nodes are excluded: the design skill manages its own step-complete
        // signaling, and nudging mid-design just confuses the agent.
        if !buf.lifecycle_step.is_empty() && buf.lifecycle_step != "designing" {
            // If nudge is suppressed, silently return without nudging or forwarding idle.
            if let Some(suppressed_until) = buf.nudge_suppressed_until
                && Instant::now() < suppressed_until
            {
                info!("Nudge suppressed, skipping");
                drop(buf);
                return Ok(Response::new(NotifyIdleResponse {}));
            }

            let step = buf.lifecycle_step.clone();
            info!(
                lifecycle_step = step.as_str(),
                "Buffer drained but step not complete, nudging agent"
            );
            drop(buf);

            let nudge_message = format!(
                "Your '{step}' step is still in progress. Next steps:\n\
                 - Run `workertools status step-complete` to signal completion\n\
                 - Run `workertools status pause-nudge` if you are waiting on a background job or agent\n\
                 - Run `workertools status request-human \"<reason>\"` if you need help"
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
        buf.nudge_suppressed_until = None;
        Ok(Response::new(StepCompleteResponse {}))
    }

    async fn pause_nudge(
        &self,
        _request: Request<PauseNudgeRequest>,
    ) -> Result<Response<PauseNudgeResponse>, Status> {
        info!("PauseNudge received, suppressing nudges for 5 minutes");
        let mut buf = self.dispatch_buffer.lock().await;
        buf.nudge_suppressed_until = Some(Instant::now() + Duration::from_secs(300));
        Ok(Response::new(PauseNudgeResponse {}))
    }

    async fn implement(
        &self,
        request: Request<ImplementRequest>,
    ) -> Result<Response<ImplementResponse>, Status> {
        let ticket_id = &request.into_inner().ticket_id;
        let skill_command = format!("/implement {ticket_id}");
        info!(ticket_id, "Implement received, loading dispatch buffer");

        let mut buf = self.dispatch_buffer.lock().await;
        buf.lifecycle_step = "implementing".to_string();
        buf.step_complete = false;
        buf.nudge_suppressed_until = None;
        buf.commands = VecDeque::from(vec!["/clear".to_string(), skill_command]);

        // Pop the first command and send it immediately
        let first_command = buf.commands.pop_front().expect("commands is non-empty");
        drop(buf);

        let session = tmux::Session::agent();
        if let Err(e) = session.send_keys(&first_command).await {
            error!(error = %e, "tmux send-keys failed for first buffered command");
        }

        Ok(Response::new(ImplementResponse {}))
    }

    async fn design(
        &self,
        request: Request<DesignRequest>,
    ) -> Result<Response<DesignResponse>, Status> {
        let ticket_id = &request.into_inner().ticket_id;
        let skill_command = format!("/design {ticket_id}");
        info!(ticket_id, "Design received, loading dispatch buffer");

        let mut buf = self.dispatch_buffer.lock().await;
        buf.lifecycle_step = "designing".to_string();
        buf.step_complete = false;
        buf.nudge_suppressed_until = None;
        buf.commands = VecDeque::from(vec!["/clear".to_string(), skill_command]);

        // Pop the first command and send it immediately
        let first_command = buf.commands.pop_front().expect("commands is non-empty");
        drop(buf);

        let session = tmux::Session::agent();
        if let Err(e) = session.send_keys(&first_command).await {
            error!(error = %e, "tmux send-keys failed for first buffered command");
        }

        Ok(Response::new(DesignResponse {}))
    }

    async fn address_feedback_tickets(
        &self,
        request: Request<AddressFeedbackRequest>,
    ) -> Result<Response<AddressFeedbackResponse>, Status> {
        let inner = request.into_inner();
        let skill_command = format!("/address-feedback {} {}", inner.ticket_id, inner.pr_number);
        info!(
            ticket_id = inner.ticket_id,
            pr_number = inner.pr_number,
            "AddressFeedbackTickets received, loading dispatch buffer"
        );

        let mut buf = self.dispatch_buffer.lock().await;
        buf.lifecycle_step = "addressing_feedback".to_string();
        buf.step_complete = false;
        buf.nudge_suppressed_until = None;
        buf.commands = VecDeque::from(vec!["/clear".to_string(), skill_command]);

        // Pop the first command and send it immediately
        let first_command = buf.commands.pop_front().expect("commands is non-empty");
        drop(buf);

        let session = tmux::Session::agent();
        if let Err(e) = session.send_keys(&first_command).await {
            error!(error = %e, "tmux send-keys failed for first buffered command");
        }

        Ok(Response::new(AddressFeedbackResponse {}))
    }

    async fn set_ticket(
        &self,
        request: Request<SetTicketRequest>,
    ) -> Result<Response<SetTicketResponse>, Status> {
        if !self.is_design_worker {
            return Err(Status::failed_precondition(
                "SetTicket is only available in design mode",
            ));
        }

        let ticket_id = request.into_inner().ticket_id;
        info!(ticket_id = %ticket_id, "SetTicket received");

        let mut stored = self.dispatch_ticket_id.lock().await;
        *stored = Some(ticket_id);

        Ok(Response::new(SetTicketResponse {}))
    }

    async fn set_status_left(
        &self,
        request: Request<SetStatusLeftRequest>,
    ) -> Result<Response<SetStatusLeftResponse>, Status> {
        let req = request.into_inner();
        info!(
            status_left = req.status_left.as_str(),
            status_left_length = req.status_left_length,
            "SetStatusLeft received"
        );

        let session = tmux::Session::agent();

        if req.status_left_length != 0 {
            if let Err(e) = session
                .set_option("status-left-length", &req.status_left_length.to_string())
                .await
            {
                error!(error = %e, "tmux set-option status-left-length failed");
                return Ok(Response::new(SetStatusLeftResponse {
                    success: false,
                    error: e.to_string(),
                }));
            }
        }

        match session.set_status_left(&req.status_left).await {
            Ok(()) => {
                info!("set-status-left succeeded");
                Ok(Response::new(SetStatusLeftResponse {
                    success: true,
                    error: String::new(),
                }))
            }
            Err(e) => {
                error!(error = %e, "tmux set-status-left failed");
                Ok(Response::new(SetStatusLeftResponse {
                    success: false,
                    error: e.to_string(),
                }))
            }
        }
    }

    async fn dispatch_ticket(
        &self,
        _request: Request<DispatchTicketRequest>,
    ) -> Result<Response<DispatchTicketResponse>, Status> {
        if !self.is_design_worker {
            return Err(Status::failed_precondition(
                "DispatchTicket is only available in design mode",
            ));
        }

        let ticket_id = {
            let stored = self.dispatch_ticket_id.lock().await;
            stored.clone()
        };

        let Some(ticket_id) = ticket_id else {
            info!("DispatchTicket called with no ticket set");
            return Ok(Response::new(DispatchTicketResponse {
                error: "No ticket set. Use `workertools set-ticket <id>` first.".to_string(),
            }));
        };

        info!(ticket_id = %ticket_id, "DispatchTicket received, sending to server");

        match self.dispatch_ticket_to_server(&ticket_id).await {
            Ok(()) => Ok(Response::new(DispatchTicketResponse {
                error: String::new(),
            })),
            Err(e) => {
                error!(error = %e, "DispatchTicket server RPCs failed");
                Ok(Response::new(DispatchTicketResponse { error: e }))
            }
        }
    }
}
