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
    pub workspace: PathBuf,
    /// Fixed container-side gRPC port for per-agent servers.
    pub agent_grpc_port: u16,
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
        self.process_manager
            .prepare(&req.process_id)
            .await
            .map_err(Status::internal)?;

        // Spawn per-agent gRPC server on TCP (OS-assigned port)
        let core_handler = CoreServiceHandler {
            process_manager: self.process_manager.clone(),
            repo_registry: self.repo_registry.clone(),
            workspace: self.workspace.clone(),
            agent_grpc_port: self.agent_grpc_port,
        };

        #[cfg(feature = "git")]
        let git_handler = crate::grpc_git::GitServiceHandler {
            repo_registry: self.repo_registry.clone(),
            process_id: req.process_id.clone(),
        };

        #[cfg(feature = "git")]
        let (grpc_port, server_handle) =
            crate::grpc_server::serve_agent_grpc(core_handler, git_handler)
                .await
                .map_err(|e| Status::internal(format!("failed to start per-agent gRPC: {e}")))?;

        #[cfg(not(feature = "git"))]
        let (grpc_port, server_handle) =
            crate::grpc_server::serve_agent_grpc(core_handler)
                .await
                .map_err(|e| Status::internal(format!("failed to start per-agent gRPC: {e}")))?;

        // Phase 2: run container
        let container_id = self
            .process_manager
            .run_and_record(
                &req.process_id,
                &req.image_id,
                req.cpus,
                &req.memory,
                grpc_port,
                self.agent_grpc_port,
                server_handle,
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
