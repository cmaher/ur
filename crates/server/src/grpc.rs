use std::collections::HashMap;
use std::path::PathBuf;

use tonic::{Code, Request, Response, Status};
use tracing::{error, info, warn};

use ur_db::TicketRepo;
use ur_db::WorkflowRepo;
use ur_db::model::AgentStatus;
use ur_rpc::error::{self, DOMAIN_CORE, INTERNAL, INVALID_ARGUMENT, NOT_FOUND};
use ur_rpc::proto::core::core_service_server::CoreService;
use ur_rpc::proto::core::{
    PingRequest, PingResponse, SendWorkerMessageRequest, SendWorkerMessageResponse,
    UpdateAgentStatusRequest, UpdateAgentStatusResponse, WorkerInfoRequest, WorkerInfoResponse,
    WorkerLaunchRequest, WorkerLaunchResponse, WorkerListRequest, WorkerListResponse,
    WorkerStopRequest, WorkerStopResponse, WorkerSummary, WorkflowStepCompleteRequest,
    WorkflowStepCompleteResponse,
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
    pub workflow_repo: WorkflowRepo,
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
        let has_pool_slot = slot_id.is_some();
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

        // Bind the ticket to its worker on the workflow table (if a workflow exists).
        self.workflow_repo
            .set_workflow_worker_id(&process_id, &worker_id_str)
            .await
            .map_err(|e| CoreError::RunFailed {
                reason: format!("failed to set workflow worker_id: {e}"),
            })?;

        // Pool slots check out a branch named after the worker ID; persist
        // that on the ticket so the push handler knows which branch to push.
        if has_pool_slot {
            persist_pool_branch(&self.ticket_repo, &process_id, &worker_id_str).await?;
        }

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
            let pr_url = if ticket.is_some() {
                let mut meta = self
                    .ticket_repo
                    .get_meta(&s.process_id, "ticket")
                    .await
                    .unwrap_or_default();
                meta.remove("pr_url").unwrap_or_default()
            } else {
                String::new()
            };
            let workflow = self
                .workflow_repo
                .get_workflow_by_ticket(&s.process_id)
                .await
                .ok()
                .flatten();
            let workflow_stalled = workflow.as_ref().map(|w| w.stalled).unwrap_or(false);
            let workflow_stall_reason = workflow
                .as_ref()
                .map(|w| w.stall_reason.clone())
                .unwrap_or_default();
            let stall_reason = workflow_stall_reason.clone();
            let (workflow_id, workflow_status) = workflow
                .map(|w| (w.id, w.status.to_string()))
                .unwrap_or_default();
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
                workflow_id,
                workflow_status,
                workflow_stalled,
                workflow_stall_reason,
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
            .send_message(&req.message, req.submit)
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

    async fn workflow_step_complete(
        &self,
        _req: Request<WorkflowStepCompleteRequest>,
    ) -> Result<Response<WorkflowStepCompleteResponse>, Status> {
        // Server-side handler will be implemented in a separate ticket (ur-a9b62).
        Err(CoreError::Unimplemented.into())
    }
}

/// Lightweight CoreService for the worker gRPC server.
///
/// Implements `Ping` (health check), `UpdateAgentStatus` (worker status
/// updates), and `WorkflowStepComplete` (workerd-driven step completion);
/// worker management RPCs return `Unimplemented` because they are
/// host-only operations.
#[derive(Clone)]
pub struct WorkerCoreServiceHandler {
    pub worker_repo: WorkerRepo,
    pub ticket_repo: TicketRepo,
    pub workflow_repo: WorkflowRepo,
    /// Docker container name prefix for workers (e.g., `ur-worker-`).
    pub worker_prefix: String,
    /// Channel sender for submitting transition requests to the
    /// WorkflowCoordinator.
    pub transition_tx: tokio::sync::mpsc::Sender<crate::workflow::TransitionRequest>,
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

