//! `WorkerDaemonService` on builderd's port, routed to the local worker named
//! by the `ur-worker-id` metadata header. Unknown id → `NotFound`.

use tonic::{Request, Response, Status};
use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonService;
use ur_rpc::proto::workerd::{
    AddressFeedbackRequest, AddressFeedbackResponse, DesignRequest, DesignResponse,
    DispatchTicketRequest, DispatchTicketResponse, ImplementRequest, ImplementResponse,
    NotifyIdleRequest, NotifyIdleResponse, PauseNudgeRequest, PauseNudgeResponse,
    SendMessageRequest, SendMessageResponse, SetStatusLeftRequest, SetStatusLeftResponse,
    SetTicketRequest, SetTicketResponse, StepCompleteRequest, StepCompleteResponse,
};
use workerd::WorkerDaemonServiceImpl;

use crate::local_worker::LocalWorkerRegistry;

#[derive(Clone)]
pub struct WorkerdRouter {
    pub registry: LocalWorkerRegistry,
}

impl WorkerdRouter {
    fn daemon<T>(&self, request: &Request<T>) -> Result<WorkerDaemonServiceImpl, Status> {
        let id = request
            .metadata()
            .get(ur_config::WORKER_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                Status::invalid_argument(format!("missing {}", ur_config::WORKER_ID_HEADER))
            })?;
        self.registry
            .daemon(id)
            .ok_or_else(|| Status::not_found(format!("no local worker '{id}'")))
    }
}

/// Each method looks up the worker's daemon and delegates to it.
macro_rules! route {
    ($($method:ident($req:ty) -> $resp:ty;)*) => {
        #[tonic::async_trait]
        impl WorkerDaemonService for WorkerdRouter {
            $(
                async fn $method(
                    &self,
                    request: Request<$req>,
                ) -> Result<Response<$resp>, Status> {
                    self.daemon(&request)?.$method(request).await
                }
            )*
        }
    };
}

route! {
    send_message(SendMessageRequest) -> SendMessageResponse;
    notify_idle(NotifyIdleRequest) -> NotifyIdleResponse;
    implement(ImplementRequest) -> ImplementResponse;
    design(DesignRequest) -> DesignResponse;
    address_feedback_tickets(AddressFeedbackRequest) -> AddressFeedbackResponse;
    step_complete(StepCompleteRequest) -> StepCompleteResponse;
    pause_nudge(PauseNudgeRequest) -> PauseNudgeResponse;
    set_ticket(SetTicketRequest) -> SetTicketResponse;
    dispatch_ticket(DispatchTicketRequest) -> DispatchTicketResponse;
    set_status_left(SetStatusLeftRequest) -> SetStatusLeftResponse;
}
