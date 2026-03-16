use std::collections::HashMap;
use std::pin::Pin;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Response, Status};
use tracing::{info, warn};

use ur_rpc::error::{
    self, BUILDERD_UNAVAILABLE, COMMAND_NOT_ALLOWED, DOMAIN_HOSTEXEC, INTERNAL, INVALID_ARGUMENT,
    NOT_FOUND, TRANSFORM_REJECTED,
};
use ur_rpc::proto::builder::BuilderExecRequest;
use ur_rpc::proto::builder::builder_daemon_service_client::BuilderDaemonServiceClient;
use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::hostexec::host_exec_service_server::HostExecService;
use ur_rpc::proto::hostexec::{
    HostExecRequest, ListHostExecCommandsRequest, ListHostExecCommandsResponse,
};

use crate::WorkerManager;
use crate::hostexec::{HostExecConfigManager, LuaTransformManager};

#[derive(Debug, thiserror::Error)]
pub enum HostExecError {
    #[error("command not allowed: {command}")]
    CommandNotAllowed { command: String },

    #[error("transform rejected: {reason}")]
    TransformRejected { reason: String },

    #[error("builderd unavailable")]
    BuilderdUnavailable,

    #[error("builderd exec failed: {message}")]
    BuilderdExecFailed { message: String },

    #[error("invalid worker ID: {reason}")]
    InvalidWorkerId { reason: String },

    #[error("worker not found: {worker_id}")]
    WorkerNotFound { worker_id: String },

    #[error("invalid working directory {path}: {reason}")]
    InvalidWorkingDir { path: String, reason: String },
}

impl From<HostExecError> for Status {
    fn from(err: HostExecError) -> Self {
        match &err {
            HostExecError::CommandNotAllowed { command } => {
                let mut meta = HashMap::new();
                meta.insert("command".into(), command.clone());
                error::status_with_info(
                    Code::PermissionDenied,
                    err.to_string(),
                    DOMAIN_HOSTEXEC,
                    COMMAND_NOT_ALLOWED,
                    meta,
                )
            }
            HostExecError::TransformRejected { .. } => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                TRANSFORM_REJECTED,
                HashMap::new(),
            ),
            HostExecError::BuilderdUnavailable => error::status_with_info(
                Code::Unavailable,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                BUILDERD_UNAVAILABLE,
                HashMap::new(),
            ),
            HostExecError::BuilderdExecFailed { .. } => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                INTERNAL,
                HashMap::new(),
            ),
            HostExecError::InvalidWorkerId { .. } => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                INVALID_ARGUMENT,
                HashMap::new(),
            ),
            HostExecError::WorkerNotFound { worker_id } => {
                let mut meta = HashMap::new();
                meta.insert("worker_id".into(), worker_id.clone());
                error::status_with_info(
                    Code::NotFound,
                    err.to_string(),
                    DOMAIN_HOSTEXEC,
                    NOT_FOUND,
                    meta,
                )
            }
            HostExecError::InvalidWorkingDir { path, .. } => {
                let mut meta = HashMap::new();
                meta.insert("path".into(), path.clone());
                error::status_with_info(
                    Code::InvalidArgument,
                    err.to_string(),
                    DOMAIN_HOSTEXEC,
                    INVALID_ARGUMENT,
                    meta,
                )
            }
        }
    }
}

type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[derive(Clone)]
pub struct HostExecServiceHandler {
    pub config: HostExecConfigManager,
    pub lua: LuaTransformManager,
    pub worker_manager: WorkerManager,
    pub projects: HashMap<String, ur_config::ProjectConfig>,
    pub builderd_addr: String,
    pub host_workspace: std::path::PathBuf,
}

