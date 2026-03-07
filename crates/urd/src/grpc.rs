use std::path::PathBuf;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use ur_rpc::proto::core::core_service_server::CoreService;
use ur_rpc::proto::core::{
    PingRequest, PingResponse, ProcessLaunchRequest, ProcessLaunchResponse, ProcessStopRequest,
    ProcessStopResponse,
};

use crate::bridge::{agent_accept_loop, AgentBridge};
use crate::{ProcessManager, RepoRegistry};

/// gRPC implementation of the CoreService.
#[derive(Clone)]
pub struct CoreServiceHandler {
    pub process_manager: ProcessManager,
    pub repo_registry: Arc<RepoRegistry>,
    /// Will be used by future gRPC services (e.g., container management).
    #[allow(dead_code)]
    pub config_dir: PathBuf,
    /// Will be used by future gRPC services (e.g., container management).
    #[allow(dead_code)]
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

        // Spawn per-agent tarpc accept loop (will be replaced by gRPC in a later ticket)
        let agent_socket_dir = socket_path
            .parent()
            .expect("socket_path must have a parent dir")
            .to_path_buf();
        let agent = AgentBridge {
            repo_registry: self.repo_registry.clone(),
            socket_dir: agent_socket_dir,
            process_id: req.process_id.clone(),
        };
        let sp = socket_path.clone();
        let accept_handle = tokio::spawn(async move {
            if let Err(e) = agent_accept_loop(sp, agent).await {
                tracing::warn!("per-agent accept_loop error: {e}");
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
