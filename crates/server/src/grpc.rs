use std::collections::HashMap;
use std::path::PathBuf;

use tonic::{Code, Request, Response, Status};
use tracing::{info, warn};

use ur_db::TicketRepo;
use ur_db::model::LifecycleStatus;
use ur_rpc::error::{self, DOMAIN_CORE, INTERNAL, INVALID_ARGUMENT, NOT_FOUND};
use ur_rpc::proto::core::core_service_server::CoreService;
use ur_rpc::proto::core::{
    PingRequest, PingResponse, SendWorkerMessageRequest, SendWorkerMessageResponse,
    UpdateAgentStatusRequest, UpdateAgentStatusResponse, WorkerInfoRequest, WorkerInfoResponse,
    WorkerLaunchRequest, WorkerLaunchResponse, WorkerListRequest, WorkerListResponse,
    WorkerStopRequest, WorkerStopResponse, WorkerSummary,
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
    pub network_config: ur_config::NetworkConfig,
    pub hostexec_config: crate::hostexec::HostExecConfigManager,
    pub builderd_addr: String,
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

        // Resolve worker strategy from the mode field early in the launch flow.
        let (strategy, resolved_skills) = self
            .worker_manager
            .resolve_mode(&req.mode)
            .map_err(|e| CoreError::InvalidMode { reason: e })?;

        // Resolve workspace: project_key triggers pool acquire via the strategy,
        // otherwise use the explicit workspace_dir from the request.
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

        // Generate unique worker ID for this launch
        let worker_id = self.worker_manager.generate_worker_id(&req.worker_id);
        info!(
            worker_id = req.worker_id,
            internal_worker_id = %worker_id,
            "generated worker ID"
        );

        // Checkout a worker-specific branch in pool slots so each worker
        // has its own branch for commits.
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

        // Phase 1: prepare (create repo, git init, register)
        // prepare() returns the resolved workspace path — for the default case this
        // is the newly created git-init'd directory that must be mounted into the
        // container.
        let workspace_dir = self
            .worker_manager
            .prepare(&req.worker_id, &worker_id, workspace_dir)
            .await
            .map_err(|e| CoreError::PrepareFailed {
                reason: e.to_string(),
            })?;

        // Use explicit skills from the request if provided, otherwise use
        // the skills resolved from the strategy/mode.
        let skills = if req.skills.is_empty() {
            resolved_skills
        } else {
            req.skills
        };

        // Phase 2: run container
        let (git_hooks_dir, mounts) = if !project_key.is_empty() {
            let proj = self.projects.get(&project_key);
            (
                proj.and_then(|p| p.git_hooks_dir.clone()),
                proj.map(|p| p.mounts.clone()).unwrap_or_default(),
            )
        } else {
            (None, Vec::new())
        };

        let config = crate::WorkerConfig {
            process_id: req.worker_id,
            worker_id,
            image_id: req.image_id,
            cpus: req.cpus,
            memory: req.memory,
            workspace_dir,
            proxy_hostname: self.proxy_hostname.clone(),
            project_key,
            strategy,
            skills,
            git_hooks_dir,
            mounts,
            slot_id,
        };
        let (container_id, _worker_secret) = self
            .worker_manager
            .run_and_record(config)
            .await
            .map_err(|e| CoreError::RunFailed {
                reason: e.to_string(),
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
        let workers = summaries
            .into_iter()
            .map(|s| WorkerSummary {
                worker_id: s.process_id,
                worker_id_full: s.worker_id,
                container_id: s.container_id,
                project_key: s.project_key,
                mode: s.mode,
                grpc_port: 0,
                directory: s.directory,
                container_status: s.container_status,
                agent_status: s.agent_status,
            })
            .collect();
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
        let workerd_client = crate::WorkerdClient::new(workerd_addr);
        workerd_client
            .send_message(&req.message)
            .await
            .map_err(|e| CoreError::SendMessageFailed { reason: e })?;

        // On success, update agent_status to 'working' in DB.
        self.worker_repo
            .update_worker_agent_status(&worker.worker_id, "working")
            .await
            .map_err(|e| CoreError::SendMessageFailed {
                reason: format!("failed to update agent status: {e}"),
            })?;

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
}

/// Maximum number of idle re-dispatches before a ticket is stalled.
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
            "update_agent_status request received"
        );

        self.worker_repo
            .update_worker_agent_status(&worker_id, &inner.status)
            .await
            .map_err(|e| Status::internal(format!("failed to update agent status: {e}")))?;

        // When a worker goes idle, check if it has an assigned ticket that
        // still needs work and re-dispatch the appropriate RPC.
        if inner.status == "idle" {
            let worker_prefix = self.worker_prefix.clone();
            let worker_repo = self.worker_repo.clone();
            let ticket_repo = self.ticket_repo.clone();
            let wid = worker_id.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    handle_idle_redispatch(&wid, &worker_repo, &ticket_repo, &worker_prefix).await
                {
                    warn!(
                        worker_id = %wid,
                        error = %e,
                        "idle re-dispatch failed"
                    );
                }
            });
        }

        Ok(Response::new(UpdateAgentStatusResponse {}))
    }
}

