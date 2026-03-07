use std::path::PathBuf;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use ur_rpc::proto::core::core_service_server::CoreService;
use ur_rpc::proto::core::{
    PingRequest, PingResponse, ProcessLaunchRequest, ProcessLaunchResponse, ProcessStopRequest,
    ProcessStopResponse,
};

use crate::{ProcessManager, RepoRegistry};

/// gRPC implementation of the CoreService.
#[derive(Clone)]
pub struct CoreServiceHandler {
    pub process_manager: ProcessManager,
    pub repo_registry: Arc<RepoRegistry>,
    pub config_dir: PathBuf,
    pub workspace: PathBuf,
}

#[tonic::async_trait]
impl CoreService for CoreServiceHandler {
    async fn ping(&self, _req: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            message: "pong".into(),
        }))
    }

    async fn process_launch(
        &self,
        req: Request<ProcessLaunchRequest>,
    ) -> Result<Response<ProcessLaunchResponse>, Status> {
        let req = req.into_inner();

        // Phase 1: prepare (create repo, git init, register)
        let socket_path = self
            .process_manager
            .prepare(&req.process_id)
            .await
            .map_err(Status::internal)?;

        // Spawn per-agent gRPC server with both CoreService and GitService
        let core_handler = CoreServiceHandler {
            process_manager: self.process_manager.clone(),
            repo_registry: self.repo_registry.clone(),
            config_dir: self.config_dir.clone(),
            workspace: self.workspace.clone(),
        };

        #[cfg(feature = "git")]
        let git_handler = crate::grpc_git::GitServiceHandler {
            repo_registry: self.repo_registry.clone(),
            process_id: req.process_id.clone(),
        };

        let sp = socket_path.clone();
        let accept_handle = tokio::spawn(async move {
            #[cfg(feature = "git")]
            let result =
                crate::grpc_server::serve_grpc_with_git(&sp, core_handler, git_handler).await;

            #[cfg(not(feature = "git"))]
            let result = crate::grpc_server::serve_grpc(&sp, core_handler).await;

            if let Err(e) = result {
                tracing::warn!("per-agent gRPC server error: {e}");
            }
        });

        // Phase 2: run container
        let container_id = self
            .process_manager
            .run_and_record(
                &req.process_id,
                &req.image_id,
                req.cpus,
                &req.memory,
                socket_path,
                accept_handle,
            )
            .await
            .map_err(Status::internal)?;

        Ok(Response::new(ProcessLaunchResponse { container_id }))
    }

    async fn process_stop(
        &self,
        req: Request<ProcessStopRequest>,
    ) -> Result<Response<ProcessStopResponse>, Status> {
        let req = req.into_inner();
        self.process_manager
            .stop(&req.process_id)
            .await
            .map_err(Status::internal)?;
        Ok(Response::new(ProcessStopResponse {}))
    }
}
