use std::collections::HashMap;
use std::path::PathBuf;

use tonic::{Code, Request, Response, Status};
use tracing::{info, warn};

use ur_db::TicketRepo;
use ur_db::model::{AgentStatus, LifecycleStatus};
use ur_rpc::error::{self, DOMAIN_CORE, INTERNAL, INVALID_ARGUMENT, NOT_FOUND};
use ur_rpc::proto::core::core_service_server::CoreService;
use ur_rpc::proto::core::{
    PingRequest, PingResponse, SendWorkerMessageRequest, SendWorkerMessageResponse,
    ShipWorkerRequest, ShipWorkerResponse, UpdateAgentStatusRequest, UpdateAgentStatusResponse,
    WorkerInfoRequest, WorkerInfoResponse, WorkerLaunchRequest, WorkerLaunchResponse,
    WorkerListRequest, WorkerListResponse, WorkerStopRequest, WorkerStopResponse, WorkerSummary,
};

use ur_db::WorkerRepo;

use crate::{RepoPoolManager, WorkerManager};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid mode: {reason}")]
    InvalidMode { reason: String },

    #[error("pool slot acquisition failed: {reason}")]
    PoolSlotFailed { reason: String },

    #[error("prepare failed: {reason}")]
    PrepareFailed { reason: String },

    #[error("run failed: {reason}")]
    RunFailed { reason: String },

    #[error("stop failed: {reason}")]
    StopFailed { reason: String },

    #[error("workspace not found for process: {process_id}")]
    WorkspaceNotFound { process_id: String },

    #[error("worker not found: {worker_id}")]
    WorkerNotFound { worker_id: String },

    #[error("send message failed: {reason}")]
    SendMessageFailed { reason: String },

    #[error("operation not implemented on worker server")]
    Unimplemented,
}

impl From<CoreError> for Status {
    fn from(err: CoreError) -> Self {
        match &err {
            CoreError::InvalidMode { .. } => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_CORE,
                INVALID_ARGUMENT,
                HashMap::new(),
            ),
            CoreError::PoolSlotFailed { .. } => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_CORE,
                INTERNAL,
                HashMap::new(),
            ),
            CoreError::PrepareFailed { .. } => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_CORE,
                INTERNAL,
                HashMap::new(),
            ),
            CoreError::RunFailed { .. } => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_CORE,
                INTERNAL,
                HashMap::new(),
            ),
            CoreError::StopFailed { .. } => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_CORE,
                INTERNAL,
                HashMap::new(),
            ),
            CoreError::WorkerNotFound { worker_id } => {
                let mut meta = HashMap::new();
                meta.insert("worker_id".into(), worker_id.clone());
                error::status_with_info(
                    Code::NotFound,
                    err.to_string(),
                    DOMAIN_CORE,
                    NOT_FOUND,
                    meta,
                )
            }
            CoreError::SendMessageFailed { .. } => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_CORE,
                INTERNAL,
                HashMap::new(),
            ),
            CoreError::WorkspaceNotFound { process_id } => {
                let mut meta = HashMap::new();
                meta.insert("process_id".into(), process_id.clone());
                error::status_with_info(
                    Code::NotFound,
                    err.to_string(),
                    DOMAIN_CORE,
                    NOT_FOUND,
                    meta,
                )
            }
            CoreError::Unimplemented => Status::unimplemented(err.to_string()),
        }
    }
}

/// gRPC implementation of the CoreService.
#[derive(Clone)]
pub struct CoreServiceHandler {
    pub worker_manager: WorkerManager,
    pub repo_pool_manager: RepoPoolManager,
    pub workspace: PathBuf,
    pub proxy_hostname: String,
    pub projects: std::collections::HashMap<String, ur_config::ProjectConfig>,
    pub worker_repo: WorkerRepo,
    pub ticket_repo: TicketRepo,
    pub network_config: ur_config::NetworkConfig,
    pub hostexec_config: crate::hostexec::HostExecConfigManager,
    pub builderd_addr: String,
}

