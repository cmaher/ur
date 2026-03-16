use std::collections::HashMap;
use std::path::PathBuf;

use tonic::{Code, Request, Response, Status};
use tracing::info;

use ur_rpc::error::{self, DOMAIN_CORE, INTERNAL, INVALID_ARGUMENT, NOT_FOUND};
use ur_rpc::proto::core::core_service_server::CoreService;
use ur_rpc::proto::core::{
    PingRequest, PingResponse, WorkerInfoRequest, WorkerInfoResponse, WorkerLaunchRequest,
    WorkerLaunchResponse, WorkerListRequest, WorkerListResponse, WorkerStopRequest,
    WorkerStopResponse, WorkerSummary,
};

use crate::{ProcessManager, RepoPoolManager};

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
    pub process_manager: ProcessManager,
    pub repo_pool_manager: RepoPoolManager,
    pub workspace: PathBuf,
    pub proxy_hostname: String,
    pub projects: std::collections::HashMap<String, ur_config::ProjectConfig>,
    #[cfg(feature = "hostexec")]
    pub hostexec_config: crate::hostexec::HostExecConfigManager,
    #[cfg(feature = "hostexec")]
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
            .process_manager
            .resolve_mode(&req.mode)
            .map_err(|e| CoreError::InvalidMode { reason: e })?;

        // Resolve workspace: project_key triggers pool acquire via the strategy,
        // otherwise use the explicit workspace_dir from the request.
        let (workspace_dir, project_key) = if !req.project_key.is_empty() {
            let slot_path = strategy
                .acquire_slot(&self.repo_pool_manager, &req.project_key)
                .await
                .map_err(|e| CoreError::PoolSlotFailed {
                    reason: e.to_string(),
                })?;
            info!(
                worker_id = req.worker_id,
                project_key = req.project_key,
                slot_path = %slot_path.display(),
                strategy = strategy.name(),
                "acquired pool slot"
            );
            (Some(slot_path), req.project_key.clone())
        } else if !req.workspace_dir.is_empty() {
            (Some(PathBuf::from(&req.workspace_dir)), String::new())
        } else {
            (None, String::new())
        };

        // Generate unique worker ID for this launch
        let worker_id = self.process_manager.generate_worker_id(&req.worker_id);
        info!(
            worker_id = req.worker_id,
            internal_worker_id = %worker_id,
            "generated worker ID"
        );

        // Phase 1: prepare (create repo, git init, register)
        self.process_manager
            .prepare(&req.worker_id, &worker_id, workspace_dir.clone())
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
        };
        let (container_id, _agent_secret) = self
            .process_manager
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
        self.process_manager
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
            .process_manager
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
        let summaries = self.process_manager.list().await;
        let workers = summaries
            .into_iter()
            .map(|s| WorkerSummary {
                worker_id: s.process_id,
                worker_id_full: s.worker_id,
                container_id: s.container_id,
                project_key: s.project_key,
                mode: s.mode,
                grpc_port: 0,
            })
            .collect();
        Ok(Response::new(WorkerListResponse { workers }))
    }
}

/// Lightweight CoreService for the worker gRPC server.
///
/// Only implements `Ping` (health check for workers); worker management RPCs
/// return `Unimplemented` because they are host-only operations.
#[derive(Clone)]
pub struct WorkerCoreServiceHandler;

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
}
