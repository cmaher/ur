use std::collections::HashMap;
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

use crate::hostexec::{HostExecConfigManager, LuaTransformManager};
use crate::{ProcessManager, RepoRegistry};

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct HostExecServiceHandler {
    pub config: HostExecConfigManager,
    pub lua: LuaTransformManager,
    pub repo_registry: Arc<RepoRegistry>,
    pub process_manager: ProcessManager,
    pub projects: HashMap<String, ur_config::ProjectConfig>,
    pub hostd_addr: String,
}

#[tonic::async_trait]
impl HostExecService for HostExecServiceHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<HostExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        // Extract agent context from metadata (if present)
        let (process_id, agent_context, config) = self.resolve_request_context(&req)?;

        let req = req.into_inner();

        // 1. Allowlist check
        let cmd_config = config.get(&req.command).ok_or_else(|| {
            warn!(
                command = req.command,
                process_id, "host exec command denied: not in allowlist"
            );
            Status::permission_denied(format!("command not allowed: {}", req.command))
        })?;

        // 2. CWD mapping: /workspace prefix -> host workspace path
        let host_working_dir = self.map_working_dir(&process_id, &req.working_dir)?;

        // 3. Lua transform (if configured)
        let transform_result = if let Some(lua_source) = &cmd_config.lua_source {
            self.lua
                .run_transform(
                    lua_source,
                    &req.command,
                    &req.args,
                    &host_working_dir,
                    agent_context.as_ref(),
                )
                .map_err(|e| Status::invalid_argument(format!("transform rejected: {e}")))?
        } else {
            crate::hostexec::lua_transform::TransformResult {
                command: req.command.clone(),
                args: req.args,
                working_dir: host_working_dir,
                env: std::collections::HashMap::new(),
            }
        };

        info!(
            command = transform_result.command,
            process_id,
            working_dir = transform_result.working_dir,
            args_count = transform_result.args.len(),
            "host exec forwarding to hostd"
        );

        // 4. Forward to ur-hostd
        let mut client = HostDaemonServiceClient::connect(self.hostd_addr.clone())
            .await
            .map_err(|e| Status::unavailable(format!("hostd unavailable: {e}")))?;

        let hostd_req = HostDaemonExecRequest {
            command: transform_result.command,
            args: transform_result.args,
            working_dir: transform_result.working_dir,
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
        // Extract agent context to get per-project merged config
        let (process_id, _agent_context, config) = self.resolve_request_context(&req)?;

        let commands = config.command_names();
        info!(
            process_id,
            command_count = commands.len(),
            "list_commands request"
        );
        Ok(Response::new(ListHostExecCommandsResponse { commands }))
    }
}

impl HostExecServiceHandler {
    /// Extract agent context from request metadata and resolve per-request state.
    ///
    /// Returns `(process_id, agent_context, effective_config)` where:
    /// - `process_id` is resolved from the agent ID (or empty for host-server requests)
    /// - `agent_context` is the Lua-facing context (if the agent has a project/slot)
    /// - `effective_config` is the base hostexec config merged with per-project passthrough commands
    #[allow(clippy::result_large_err)]
    fn resolve_request_context<T>(
        &self,
        req: &Request<T>,
    ) -> Result<
        (
            String,
            Option<crate::hostexec::AgentContext>,
            HostExecConfigManager,
        ),
        Status,
    > {
        let Some(agent_id_val) = req.metadata().get(ur_config::AGENT_ID_HEADER) else {
            // No agent ID header — host-server request (e.g., from `ur` CLI).
            return Ok((String::new(), None, self.config.clone()));
        };

        let agent_id_str = agent_id_val
            .to_str()
            .map_err(|_| Status::invalid_argument("invalid ur-agent-id header encoding"))?;
        let agent_id = crate::AgentId::parse(agent_id_str).map_err(Status::invalid_argument)?;

        // Look up process_id from ProcessManager
        let process_id = self
            .process_manager
            .resolve_process_id(&agent_id)
            .map_err(Status::not_found)?;

        // Look up agent context (project_key, slot_path) from ProcessManager
        let proc_context = self.process_manager.get_agent_context(&agent_id);

        // Build Lua-facing AgentContext and merge per-project passthrough commands
        let (agent_context, config) = match proc_context {
            Some(ref ctx) if ctx.project_key.is_some() => {
                let project_key = ctx.project_key.as_ref().unwrap();
                let lua_ctx = crate::hostexec::AgentContext {
                    agent_id: agent_id_str.to_owned(),
                    project_key: project_key.clone(),
                    slot_path: ctx.slot_path.clone(),
                };

                // Merge per-project passthrough commands
                let extra = self
                    .projects
                    .get(project_key)
                    .map(|p| p.hostexec.as_slice())
                    .unwrap_or_default();
                let merged_config = self.config.with_passthrough_commands(extra);

                (Some(lua_ctx), merged_config)
            }
            Some(ref ctx) => {
                // Agent has a slot but no project — raw workspace mount
                let lua_ctx = crate::hostexec::AgentContext {
                    agent_id: agent_id_str.to_owned(),
                    project_key: String::new(),
                    slot_path: ctx.slot_path.clone(),
                };
                (Some(lua_ctx), self.config.clone())
            }
            None => (None, self.config.clone()),
        };

        Ok((process_id, agent_context, config))
    }

