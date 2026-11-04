//! `BuilderLocalWorkerService`: proto adapter over [`LocalWorkerManager`].

use std::path::PathBuf;

use tonic::{Request, Response, Status};
use ur_config::AgentType;
use ur_rpc::proto::builder_local_worker::builder_local_worker_service_server::BuilderLocalWorkerService;
use ur_rpc::proto::builder_local_worker::{
    LaunchLocalWorkerRequest, LaunchLocalWorkerResponse, ListLocalWorkersRequest,
    ListLocalWorkersResponse, LocalWorkerStatus, StopLocalWorkerRequest, StopLocalWorkerResponse,
};

use crate::local_worker::{LocalWorkerManager, session_name};
use crate::local_workspace::{LocalMount, LocalWorkspaceRequest};

#[derive(Clone)]
pub struct BuilderLocalWorkerHandler {
    pub manager: LocalWorkerManager,
}

fn non_empty(s: String) -> Option<String> {
    (!s.trim().is_empty()).then_some(s)
}

/// Map the wire request onto the workspace request and session env.
fn workspace_request(
    req: LaunchLocalWorkerRequest,
) -> Result<(LocalWorkspaceRequest, Vec<(String, String)>), Status> {
    if req.worker_id.is_empty() || req.slot_path.is_empty() {
        return Err(Status::invalid_argument(
            "worker_id and slot_path are required",
        ));
    }
    let agent = AgentType::parse(&req.agent_type)
        .map_err(|e| Status::invalid_argument(format!("agent_type: {e}")))?;
    let env = req.env.into_iter().map(|e| (e.key, e.value)).collect();
    let workspace = LocalWorkspaceRequest {
        worker_id: req.worker_id,
        slot_path: PathBuf::from(req.slot_path),
        agent,
        model: non_empty(req.model),
        effort: req.effort,
        mounts: req
            .mounts
            .into_iter()
            .map(|m| LocalMount {
                source: PathBuf::from(m.source),
                dest: m.dest,
                read_only: m.read_only,
            })
            .collect(),
        memory_dir: non_empty(req.memory_dir).map(PathBuf::from),
        brain_dir: non_empty(req.brain_dir).map(PathBuf::from),
        skills: req.skills,
        instruction_strategy: non_empty(req.instruction_strategy),
        project_key: req.project_key,
        project_instruction_path: non_empty(req.project_instruction_path).map(PathBuf::from),
        allow_no_verify: req.allow_no_verify,
    };
    Ok((workspace, env))
}

#[tonic::async_trait]
impl BuilderLocalWorkerService for BuilderLocalWorkerHandler {
    async fn launch_local_worker(
        &self,
        request: Request<LaunchLocalWorkerRequest>,
    ) -> Result<Response<LaunchLocalWorkerResponse>, Status> {
        let (req, env) = workspace_request(request.into_inner())?;
        let drive = self.manager.launch(&req, env).await.map_err(|e| {
            Status::new(
                e.code(),
                format!("launch local worker '{}': {}", req.worker_id, e.message()),
            )
        })?;
        Ok(Response::new(LaunchLocalWorkerResponse {
            session_name: session_name(&req.worker_id),
            drive: drive.to_string_lossy().into_owned(),
        }))
    }

    async fn stop_local_worker(
        &self,
        request: Request<StopLocalWorkerRequest>,
    ) -> Result<Response<StopLocalWorkerResponse>, Status> {
        let worker_id = request.into_inner().worker_id;
        self.manager.stop(&worker_id).await.map_err(|e| {
            Status::new(
                e.code(),
                format!("stop local worker '{worker_id}': {}", e.message()),
            )
        })?;
        Ok(Response::new(StopLocalWorkerResponse {}))
    }

    async fn list_local_workers(
        &self,
        _request: Request<ListLocalWorkersRequest>,
    ) -> Result<Response<ListLocalWorkersResponse>, Status> {
        let workers = self
            .manager
            .list()
            .await?
            .into_iter()
            .map(|(worker_id, session_alive)| LocalWorkerStatus {
                worker_id,
                session_alive,
            })
            .collect();
        Ok(Response::new(ListLocalWorkersResponse { workers }))
    }
}
