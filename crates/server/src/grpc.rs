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
    pub proxy_hostname: String,
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

        // Parse workspace_dir: empty string means None
        let workspace_dir = if req.workspace_dir.is_empty() {
            None
        } else {
            Some(PathBuf::from(&req.workspace_dir))
        };

        // Phase 1: prepare (create repo, git init, register)
        self.process_manager
            .prepare(&req.process_id, workspace_dir.clone())
            .await
            .map_err(Status::internal)?;

        // Spawn per-agent gRPC server on TCP bound to 0.0.0.0 (reachable via
        // Docker network; network isolation handled by Docker network membership).
        let core_handler = CoreServiceHandler {
            process_manager: self.process_manager.clone(),
            repo_registry: self.repo_registry.clone(),
            workspace: self.workspace.clone(),
            proxy_hostname: self.proxy_hostname.clone(),
        };

        let bind_host = "0.0.0.0";
        let (grpc_port, server_handle) =
            crate::grpc_server::serve_agent_grpc(bind_host, core_handler, &req.process_id)
                .await
                .map_err(|e| Status::internal(format!("failed to start per-agent gRPC: {e}")))?;

        // Phase 2: run container
        let config = crate::ProcessConfig {
            process_id: req.process_id,
            image_id: req.image_id,
            cpus: req.cpus,
            memory: req.memory,
            grpc_port,
            workspace_dir,
            proxy_hostname: self.proxy_hostname.clone(),
        };
        let container_id = self
            .process_manager
            .run_and_record(config, server_handle)
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
