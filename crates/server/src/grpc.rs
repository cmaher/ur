use std::path::PathBuf;
use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::info;

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
    #[cfg(feature = "hostexec")]
    pub hostexec_config: crate::hostexec::HostExecConfigManager,
    #[cfg(feature = "hostexec")]
    pub hostd_addr: String,
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

        info!(
            process_id = req.process_id,
            image_id = req.image_id,
            workspace_dir = req.workspace_dir,
            "process_launch request received"
        );

        // Parse workspace_dir: empty string means None
        let workspace_dir = if req.workspace_dir.is_empty() {
            None
        } else {
            Some(PathBuf::from(&req.workspace_dir))
        };

        // Generate unique agent ID for this launch
        let agent_id = self.process_manager.generate_agent_id(&req.process_id);
        info!(
            process_id = req.process_id,
            agent_id = %agent_id,
            "generated agent ID"
        );

        // Phase 1: prepare (create repo, git init, register)
        self.process_manager
            .prepare(&req.process_id, &agent_id, workspace_dir.clone())
            .await
            .map_err(Status::internal)?;

        // Spawn per-agent gRPC server on TCP bound to 0.0.0.0 (reachable via
        // Docker network; network isolation handled by Docker network membership).
        let core_handler = CoreServiceHandler {
            process_manager: self.process_manager.clone(),
            repo_registry: self.repo_registry.clone(),
            workspace: self.workspace.clone(),
            proxy_hostname: self.proxy_hostname.clone(),
            #[cfg(feature = "hostexec")]
            hostexec_config: self.hostexec_config.clone(),
            #[cfg(feature = "hostexec")]
            hostd_addr: self.hostd_addr.clone(),
        };

        let bind_host = "0.0.0.0";
        let (grpc_port, server_handle) =
            crate::grpc_server::serve_agent_grpc(bind_host, core_handler, &req.process_id)
                .await
                .map_err(|e| Status::internal(format!("failed to start per-agent gRPC: {e}")))?;

        // Resolve skills from request template/skills fields
        let skills = self
            .process_manager
            .resolve_skills(&req.template, &req.skills)
            .map_err(Status::invalid_argument)?;

        // Phase 2: run container
        let config = crate::ProcessConfig {
            process_id: req.process_id,
            agent_id,
            image_id: req.image_id,
            cpus: req.cpus,
            memory: req.memory,
            grpc_port,
            workspace_dir,
            proxy_hostname: self.proxy_hostname.clone(),
            project_key: String::new(),
            skills,
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
        info!(process_id = req.process_id, "process_stop request received");
        self.process_manager
            .stop(&req.process_id)
            .await
            .map_err(Status::internal)?;
        Ok(Response::new(ProcessStopResponse {}))
    }
}