impl CoreServiceHandler {
    /// Resolve workspace, slot, worker ID, skills, and strategy for a launch request.
    ///
    /// Extracted from `worker_launch` to keep method body within the line limit.
    async fn resolve_launch_workspace(
        &self,
        req: &WorkerLaunchRequest,
    ) -> Result<
        (
            Option<PathBuf>,
            String,
            Option<String>,
            crate::WorkerId,
            Vec<String>,
            crate::WorkerStrategy,
        ),
        Status,
    > {
        let (strategy, resolved_skills) = self
            .worker_manager
            .resolve_mode(&req.mode)
            .map_err(|e| CoreError::InvalidMode { reason: e })?;

        let (workspace_dir, project_key, slot_id) = if !req.project_key.is_empty() {
            let (slot_path, slot_id) = strategy
                .acquire_slot(&self.repo_pool_manager, &req.project_key)
                .await
                .map_err(|e| CoreError::PoolSlotFailed {
                    reason: e.to_string(),
                })?;
            info!(
                worker_id = req.worker_id,
                project_key = req.project_key,
                slot_path = %slot_path.display(),
                slot_id = %slot_id,
                strategy = strategy.name(),
                "acquired pool slot"
            );
            (Some(slot_path), req.project_key.clone(), Some(slot_id))
        } else if !req.workspace_dir.is_empty() {
            (Some(PathBuf::from(&req.workspace_dir)), String::new(), None)
        } else {
            (None, String::new(), None)
        };

        let worker_id = self.worker_manager.generate_worker_id(&req.worker_id);
        info!(
            worker_id = req.worker_id,
            internal_worker_id = %worker_id,
            "generated worker ID"
        );

        if let (Some(slot_path), true) = (&workspace_dir, slot_id.is_some()) {
            self.repo_pool_manager
                .checkout_branch(slot_path, &worker_id.to_string())
                .await
                .map_err(|e| CoreError::PoolSlotFailed {
                    reason: e.to_string(),
                })?;
            info!(
                worker_id = %worker_id,
                branch = %worker_id,
                "checked out worker branch in pool slot"
            );
        }

        Ok((
            workspace_dir,
            project_key,
            slot_id,
            worker_id,
            resolved_skills,
            strategy,
        ))
    }
}