    #[allow(clippy::result_large_err)]
    fn map_working_dir(&self, process_id: &str, container_dir: &str) -> Result<String, Status> {
        let host_base = self
            .repo_registry
            .resolve(process_id)
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

    fn test_process_manager(workspace: &std::path::Path) -> ProcessManager {
        let registry = Arc::new(RepoRegistry::new(workspace.to_path_buf()));
        let config = ur_config::Config {
            config_dir: workspace.to_path_buf(),
            workspace: workspace.to_path_buf(),
            daemon_port: ur_config::DEFAULT_DAEMON_PORT,
            hostd_port: ur_config::DEFAULT_HOSTD_PORT,
            compose_file: workspace.join("docker-compose.yml"),
            proxy: ur_config::ProxyConfig {
                hostname: ur_config::DEFAULT_PROXY_HOSTNAME.into(),
                allowlist: vec![],
            },
            network: ur_config::NetworkConfig {
                name: ur_config::DEFAULT_NETWORK_NAME.into(),
                worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
                server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
                agent_prefix: ur_config::DEFAULT_AGENT_PREFIX.into(),
            },
            hostexec: ur_config::HostExecConfig::default(),
            rag: ur_config::RagConfig {
                qdrant_hostname: ur_config::DEFAULT_QDRANT_HOSTNAME.into(),
                embedding_model: ur_config::DEFAULT_EMBEDDING_MODEL.into(),
                docs: ur_config::RagDocsConfig::default(),
            },
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
            worker_port: ur_config::DEFAULT_DAEMON_PORT + 1,
            projects: HashMap::new(),
        };
        let repo_pool_manager = crate::RepoPoolManager::new(
            &config,
            workspace.to_path_buf(),
            workspace.to_path_buf(),
            crate::HostdClient::new("http://localhost:42070".into()),
        );
        let network_manager = container::NetworkManager::new(
            "docker".into(),
            ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
        );
        let network_config = ur_config::NetworkConfig {
            name: ur_config::DEFAULT_NETWORK_NAME.into(),
            worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
            server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
            agent_prefix: ur_config::DEFAULT_AGENT_PREFIX.into(),
        };
        ProcessManager::new(
            workspace.to_path_buf(),
            workspace.to_path_buf(),
            registry,
            repo_pool_manager,
            network_manager,
            network_config,
            ur_config::DEFAULT_DAEMON_PORT + 1,
            crate::process::PromptModesConfig::default(),
        )
    }

    fn test_handler(process_id: &str, path: &str) -> HostExecServiceHandler {
        let registry = test_registry(process_id, path);
        let tmp = tempfile::tempdir().unwrap();
        let process_manager = test_process_manager(tmp.path());
        HostExecServiceHandler {
            config: HostExecConfigManager::load(
                std::path::Path::new("/nonexistent"),
                &ur_config::HostExecConfig::default(),
            )
            .unwrap(),
            lua: LuaTransformManager::new(),
            repo_registry: registry,
            process_manager,
            projects: HashMap::new(),
            hostd_addr: "http://localhost:42070".into(),
        }
    }

    #[test]
    fn test_map_working_dir_root() {
        let handler = test_handler("test", "/host/workspace/test");
        let result = handler.map_working_dir("test", "/workspace").unwrap();
        assert_eq!(result, "/host/workspace/test");
    }

    #[test]
    fn test_map_working_dir_subdir() {
        let handler = test_handler("test", "/host/workspace/test");
        let result = handler
            .map_working_dir("test", "/workspace/src/main")
            .unwrap();
        assert_eq!(result, "/host/workspace/test/src/main");
    }

    #[test]
    fn test_map_working_dir_rejects_invalid() {
        let handler = test_handler("test", "/host/workspace/test");
        assert!(handler.map_working_dir("test", "/tmp").is_err());
        assert!(handler.map_working_dir("test", "/workspacefoo").is_err());
    }
}