#[tonic::async_trait]
impl HostExecService for HostExecServiceHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<HostExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        // Extract worker context from metadata (if present)
        let (process_id, worker_context, config) = self.resolve_request_context(&req).await?;

        let req = req.into_inner();

        // 1. Allowlist check
        let cmd_config = config.get(&req.command).ok_or_else(|| {
            warn!(
                command = req.command,
                process_id, "host exec command denied: not in allowlist"
            );
            HostExecError::CommandNotAllowed {
                command: req.command.clone(),
            }
        })?;

        // 2. CWD mapping: /workspace prefix -> %WORKSPACE% template for builderd resolution.
        // For pool workers, /workspace maps to a specific slot subdirectory, not the root.
        let slot_path = worker_context.as_ref().map(|ctx| ctx.slot_path.as_path());
        let host_working_dir = self.map_working_dir(&req.working_dir, slot_path)?;

        // 3. Lua transform (if configured)
        let transform_result = if let Some(lua_source) = &cmd_config.lua_source {
            self.lua
                .run_transform(
                    lua_source,
                    &req.command,
                    &req.args,
                    &host_working_dir,
                    worker_context.as_ref(),
                )
                .map_err(|e| HostExecError::TransformRejected {
                    reason: e.to_string(),
                })?
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
            .map_err(|_| HostExecError::BuilderdUnavailable)?;

        let builder_req = BuilderExecRequest {
            command: transform_result.command,
            args: transform_result.args,
            working_dir: transform_result.working_dir,
            env: transform_result.env,
        };

        let response =
            client
                .exec(builder_req)
                .await
                .map_err(|e| HostExecError::BuilderdExecFailed {
                    message: e.to_string(),
                })?;

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
        // Extract worker context to get per-project merged config
        let (process_id, _worker_context, config) = self.resolve_request_context(&req).await?;

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
    /// Extract worker context from request metadata and resolve per-request state.
    ///
    /// Returns `(process_id, worker_context, effective_config)` where:
    /// - `process_id` is resolved from the worker ID (or empty for host-server requests)
    /// - `worker_context` is the Lua-facing context (if the worker has a project/slot)
    /// - `effective_config` is the base hostexec config merged with per-project passthrough commands
    #[allow(clippy::result_large_err)]
    async fn resolve_request_context<T: Send + Sync>(
        &self,
        req: &Request<T>,
    ) -> Result<
        (
            String,
            Option<crate::hostexec::WorkerContext>,
            HostExecConfigManager,
        ),
        HostExecError,
    > {
        let Some(worker_id_val) = req.metadata().get(ur_config::WORKER_ID_HEADER) else {
            // No worker ID header — host-server request (e.g., from `ur` CLI).
            return Ok((String::new(), None, self.config.clone()));
        };

        let worker_id_str = worker_id_val
            .to_str()
            .map_err(|_| HostExecError::InvalidWorkerId {
                reason: "invalid ur-worker-id header encoding".into(),
            })?;
        let worker_id = crate::WorkerId::parse(worker_id_str)
            .map_err(|e| HostExecError::InvalidWorkerId { reason: e })?;

        // Look up process_id from worker_manager
        let process_id = self
            .worker_manager
            .resolve_process_id(&worker_id)
            .await
            .map_err(|e| HostExecError::WorkerNotFound { worker_id: e })?;

        // Look up worker context (project_key, slot_path) from worker_manager
        let proc_context = self.worker_manager.get_worker_context(&worker_id).await;

        // Build Lua-facing WorkerContext and merge per-project passthrough commands
        let (worker_context, config) = match proc_context {
            Some(ref ctx) if ctx.project_key.is_some() => {
                let project_key = ctx.project_key.as_ref().unwrap();
                let lua_ctx = crate::hostexec::WorkerContext {
                    worker_id: worker_id_str.to_owned(),
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
                // Worker has a slot but no project — raw workspace mount
                let lua_ctx = crate::hostexec::WorkerContext {
                    worker_id: worker_id_str.to_owned(),
                    project_key: String::new(),
                    slot_path: ctx.slot_path.clone(),
                };
                (Some(lua_ctx), self.config.clone())
            }
            None => (None, self.config.clone()),
        };

        Ok((process_id, worker_context, config))
    }

    /// Map container CWD to a `%WORKSPACE%` template path for builderd.
    ///
    /// For workspace mounts (`-w`), `/workspace/foo` maps to `%WORKSPACE%/foo`.
    /// For pool mounts (`-p`), `/workspace/foo` maps to `%WORKSPACE%/<slot_relative>/foo`
    /// where `slot_relative` is the slot's path relative to `host_workspace`.
    #[allow(clippy::result_large_err)]
    fn map_working_dir(
        &self,
        container_dir: &str,
        slot_path: Option<&std::path::Path>,
    ) -> Result<String, HostExecError> {
        map_working_dir_impl(container_dir, slot_path, &self.host_workspace)
    }
}

/// Map container CWD to a `%WORKSPACE%` template path for builderd (implementation).
///
/// Extracted as a free function for testability.
#[allow(clippy::result_large_err)]
fn map_working_dir_impl(
    container_dir: &str,
    slot_path: Option<&std::path::Path>,
    host_workspace: &std::path::Path,
) -> Result<String, HostExecError> {
    let Some(suffix) = container_dir.strip_prefix("/workspace") else {
        return Err(HostExecError::InvalidWorkingDir {
            path: container_dir.to_string(),
            reason: "working_dir must start with /workspace".into(),
        });
    };

    if !suffix.is_empty() && !suffix.starts_with('/') {
        return Err(HostExecError::InvalidWorkingDir {
            path: container_dir.to_string(),
            reason: "invalid working_dir".into(),
        });
    }

    // Compute the slot-relative prefix (empty for workspace mounts, e.g. "pool/proj/0" for pool).
    let slot_relative = slot_path
        .and_then(|sp| {
            let host_ws = host_workspace.to_string_lossy();
            let sp_str = sp.to_string_lossy();
            sp_str
                .strip_prefix(host_ws.as_ref())
                .map(|rel| rel.trim_start_matches('/').to_string())
        })
        .unwrap_or_default();

    let mut result = "%WORKSPACE%".to_string();
    if !slot_relative.is_empty() {
        result.push('/');
        result.push_str(&slot_relative);
    }
    if !suffix.is_empty() {
        result.push_str(suffix);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const HOST_WS: &str = "/home/user/.ur/workspace";

    #[test]
    fn test_map_working_dir_workspace_mount() {
        let result = map_working_dir_impl("/workspace", None, Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%");
    }

    #[test]
    fn test_map_working_dir_workspace_mount_subdir() {
        let result = map_working_dir_impl("/workspace/src/main", None, Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/src/main");
    }

    #[test]
    fn test_map_working_dir_pool_mount() {
        let slot = std::path::PathBuf::from("/home/user/.ur/workspace/pool/proj/0");
        let result = map_working_dir_impl("/workspace", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/proj/0");
    }

    #[test]
    fn test_map_working_dir_pool_mount_subdir() {
        let result = map_working_dir_impl(
            "/workspace/src/main",
            Some(&std::path::PathBuf::from(
                "/home/user/.ur/workspace/pool/proj/0",
            )),
            Path::new(HOST_WS),
        )
        .unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/proj/0/src/main");
    }

    #[test]
    fn test_map_working_dir_rejects_invalid() {
        assert!(map_working_dir_impl("/tmp", None, Path::new(HOST_WS)).is_err());
        assert!(map_working_dir_impl("/workspacefoo", None, Path::new(HOST_WS)).is_err());
    }
}