    async fn workflow_step_complete(
        &self,
        req: Request<WorkflowStepCompleteRequest>,
    ) -> Result<Response<WorkflowStepCompleteResponse>, Status> {
        let metadata = req.metadata();
        let worker_id = metadata
            .get(ur_config::WORKER_ID_HEADER)
            .ok_or_else(|| Status::unauthenticated("missing ur-worker-id header"))?
            .to_str()
            .map_err(|_| Status::invalid_argument("invalid ur-worker-id header encoding"))?
            .to_owned();

        info!(worker_id = %worker_id, "workflow_step_complete request received");

        let workflow_repo = self.workflow_repo.clone();
        let transition_tx = self.transition_tx.clone();
        tokio::spawn(async move {
            if let Err(e) =
                handle_workflow_step_complete(&worker_id, &workflow_repo, &transition_tx).await
            {
                warn!(
                    worker_id = %worker_id,
                    error = %e,
                    "workflow step complete handling failed"
                );
            }
        });

        Ok(Response::new(WorkflowStepCompleteResponse {}))
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

        // AwaitingDispatch readiness trigger: when a worker reports idle and
        // its assigned ticket is in AwaitingDispatch, transition to Implementing.
        if inner.status == ur_rpc::agent_status::IDLE {
            let workflow_repo = self.workflow_repo.clone();
            let transition_tx = self.transition_tx.clone();
            let wid = worker_id.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    handle_awaiting_dispatch_readiness(&wid, &workflow_repo, &transition_tx).await
                {
                    warn!(
                        worker_id = %wid,
                        error = %e,
                        "awaiting dispatch readiness check failed"
                    );
                }
            });
        }

        // When a worker requests human attention, add activity to the assigned ticket.
        if inner.status == ur_rpc::agent_status::STALLED && !inner.message.is_empty() {
            let ticket_repo = self.ticket_repo.clone();
            let workflow_repo = self.workflow_repo.clone();
            let wid = worker_id.clone();
            let message = inner.message.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    handle_request_human_activity(&wid, &ticket_repo, &workflow_repo, &message)
                        .await
                {
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
}

/// Handle a WorkflowStepComplete signal from a worker.
///
/// Persist the pool-slot branch name on the ticket so the push handler knows
/// which branch to push. No-ops if the ticket does not exist (e.g. a bare
/// pool launch without a dispatched ticket).
async fn persist_pool_branch(
    ticket_repo: &TicketRepo,
    ticket_id: &str,
    branch: &str,
) -> Result<(), Status> {
    let ticket = ticket_repo
        .get_ticket(ticket_id)
        .await
        .map_err(|e| CoreError::RunFailed {
            reason: format!("failed to look up ticket: {e}"),
        })?;
    if ticket.is_none() {
        return Ok(());
    }
    let branch_update = ur_db::model::TicketUpdate {
        branch: Some(Some(branch.to_owned())),
        ..Default::default()
    };
    ticket_repo
        .update_ticket(ticket_id, &branch_update)
        .await
        .map_err(|e| CoreError::RunFailed {
            reason: format!("failed to set branch on ticket: {e}"),
        })?;
    info!(
        ticket_id = %ticket_id,
        branch = %branch,
        "persisted branch name on ticket"
    );
    Ok(())
}

/// Looks up the worker's assigned ticket, consults the `WorkerdNextStepRouter`
/// for the current workflow status, and sends a transition request to the
/// coordinator.
async fn handle_workflow_step_complete(
    worker_id: &str,
    workflow_repo: &WorkflowRepo,
    transition_tx: &tokio::sync::mpsc::Sender<crate::workflow::TransitionRequest>,
) -> Result<(), anyhow::Error> {
    use crate::workflow::{NextStepResult, WorkerdNextStepRouter};
    use ur_db::model::LifecycleStatus;

    // 1. Find the ticket assigned to this worker via workflow table.
    // Don't filter by ticket status — closed tickets still need their workflow
    // advanced (e.g., implementing → pushing) so the branch gets pushed and PR'd.
    let matched = workflow_repo
        .tickets_by_workflow_worker_id(worker_id)
        .await?;

    if matched.is_empty() {
        info!(
            worker_id = %worker_id,
            "workflow_step_complete: worker has no assigned ticket — no-op"
        );
        return Ok(());
    }

    let ticket_id = &matched[0].id;

    // 2. Look up the workflow status for this ticket.
    let workflow = workflow_repo
        .get_workflow_by_ticket(ticket_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no workflow found for ticket {ticket_id}"))?;

    // 3. Consult the router.
    let router = WorkerdNextStepRouter;
    let result = router.route(workflow.status);

    match result {
        NextStepResult::Advance { to } => {
            info!(
                worker_id = %worker_id,
                ticket_id = %ticket_id,
                from = %workflow.status,
                to = %to,
                "step complete: advancing lifecycle"
            );
            send_transition(transition_tx, ticket_id, to).await
        }
        NextStepResult::AdvanceByFeedbackMode => {
            // Mark pending comments as feedback_created before advancing.
            // If this fails, we log and continue — the comments will be
            // re-processed on the next FeedbackCreating cycle (safe retry).
            mark_pending_feedback_comments(workflow_repo, ticket_id).await;

            let feedback_mode = &workflow.feedback_mode;
            let to = match feedback_mode.as_str() {
                ur_rpc::feedback_mode::NOW => LifecycleStatus::Implementing,
                _ => LifecycleStatus::Merging,
            };
            info!(
                worker_id = %worker_id,
                ticket_id = %ticket_id,
                feedback_mode = %feedback_mode,
                from = %workflow.status,
                to = %to,
                "step complete: advancing by feedback_mode"
            );
            send_transition(transition_tx, ticket_id, to).await
        }
        NextStepResult::Ignore => {
            info!(
                worker_id = %worker_id,
                ticket_id = %ticket_id,
                workflow_status = %workflow.status,
                "step complete: no routing for current status — ignoring"
            );
            Ok(())
        }
    }
}

/// Mark all pending feedback comments as created for the given ticket.
///
/// Called on successful FeedbackCreating step completion. Queries pending
/// comment IDs (`feedback_created = 0`) and marks them as created. If the
/// worker dies before step completion, comments remain unmarked and will be
/// re-processed on the next FeedbackCreating cycle.
async fn mark_pending_feedback_comments(workflow_repo: &WorkflowRepo, ticket_id: &str) {
    match workflow_repo.get_pending_feedback_comments(ticket_id).await {
        Ok(pending) if pending.is_empty() => {
            info!(
                ticket_id = %ticket_id,
                "no pending feedback comments to mark"
            );
        }
        Ok(pending) => {
            let count = pending.len();
            if let Err(e) = workflow_repo
                .mark_feedback_created(ticket_id, &pending)
                .await
            {
                error!(
                    ticket_id = %ticket_id,
                    error = %e,
                    "failed to mark feedback comments as created"
                );
            } else {
                info!(
                    ticket_id = %ticket_id,
                    count = count,
                    "marked pending feedback comments as created"
                );
            }
        }
        Err(e) => {
            error!(
                ticket_id = %ticket_id,
                error = %e,
                "failed to query pending feedback comments"
            );
        }
    }
}

/// Check if a worker's idle signal should trigger AwaitingDispatch -> Implementing.
///
/// Queries the workflow table for a workflow with status=awaiting_dispatch
/// for the worker's assigned ticket, instead of checking ticket.lifecycle_status.
async fn handle_awaiting_dispatch_readiness(
    worker_id: &str,
    workflow_repo: &WorkflowRepo,
    transition_tx: &tokio::sync::mpsc::Sender<crate::workflow::TransitionRequest>,
) -> Result<(), anyhow::Error> {
    use ur_db::model::LifecycleStatus;

    let matched = workflow_repo
        .tickets_by_workflow_worker_id(worker_id)
        .await?;

    if matched.is_empty() {
        return Ok(());
    }

    let ticket_id = &matched[0].id;

    // Check the workflow table for an awaiting_dispatch workflow.
    let workflow = workflow_repo.get_workflow_by_ticket(ticket_id).await?;

    if let Some(wf) = workflow
        && wf.status == LifecycleStatus::AwaitingDispatch
    {
        info!(
            worker_id = %worker_id,
            ticket_id = %ticket_id,
            "worker idle with awaiting_dispatch workflow — sending transition to implementing"
        );
        send_transition(transition_tx, ticket_id, LifecycleStatus::Implementing).await?;
    }

    Ok(())
}

/// Send a transition request to the WorkflowCoordinator.
async fn send_transition(
    transition_tx: &tokio::sync::mpsc::Sender<crate::workflow::TransitionRequest>,
    ticket_id: &str,
    to: ur_db::model::LifecycleStatus,
) -> Result<(), anyhow::Error> {
    transition_tx
        .send(crate::workflow::TransitionRequest {
            ticket_id: ticket_id.to_owned(),
            target_status: to,
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to send transition request: {e}"))?;
    Ok(())
}

/// When a worker requests human attention, find its assigned ticket and add
/// an activity entry with `source: agent, kind: request-human` metadata.
async fn handle_request_human_activity(
    worker_id: &str,
    ticket_repo: &TicketRepo,
    workflow_repo: &WorkflowRepo,
    message: &str,
) -> Result<(), anyhow::Error> {
    let matched = workflow_repo
        .tickets_by_workflow_worker_id(worker_id)
        .await?;

    if matched.is_empty() {
        warn!(
            worker_id = %worker_id,
            "request-human: worker has no assigned ticket — cannot record activity"
        );
        return Ok(());
    }

    let ticket_id = &matched[0].id;

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