#[tonic::async_trait]
impl CoreService for CoreServiceHandler {
    async fn ping(&self, _req: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            message: "pong".into(),
        }))
    }

    async fn worker_launch(
        &self,
        req: Request<WorkerLaunchRequest>,
    ) -> Result<Response<WorkerLaunchResponse>, Status> {
        let req = req.into_inner();

        info!(
            worker_id = req.worker_id,
            image_id = req.image_id,
            workspace_dir = req.workspace_dir,
            project_key = req.project_key,
            "worker_launch request received"
        );

        let (workspace_dir, project_key, slot_id, worker_id, resolved_skills, strategy) =
            self.resolve_launch_workspace(&req).await?;

        // Phase 1: prepare (create repo, git init, register)
        let workspace_dir = self
            .worker_manager
            .prepare(&req.worker_id, &worker_id, workspace_dir)
            .await
            .map_err(|e| CoreError::PrepareFailed {
                reason: e.to_string(),
            })?;

        let skills = if req.skills.is_empty() {
            resolved_skills
        } else {
            req.skills
        };

        let (git_hooks_dir, skill_hooks_dir, mounts, ports, resolved_image) =
            match self.projects.get(&project_key) {
                Some(proj) if !project_key.is_empty() => (
                    proj.git_hooks_dir.clone(),
                    proj.skill_hooks_dir.clone(),
                    proj.container.mounts.clone(),
                    proj.container.ports.clone(),
                    proj.container.image.clone(),
                ),
                _ => (None, None, Vec::new(), Vec::new(), String::new()),
            };

        // Use the image from the request if provided, otherwise fall back to
        // the project's configured image.
        let image_id = if req.image_id.is_empty() {
            if resolved_image.is_empty() {
                "ur-worker-rust:latest".to_owned()
            } else {
                resolved_image
            }
        } else {
            req.image_id
        };

        let process_id = req.worker_id.clone();
        let worker_id_str = worker_id.to_string();
        let config = crate::WorkerConfig {
            process_id: req.worker_id,
            worker_id,
            image_id,
            cpus: req.cpus,
            memory: req.memory,
            workspace_dir,
            proxy_hostname: self.proxy_hostname.clone(),
            project_key,
            strategy,
            skills,
            git_hooks_dir,
            skill_hooks_dir,
            mounts,
            ports,
            slot_id,
        };
        let (container_id, _worker_secret) = self
            .worker_manager
            .run_and_record(config)
            .await
            .map_err(|e| CoreError::RunFailed {
                reason: e.to_string(),
            })?;

        // Bind the ticket to its worker for workflow handler lookups.
        self.ticket_repo
            .set_meta(&process_id, "ticket", "worker_id", &worker_id_str)
            .await
            .map_err(|e| CoreError::RunFailed {
                reason: format!("failed to set worker_id metadata: {e}"),
            })?;

        Ok(Response::new(WorkerLaunchResponse { container_id }))
    }

    async fn worker_stop(
        &self,
        req: Request<WorkerStopRequest>,
    ) -> Result<Response<WorkerStopResponse>, Status> {
        let req = req.into_inner();
        info!(worker_id = req.worker_id, "worker_stop request received");
        self.worker_manager
            .stop(&req.worker_id)
            .await
            .map_err(|e| CoreError::StopFailed {
                reason: e.to_string(),
            })?;
        Ok(Response::new(WorkerStopResponse {}))
    }

    async fn worker_info(
        &self,
        req: Request<WorkerInfoRequest>,
    ) -> Result<Response<WorkerInfoResponse>, Status> {
        let req = req.into_inner();
        info!(worker_id = req.worker_id, "worker_info request received");
        let workspace_dir = self
            .worker_manager
            .get_workspace_dir(&req.worker_id)
            .await
            .map_err(|_| CoreError::WorkspaceNotFound {
                process_id: req.worker_id.clone(),
            })?;
        let workspace_dir = workspace_dir
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Ok(Response::new(WorkerInfoResponse { workspace_dir }))
    }

    async fn worker_list(
        &self,
        _req: Request<WorkerListRequest>,
    ) -> Result<Response<WorkerListResponse>, Status> {
        info!("worker_list request received");
        let summaries = self.worker_manager.list().await;
        let mut workers = Vec::with_capacity(summaries.len());
        for s in summaries {
            let ticket = self
                .ticket_repo
                .get_ticket(&s.process_id)
                .await
                .ok()
                .flatten();
            let lifecycle_status = ticket
                .as_ref()
                .map(|t| t.lifecycle_status.to_string())
                .unwrap_or_default();
            let (stall_reason, pr_url) = if ticket.is_some() {
                let mut meta = self
                    .ticket_repo
                    .get_meta(&s.process_id, "ticket")
                    .await
                    .unwrap_or_default();
                (
                    meta.remove("stall_reason").unwrap_or_default(),
                    meta.remove("pr_url").unwrap_or_default(),
                )
            } else {
                (String::new(), String::new())
            };
            workers.push(WorkerSummary {
                worker_id: s.process_id,
                worker_id_full: s.worker_id,
                container_id: s.container_id,
                project_key: s.project_key,
                mode: s.mode,
                grpc_port: 0,
                directory: s.directory,
                container_status: s.container_status,
                agent_status: s.agent_status,
                lifecycle_status,
                stall_reason,
                pr_url,
            });
        }
        Ok(Response::new(WorkerListResponse { workers }))
    }

    async fn send_worker_message(
        &self,
        req: Request<SendWorkerMessageRequest>,
    ) -> Result<Response<SendWorkerMessageResponse>, Status> {
        let req = req.into_inner();
        info!(
            worker_id = req.worker_id,
            "send_worker_message request received"
        );

        // Look up the worker by process_id (the CLI-facing ID).
        let workers = self
            .worker_repo
            .list_workers_by_container_status("running")
            .await
            .map_err(|e| CoreError::SendMessageFailed {
                reason: format!("db error: {e}"),
            })?;
        let worker = workers
            .into_iter()
            .find(|w| w.process_id == req.worker_id)
            .ok_or_else(|| CoreError::WorkerNotFound {
                worker_id: req.worker_id.clone(),
            })?;

        // Derive the workerd gRPC address from container name + fixed port.
        // Container name = worker_prefix + process_id (same as at creation time).
        let container_name = format!("{}{}", self.network_config.worker_prefix, worker.process_id);
        let workerd_addr = format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);

        // Forward the message to workerd.
        let workerd_client = crate::WorkerdClient::with_status_tracking(
            workerd_addr,
            self.worker_repo.clone(),
            worker.worker_id.clone(),
        );
        workerd_client
            .send_message(&req.message)
            .await
            .map_err(|e| CoreError::SendMessageFailed { reason: e })?;

        Ok(Response::new(SendWorkerMessageResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn update_agent_status(
        &self,
        _req: Request<UpdateAgentStatusRequest>,
    ) -> Result<Response<UpdateAgentStatusResponse>, Status> {
        Err(CoreError::Unimplemented.into())
    }

    async fn ship_worker(
        &self,
        _req: Request<ShipWorkerRequest>,
    ) -> Result<Response<ShipWorkerResponse>, Status> {
        Err(CoreError::Unimplemented.into())
    }
}

/// Maximum number of idle re-dispatches before a ticket is reverted to open.
const MAX_IDLE_REDISPATCH: i32 = 3;

/// Lightweight CoreService for the worker gRPC server.
///
/// Implements `Ping` (health check) and `UpdateAgentStatus` (worker status
/// updates); worker management RPCs return `Unimplemented` because they are
/// host-only operations.
#[derive(Clone)]
pub struct WorkerCoreServiceHandler {
    pub worker_repo: WorkerRepo,
    pub ticket_repo: TicketRepo,
    /// Docker container name prefix for workers (e.g., `ur-worker-`).
    pub worker_prefix: String,
    pub step_router: crate::workflow::LifecycleStepRouter,
}

#[tonic::async_trait]
impl CoreService for WorkerCoreServiceHandler {
    async fn ping(&self, _req: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            message: "pong".into(),
        }))
    }

    async fn worker_launch(
        &self,
        _req: Request<WorkerLaunchRequest>,
    ) -> Result<Response<WorkerLaunchResponse>, Status> {
        Err(CoreError::Unimplemented.into())
    }

    async fn worker_stop(
        &self,
        _req: Request<WorkerStopRequest>,
    ) -> Result<Response<WorkerStopResponse>, Status> {
        Err(CoreError::Unimplemented.into())
    }

    async fn worker_info(
        &self,
        _req: Request<WorkerInfoRequest>,
    ) -> Result<Response<WorkerInfoResponse>, Status> {
        Err(CoreError::Unimplemented.into())
    }

    async fn worker_list(
        &self,
        _req: Request<WorkerListRequest>,
    ) -> Result<Response<WorkerListResponse>, Status> {
        Err(CoreError::Unimplemented.into())
    }

    async fn send_worker_message(
        &self,
        _req: Request<SendWorkerMessageRequest>,
    ) -> Result<Response<SendWorkerMessageResponse>, Status> {
        Err(CoreError::Unimplemented.into())
    }

    async fn update_agent_status(
        &self,
        req: Request<UpdateAgentStatusRequest>,
    ) -> Result<Response<UpdateAgentStatusResponse>, Status> {
        let metadata = req.metadata();
        let worker_id = metadata
            .get(ur_config::WORKER_ID_HEADER)
            .ok_or_else(|| Status::unauthenticated("missing ur-worker-id header"))?
            .to_str()
            .map_err(|_| Status::invalid_argument("invalid ur-worker-id header encoding"))?
            .to_owned();

        let inner = req.into_inner();
        info!(
            worker_id = worker_id,
            status = inner.status,
            message = inner.message,
            "update_agent_status request received"
        );

        let agent_status: AgentStatus = inner
            .status
            .parse()
            .map_err(|e: String| Status::invalid_argument(e))?;

        self.worker_repo
            .update_worker_agent_status(&worker_id, agent_status)
            .await
            .map_err(|e| Status::internal(format!("failed to update agent status: {e}")))?;

        // Delegate routing to LifecycleStepRouter for lifecycle-aware actions.
        {
            let step_router = self.step_router.clone();
            let worker_prefix = self.worker_prefix.clone();
            let worker_repo = self.worker_repo.clone();
            let ticket_repo = self.ticket_repo.clone();
            let agent_status = inner.status.clone();
            let wid = worker_id.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_agent_status_routed(
                    &wid,
                    &agent_status,
                    &step_router,
                    &worker_repo,
                    &ticket_repo,
                    &worker_prefix,
                )
                .await
                {
                    warn!(
                        worker_id = %wid,
                        error = %e,
                        "agent status routing failed"
                    );
                }
            });
        }

        // When a worker requests human attention, add activity to the assigned ticket.
        if inner.status == ur_rpc::agent_status::STALLED && !inner.message.is_empty() {
            let ticket_repo = self.ticket_repo.clone();
            let wid = worker_id.clone();
            let message = inner.message.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_request_human_activity(&wid, &ticket_repo, &message).await {
                    warn!(
                        worker_id = %wid,
                        error = %e,
                        "request-human activity recording failed"
                    );
                }
            });
        }

        Ok(Response::new(UpdateAgentStatusResponse {}))
    }

    async fn ship_worker(
        &self,
        _req: Request<ShipWorkerRequest>,
    ) -> Result<Response<ShipWorkerResponse>, Status> {
        Err(CoreError::Unimplemented.into())
    }
}