/// When a worker reports idle, look up its assigned ticket and re-send the
/// appropriate workerd RPC if the ticket's lifecycle_status still matches
/// the phase that worker was dispatched for. Tracks re-dispatch count and
/// stalls the ticket after MAX_IDLE_REDISPATCH failures.
async fn handle_idle_redispatch(
    worker_id: &str,
    worker_repo: &WorkerRepo,
    ticket_repo: &TicketRepo,
    worker_prefix: &str,
) -> Result<(), anyhow::Error> {
    // 1. Find the ticket assigned to this worker via metadata.
    let matched = ticket_repo
        .tickets_by_metadata("worker_id", worker_id)
        .await?;

    // Filter to non-closed tickets only.
    let assigned: Vec<_> = matched.iter().filter(|t| t.status != "closed").collect();

    if assigned.is_empty() {
        info!(
            worker_id = %worker_id,
            "idle worker has no assigned ticket — no-op"
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

    // 3. Determine if lifecycle_status requires a re-dispatch.
    let rpc_kind = match ticket.lifecycle_status {
        LifecycleStatus::Implementing => Some("implement"),
        LifecycleStatus::Pushing => Some("push"),
        LifecycleStatus::FeedbackCreating => Some("create_feedback_tickets"),
        _ => None,
    };

    let Some(rpc_kind) = rpc_kind else {
        info!(
            worker_id = %worker_id,
            ticket_id = %ticket_id,
            lifecycle_status = %ticket.lifecycle_status,
            "ticket lifecycle has moved past dispatch phase — no re-dispatch"
        );
        return Ok(());
    };

    // 4. Increment re-dispatch count and check threshold.
    let count = worker_repo
        .increment_idle_redispatch_count(worker_id)
        .await?;

    if count > MAX_IDLE_REDISPATCH {
        warn!(
            worker_id = %worker_id,
            ticket_id = %ticket_id,
            count = count,
            "idle re-dispatch count exceeded threshold — stalling ticket"
        );
        let update = ur_db::model::TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::Stalled),
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

    // 5. Look up worker to derive workerd address.
    let worker = worker_repo
        .get_worker(worker_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found"))?;

    if worker.container_status != "running" {
        return Ok(());
    }

    let container_name = format!("{}{}", worker_prefix, worker.process_id);
    let workerd_addr = format!("http://{}:{}", container_name, ur_config::WORKERD_GRPC_PORT);
    let workerd_client = crate::WorkerdClient::new(workerd_addr);

    info!(
        worker_id = %worker_id,
        ticket_id = %ticket_id,
        rpc_kind = %rpc_kind,
        count = count,
        "re-dispatching workerd RPC for idle worker"
    );

    // 6. Re-send the appropriate RPC.
    match rpc_kind {
        "implement" => {
            workerd_client
                .implement(ticket_id)
                .await
                .map_err(|e| anyhow::anyhow!("re-dispatch implement failed: {e}"))?;
        }
        "push" => {
            workerd_client
                .push()
                .await
                .map_err(|e| anyhow::anyhow!("re-dispatch push failed: {e}"))?;
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

    // Update agent status to working after successful re-dispatch.
    worker_repo
        .update_worker_agent_status(worker_id, "working")
        .await?;

    Ok(())
}
