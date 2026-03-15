use std::collections::HashMap;
use std::pin::Pin;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::builder::BuilderExecRequest;
use ur_rpc::proto::builder::builder_daemon_service_client::BuilderDaemonServiceClient;
use ur_rpc::proto::hostexec::host_exec_service_server::HostExecService;
use ur_rpc::proto::hostexec::{
    HostExecRequest, ListHostExecCommandsRequest, ListHostExecCommandsResponse,
};

use crate::hostexec::{HostExecConfigManager, LuaTransformManager};
use crate::ProcessManager;

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct HostExecServiceHandler {
    pub config: HostExecConfigManager,
    pub lua: LuaTransformManager,
    pub process_manager: ProcessManager,
    pub projects: HashMap<String, ur_config::ProjectConfig>,
    pub builderd_addr: String,
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

        // 2. CWD mapping: /workspace prefix -> %WORKSPACE% template for builderd resolution
        let host_working_dir = Self::map_working_dir(&req.working_dir)?;

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
            "host exec forwarding to builderd"
        );

        // 4. Forward to builderd
        let mut client = BuilderDaemonServiceClient::connect(self.builderd_addr.clone())
            .await
            .map_err(|e| Status::unavailable(format!("builderd unavailable: {e}")))?;

        let builder_req = BuilderExecRequest {
            command: transform_result.command,
            args: transform_result.args,
            working_dir: transform_result.working_dir,
            env: transform_result.env,
        };

        let response = client
            .exec(builder_req)
            .await
            .map_err(|e| Status::internal(format!("builderd exec failed: {e}")))?;

        // Stream builderd response back to worker
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

    /// Replace `/workspace` prefix with `%WORKSPACE%` template.
    ///
    /// Builderd resolves `%WORKSPACE%` to its local workspace path at exec time,
    /// decoupling the server from knowing builder filesystem layout.
    #[allow(clippy::result_large_err)]
    fn map_working_dir(container_dir: &str) -> Result<String, Status> {
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

        if suffix.is_empty() {
            Ok("%WORKSPACE%".to_owned())
        } else {
            Ok(format!("%WORKSPACE%{suffix}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_working_dir_root() {
        let result = HostExecServiceHandler::map_working_dir("/workspace").unwrap();
        assert_eq!(result, "%WORKSPACE%");
    }

    #[test]
    fn test_map_working_dir_subdir() {
        let result =
            HostExecServiceHandler::map_working_dir("/workspace/src/main").unwrap();
        assert_eq!(result, "%WORKSPACE%/src/main");
    }

    #[test]
    fn test_map_working_dir_rejects_invalid() {
        assert!(HostExecServiceHandler::map_working_dir("/tmp").is_err());
        assert!(HostExecServiceHandler::map_working_dir("/workspacefoo").is_err());
    }
}