/// Route an agent status update through the `LifecycleStepRouter` and execute
/// the resulting `StepAction`.
///
/// Looks up the worker's assigned ticket, consults the router for the
/// `(lifecycle_status, agent_status)` pair, and performs the action:
/// - `Advance`: emits a workflow event to transition the ticket.
/// - `Redispatch`: re-sends the phase-appropriate workerd RPC (with idle
///   re-dispatch counting and threshold enforcement).
/// - `Ignore`: no-op.
async fn handle_agent_status_routed(
    worker_id: &str,
    agent_status: &str,
    step_router: &crate::workflow::LifecycleStepRouter,
    worker_repo: &WorkerRepo,
    ticket_repo: &TicketRepo,
    worker_prefix: &str,
) -> Result<(), anyhow::Error> {
    use crate::workflow::StepAction;

    // 1. Find the ticket assigned to this worker via metadata.
    let matched = ticket_repo
        .tickets_by_metadata("worker_id", worker_id)
        .await?;

    // Filter to non-closed tickets only.
    let assigned: Vec<_> = matched.iter().filter(|t| t.status != "closed").collect();

    let has_ticket = !assigned.is_empty();

    if !has_ticket {
        // Consult router even with no ticket — it returns Ignore for cold starts.
        let action = step_router.route(LifecycleStatus::Open, agent_status, false);
        if action != StepAction::Ignore {
            warn!(
                worker_id = %worker_id,
                agent_status = %agent_status,
                ?action,
                "unexpected non-ignore action for worker with no ticket"
            );
        }
        info!(
            worker_id = %worker_id,
            "worker has no assigned ticket — no-op"
        );
        return Ok(());
    }

    // Use the first (highest-priority) assigned ticket.
    let ticket_id = &assigned[0].id;

    // 2. Load the full ticket to get lifecycle_status.
    let ticket = ticket_repo
        .get_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ticket {ticket_id} not found"))?;

    // 2b. Worker readiness trigger: when a worker reports idle and its
    // assigned ticket is in AwaitingDispatch, transition directly to
    // Implementing. This fires the SQLite trigger which creates a
    // workflow event for the AwaitingDispatch→Implementing handler.
    if agent_status == ur_rpc::agent_status::IDLE
        && ticket.lifecycle_status == LifecycleStatus::AwaitingDispatch
    {
        info!(
            worker_id = %worker_id,
            ticket_id = %ticket_id,
            "worker idle with awaiting_dispatch ticket — transitioning to implementing"
        );
        advance_lifecycle(ticket_repo, ticket_id, LifecycleStatus::Implementing).await?;
        return Ok(());
    }

    // 3. Consult the router.
    let action = step_router.route(ticket.lifecycle_status, agent_status, true);

    match action {
        StepAction::Ignore => {
            info!(
                worker_id = %worker_id,
                ticket_id = %ticket_id,
                lifecycle_status = %ticket.lifecycle_status,
                agent_status = %agent_status,
                "step router returned Ignore — no action"
            );
            Ok(())
        }
        StepAction::Advance { to } => {
            info!(
                worker_id = %worker_id,
                ticket_id = %ticket_id,
                from = %ticket.lifecycle_status,
                to = %to,
                "step router: advancing lifecycle"
            );
            advance_lifecycle(ticket_repo, ticket_id, to).await
        }
        StepAction::AdvanceByFeedbackMode => {
            let meta = ticket_repo.get_meta(ticket_id, "ticket").await?;
            let feedback_mode = meta.get("feedback_mode").map(|s| s.as_str()).unwrap_or("");
            let to = match feedback_mode {
                ur_rpc::feedback_mode::NOW => LifecycleStatus::Implementing,
                _ => LifecycleStatus::Merging,
            };
            info!(
                worker_id = %worker_id,
                ticket_id = %ticket_id,
                feedback_mode = %feedback_mode,
                from = %ticket.lifecycle_status,
                to = %to,
                "step router: advancing by feedback_mode"
            );
            advance_lifecycle(ticket_repo, ticket_id, to).await
        }
        StepAction::Redispatch { reminder } => {
            handle_redispatch(
                worker_id,
                ticket_id,
                &ticket.lifecycle_status,
                reminder,
                worker_repo,
                ticket_repo,
                worker_prefix,
            )
            .await
        }
    }
}

