use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::hostd::HostDaemonExecRequest;
use ur_rpc::proto::hostd::host_daemon_service_client::HostDaemonServiceClient;
use ur_rpc::proto::hostexec::host_exec_service_server::HostExecService;
use ur_rpc::proto::hostexec::{
    HostExecRequest, ListHostExecCommandsRequest, ListHostExecCommandsResponse,
};

use crate::RepoRegistry;
use crate::hostexec::{AgentContext, HostExecConfigManager, LuaTransformManager};

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct HostExecServiceHandler {
    pub config: HostExecConfigManager,
    pub lua: LuaTransformManager,
    pub repo_registry: Arc<RepoRegistry>,
    pub process_id: String,
    pub hostd_addr: String,
    /// Agent context for Lua transforms. `None` for raw workspace mounts (no project association).
    pub agent_context: Option<AgentContext>,
}

#[tonic::async_trait]
impl HostExecService for HostExecServiceHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<HostExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        // Validate agent ID from metadata header (if present)
        if let Some(agent_id_val) = req.metadata().get(ur_config::AGENT_ID_HEADER) {
            let agent_id_str = agent_id_val
                .to_str()
                .map_err(|_| Status::invalid_argument("invalid ur-agent-id header encoding"))?;
            crate::AgentId::parse(agent_id_str).map_err(Status::invalid_argument)?;
        }

        let req = req.into_inner();

        // 1. Allowlist check
        let cmd_config = self.config.get(&req.command).ok_or_else(|| {
            warn!(
                command = req.command,
                process_id = self.process_id,
                "host exec command denied: not in allowlist"
            );
            Status::permission_denied(format!("command not allowed: {}", req.command))
        })?;

        // 2. CWD mapping: /workspace prefix -> host workspace path
        let host_working_dir = self.map_working_dir(&req.working_dir)?;

        // 3. Lua transform (if configured)
        // TODO(ur-bvxe): Use full TransformResult fields (command, working_dir, env)
        let transform_result = if let Some(lua_source) = &cmd_config.lua_source {
            self.lua
                .run_transform(
                    lua_source,
                    &req.command,
                    &req.args,
                    &host_working_dir,
                    self.agent_context.as_ref(),
                )
                .map_err(|e| Status::invalid_argument(format!("transform rejected: {e}")))?
        } else {
            crate::hostexec::lua_transform::TransformResult {
                command: req.command.clone(),
                args: req.args,
                working_dir: host_working_dir.clone(),
                env: std::collections::HashMap::new(),
            }
        };

        info!(
            command = transform_result.command,
            process_id = self.process_id,
            host_working_dir,
            args_count = transform_result.args.len(),
            "host exec forwarding to hostd"
        );

        // 4. Forward to ur-hostd
        let mut client = HostDaemonServiceClient::connect(self.hostd_addr.clone())
            .await
            .map_err(|e| Status::unavailable(format!("hostd unavailable: {e}")))?;

        let hostd_req = HostDaemonExecRequest {
            command: req.command,
            args: transform_result.args,
            working_dir: host_working_dir,
            env: transform_result.env,
        };

        let response = client
            .exec(hostd_req)
            .await
            .map_err(|e| Status::internal(format!("hostd exec failed: {e}")))?;

        // Stream hostd response back to worker
        let mut inbound = response.into_inner();
        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            while let Ok(Some(msg)) = inbound.message().await {
                if tx.send(Ok(msg)).await.is_err() {
                    break;
                }
            }
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::ExecStream))
    }

    async fn list_commands(
        &self,
        req: Request<ListHostExecCommandsRequest>,
    ) -> Result<Response<ListHostExecCommandsResponse>, Status> {
        // Validate agent ID from metadata header (if present)
        if let Some(agent_id_val) = req.metadata().get(ur_config::AGENT_ID_HEADER) {
            let agent_id_str = agent_id_val
                .to_str()
                .map_err(|_| Status::invalid_argument("invalid ur-agent-id header encoding"))?;
            crate::AgentId::parse(agent_id_str).map_err(Status::invalid_argument)?;
        }

        let commands = self.config.command_names();
        info!(
            process_id = self.process_id,
            command_count = commands.len(),
            "list_commands request"
        );
        Ok(Response::new(ListHostExecCommandsResponse { commands }))
    }
}

impl HostExecServiceHandler {
    #[allow(clippy::result_large_err)]
    fn map_working_dir(&self, container_dir: &str) -> Result<String, Status> {
        let host_base = self
            .repo_registry
            .resolve(&self.process_id)
            .map_err(Status::not_found)?;

        let host_base_str = host_base.to_string_lossy();

        // Replace /workspace prefix with host workspace path
        let Some(suffix) = container_dir.strip_prefix("/workspace") else {
            return Err(Status::invalid_argument(format!(
                "working_dir must start with /workspace: {container_dir}"
            )));
        };

        if !suffix.is_empty() && !suffix.starts_with('/') {
            return Err(Status::invalid_argument(format!(
                "invalid working_dir: {container_dir}"
            )));
        }

        let suffix = suffix.strip_prefix('/').unwrap_or(suffix);
        if suffix.is_empty() {
            Ok(host_base_str.into_owned())
        } else {
            Ok(format!("{host_base_str}/{suffix}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_registry(process_id: &str, path: &str) -> Arc<RepoRegistry> {
        let registry = Arc::new(RepoRegistry::new(PathBuf::from("/tmp")));
        registry.register_absolute(process_id, PathBuf::from(path));
        registry
    }

    #[test]
    fn test_map_working_dir_root() {
        let registry = test_registry("test", "/host/workspace/test");
        let handler = HostExecServiceHandler {
            config: HostExecConfigManager::load(
                std::path::Path::new("/nonexistent"),
                &ur_config::HostExecConfig::default(),
            )
            .unwrap(),
            lua: LuaTransformManager::new(),
            repo_registry: registry,
            process_id: "test".into(),
            hostd_addr: "http://localhost:42070".into(),
            agent_context: None,
        };

        let result = handler.map_working_dir("/workspace").unwrap();
        assert_eq!(result, "/host/workspace/test");
    }

    #[test]
    fn test_map_working_dir_subdir() {
        let registry = test_registry("test", "/host/workspace/test");
        let handler = HostExecServiceHandler {
            config: HostExecConfigManager::load(
                std::path::Path::new("/nonexistent"),
                &ur_config::HostExecConfig::default(),
            )
            .unwrap(),
            lua: LuaTransformManager::new(),
            repo_registry: registry,
            process_id: "test".into(),
            hostd_addr: "http://localhost:42070".into(),
            agent_context: None,
        };

        let result = handler.map_working_dir("/workspace/src/main").unwrap();
        assert_eq!(result, "/host/workspace/test/src/main");
    }

    #[test]
    fn test_map_working_dir_rejects_invalid() {
        let registry = test_registry("test", "/host/workspace/test");
        let handler = HostExecServiceHandler {
            config: HostExecConfigManager::load(
                std::path::Path::new("/nonexistent"),
                &ur_config::HostExecConfig::default(),
            )
            .unwrap(),
            lua: LuaTransformManager::new(),
            repo_registry: registry,
            process_id: "test".into(),
            hostd_addr: "http://localhost:42070".into(),
            agent_context: None,
        };

        assert!(handler.map_working_dir("/tmp").is_err());
        assert!(handler.map_working_dir("/workspacefoo").is_err());
    }
}