/// Update a ticket's lifecycle_status to the given value.
async fn advance_lifecycle(
    ticket_repo: &TicketRepo,
    ticket_id: &str,
    to: LifecycleStatus,
) -> Result<(), anyhow::Error> {
    let update = ur_db::model::TicketUpdate {
        lifecycle_status: Some(to),
        lifecycle_managed: None,
        status: None,
        type_: None,
        priority: None,
        title: None,
        body: None,
        branch: None,
        parent_id: None,
        project: None,
    };
    ticket_repo.update_ticket(ticket_id, &update).await?;
    Ok(())
}

/// Execute a redispatch: re-send the phase-appropriate workerd RPC.
///
/// Tracks re-dispatch count and reverts the ticket to open after
/// `MAX_IDLE_REDISPATCH` failures.
async fn handle_redispatch(
    worker_id: &str,
    ticket_id: &str,
    lifecycle_status: &LifecycleStatus,
    reminder: bool,
    worker_repo: &WorkerRepo,
    ticket_repo: &TicketRepo,
    worker_prefix: &str,
) -> Result<(), anyhow::Error> {
    // Determine the RPC kind from the lifecycle status.
    let rpc_kind = match lifecycle_status {
        LifecycleStatus::Implementing => "implement",
        LifecycleStatus::FeedbackCreating => "create_feedback_tickets",
        _ => {
            info!(
                worker_id = %worker_id,
                ticket_id = %ticket_id,
                lifecycle_status = %lifecycle_status,
                reminder = reminder,
                "redispatch requested but no RPC mapping for lifecycle status — skipping"
            );
            return Ok(());
        }
    };

    // Increment re-dispatch count and check threshold (only for non-reminder redispatches).
    if !reminder {
        let count = worker_repo
            .increment_idle_redispatch_count(worker_id)
            .await?;

        if count > MAX_IDLE_REDISPATCH {
            warn!(
                worker_id = %worker_id,
                ticket_id = %ticket_id,
                count = count,
                "idle re-dispatch count exceeded threshold — reverting ticket to open"
            );
            let update = ur_db::model::TicketUpdate {
                lifecycle_status: Some(LifecycleStatus::Open),
                lifecycle_managed: None,
                status: None,
                type_: None,
                priority: None,
                title: None,
                body: None,
                branch: None,
                parent_id: None,
                project: None,
            };
            ticket_repo.update_ticket(ticket_id, &update).await?;
            return Ok(());
        }
    }

    // Look up worker to derive workerd address.
    let worker = worker_repo
        .get_worker(worker_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found"))?;

    if worker.container_status != "running" {
        return Ok(());
    }

    let container_name = format!("{}{}", worker_prefix, worker.process_id);
    let workerd_addr = format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);
    let workerd_client = crate::WorkerdClient::with_status_tracking(
        workerd_addr,
        worker_repo.clone(),
        worker_id.to_string(),
    );

    info!(
        worker_id = %worker_id,
        ticket_id = %ticket_id,
        rpc_kind = %rpc_kind,
        reminder = reminder,
        "re-dispatching workerd RPC"
    );

    // Re-send the appropriate RPC.
    match rpc_kind {
        "implement" => {
            workerd_client
                .implement(ticket_id)
                .await
                .map_err(|e| anyhow::anyhow!("re-dispatch implement failed: {e}"))?;
        }
        "create_feedback_tickets" => {
            let meta = ticket_repo.get_meta(ticket_id, "ticket").await?;
            let pr_number: u32 = meta
                .get("pr_number")
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no pr_number metadata on ticket {ticket_id} for feedback re-dispatch"
                    )
                })?
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid pr_number: {e}"))?;
            workerd_client
                .create_feedback_tickets(ticket_id, pr_number)
                .await
                .map_err(|e| anyhow::anyhow!("re-dispatch create_feedback_tickets failed: {e}"))?;
        }
        _ => unreachable!(),
    }

    Ok(())
}

/// When a worker requests human attention, find its assigned ticket and add
/// an activity entry with `source: agent, kind: request-human` metadata.
async fn handle_request_human_activity(
    worker_id: &str,
    ticket_repo: &TicketRepo,
    message: &str,
) -> Result<(), anyhow::Error> {
    let matched = ticket_repo
        .tickets_by_metadata("worker_id", worker_id)
        .await?;

    let assigned: Vec<_> = matched.iter().filter(|t| t.status != "closed").collect();

    if assigned.is_empty() {
        warn!(
            worker_id = %worker_id,
            "request-human: worker has no assigned ticket — cannot record activity"
        );
        return Ok(());
    }

    let ticket_id = &assigned[0].id;

    let activity = ticket_repo
        .add_activity(ticket_id, "agent", message)
        .await?;

    ticket_repo
        .set_meta(&activity.id, "activity", "source", "agent")
        .await?;
    ticket_repo
        .set_meta(&activity.id, "activity", "kind", "request-human")
        .await?;

    info!(
        worker_id = %worker_id,
        ticket_id = %ticket_id,
        activity_id = %activity.id,
        "recorded request-human activity on ticket"
    );

    Ok(())
}
