use std::collections::HashMap;
use std::pin::Pin;

use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Response, Status, Streaming};
use tracing::{info, warn};

use ur_rpc::error::{
    self, BUILDERD_UNAVAILABLE, COMMAND_NOT_ALLOWED, DOMAIN_HOSTEXEC, INTERNAL, INVALID_ARGUMENT,
    NOT_FOUND, SCRIPT_NOT_ALLOWED, TRANSFORM_REJECTED,
};
use ur_rpc::proto::builder::BuilderdClient;
use ur_rpc::proto::builder::builder_exec_message::Payload as BuilderPayload;
use ur_rpc::proto::builder::{BuilderExecMessage, BuilderExecRequest};
use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::hostexec::host_exec_message::Payload as HostExecPayload;
use ur_rpc::proto::hostexec::host_exec_service_server::HostExecService;
use ur_rpc::proto::hostexec::{
    HostExecMessage, ListHostExecCommandsRequest, ListHostExecCommandsResponse,
};

use crate::WorkerManager;
use crate::hostexec::{HostExecConfigManager, LuaTransformManager, ScriptRegistry};

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

    #[error("missing start frame")]
    MissingStartFrame,

    #[error("script not allowed: {script} for project {project}")]
    ScriptNotAllowed { script: String, project: String },
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
            HostExecError::MissingStartFrame => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                INVALID_ARGUMENT,
                HashMap::new(),
            ),
            HostExecError::ScriptNotAllowed { script, project } => {
                let mut meta = HashMap::new();
                meta.insert("script".into(), script.clone());
                meta.insert("project".into(), project.clone());
                error::status_with_info(
                    Code::PermissionDenied,
                    err.to_string(),
                    DOMAIN_HOSTEXEC,
                    SCRIPT_NOT_ALLOWED,
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
    pub project_registry: crate::ProjectRegistry,
    pub script_registry: ScriptRegistry,
    pub lua: LuaTransformManager,
    pub worker_manager: WorkerManager,
    pub builderd_client: BuilderdClient,
    pub host_workspace: std::path::PathBuf,
    pub git_branch_prefix: String,
}

#[tonic::async_trait]
impl HostExecService for HostExecServiceHandler {
    type ExecStream = CommandOutputStream;

    async fn exec(
        &self,
        req: Request<Streaming<HostExecMessage>>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        // Extract worker context from metadata before consuming the request.
        // Streaming<T> is not Sync, so we extract metadata first.
        let metadata = req.metadata().clone();
        let (process_id, worker_context, config) = self
            .resolve_request_context_from_metadata(&metadata)
            .await?;

        let mut inbound = req.into_inner();

        // Read the first message — must be a start frame.
        let first_msg = inbound
            .next()
            .await
            .ok_or(HostExecError::MissingStartFrame)?
            .map_err(|e| HostExecError::BuilderdExecFailed {
                message: format!("stream error reading start frame: {e}"),
            })?;

        let host_exec_req = match first_msg.payload {
            Some(HostExecPayload::Start(start)) => start,
            _ => return Err(HostExecError::MissingStartFrame.into()),
        };

        let has_command = !host_exec_req.command.is_empty();
        let has_script = !host_exec_req.script_path.is_empty();

        match (has_command, has_script) {
            (true, true) | (false, false) => {
                return Err(Status::invalid_argument(
                    "exactly one of command or script_path must be set",
                ));
            }
            (true, false) => {
                self.exec_command(host_exec_req, worker_context, config, process_id, inbound)
                    .await
            }
            (false, true) => {
                self.exec_script(host_exec_req, worker_context, process_id, inbound)
                    .await
            }
        }
    }

    async fn list_commands(
        &self,
        req: Request<ListHostExecCommandsRequest>,
    ) -> Result<Response<ListHostExecCommandsResponse>, Status> {
        // Extract worker context to get per-project merged config
        let (process_id, _worker_context, config) = self.resolve_request_context(&req).await?;

        let commands = config.command_names();
        let entries = config.command_entries();
        info!(
            process_id,
            command_count = commands.len(),
            "list_commands request"
        );
        Ok(Response::new(ListHostExecCommandsResponse {
            commands,
            entries,
        }))
    }
}

/// Forward stdin frames from the client inbound stream to the builder channel.
async fn forward_stdin_to_builder(
    mut inbound: Streaming<HostExecMessage>,
    tx: mpsc::Sender<BuilderExecMessage>,
) {
    while let Some(Ok(msg)) = inbound.next().await {
        let Some(HostExecPayload::Stdin(data)) = msg.payload else {
            continue;
        };
        let builder_msg = BuilderExecMessage {
            payload: Some(BuilderPayload::Stdin(data)),
        };
        if tx.send(builder_msg).await.is_err() {
            break;
        }
    }
}

async fn forward_to_builderd(
    builderd_client: &BuilderdClient,
    transform_result: crate::hostexec::lua_transform::TransformResult,
    long_lived: bool,
    is_bidi: bool,
    inbound: Streaming<HostExecMessage>,
) -> Result<Response<CommandOutputStream>, Status> {
    let mut client = builderd_client.clone();

    let builder_req = BuilderExecRequest {
        command: transform_result.command,
        args: transform_result.args,
        working_dir: transform_result.working_dir,
        env: transform_result.env,
        long_lived,
    };

    let start_msg = BuilderExecMessage {
        payload: Some(BuilderPayload::Start(builder_req)),
    };

    let (builder_tx, builder_rx) = mpsc::channel::<BuilderExecMessage>(32);
    builder_tx
        .send(start_msg)
        .await
        .map_err(|_| HostExecError::BuilderdExecFailed {
            message: "failed to enqueue start frame".into(),
        })?;

    if is_bidi {
        let tx = builder_tx.clone();
        tokio::spawn(forward_stdin_to_builder(inbound, tx));
    }

    drop(builder_tx);

    let builder_stream = ReceiverStream::new(builder_rx);

    let response =
        client
            .exec(builder_stream)
            .await
            .map_err(|e| HostExecError::BuilderdExecFailed {
                message: e.to_string(),
            })?;

    let mut builderd_inbound = response.into_inner();
    let (tx, rx) = mpsc::channel(32);

    tokio::spawn(async move {
        while let Ok(Some(msg)) = builderd_inbound.message().await {
            if tx.send(Ok(msg)).await.is_err() {
                break;
            }
        }
    });

    let stream = ReceiverStream::new(rx);
    Ok(Response::new(Box::pin(stream) as CommandOutputStream))
}

impl HostExecServiceHandler {
    /// Handle the command branch of exec: allowlist check, CWD mapping, Lua transform.
    async fn exec_command(
        &self,
        host_exec_req: ur_rpc::proto::hostexec::HostExecRequest,
        worker_context: Option<crate::hostexec::WorkerContext>,
        config: HostExecConfigManager,
        process_id: String,
        inbound: Streaming<HostExecMessage>,
    ) -> Result<Response<CommandOutputStream>, Status> {
        // 1. Allowlist check
        let cmd_config = config.get(&host_exec_req.command).ok_or_else(|| {
            warn!(
                command = host_exec_req.command,
                process_id, "host exec command denied: not in allowlist"
            );
            HostExecError::CommandNotAllowed {
                command: host_exec_req.command.clone(),
            }
        })?;

        // 2. CWD mapping
        let slot_path = worker_context.as_ref().map(|ctx| ctx.slot_path.as_path());
        let host_working_dir = self.map_working_dir(&host_exec_req.working_dir, slot_path)?;

        // 3. Lua transform (if configured)
        let transform_result = if let Some(lua_source) = &cmd_config.lua_source {
            self.lua
                .run_transform(
                    lua_source,
                    &host_exec_req.command,
                    &host_exec_req.args,
                    &host_working_dir,
                    worker_context.as_ref(),
                )
                .map_err(|e| HostExecError::TransformRejected {
                    reason: e.to_string(),
                })?
        } else {
            crate::hostexec::lua_transform::TransformResult {
                command: host_exec_req.command.clone(),
                args: host_exec_req.args,
                working_dir: host_working_dir,
                env: std::collections::HashMap::new(),
            }
        };

        let is_bidi = cmd_config.bidi;

        info!(
            command = transform_result.command,
            process_id,
            working_dir = transform_result.working_dir,
            args_count = transform_result.args.len(),
            bidi = is_bidi,
            long_lived = cmd_config.long_lived,
            "host exec forwarding to builderd"
        );

        forward_to_builderd(
            &self.builderd_client,
            transform_result,
            cmd_config.long_lived,
            is_bidi,
            inbound,
        )
        .await
    }

    /// Handle the script branch of exec: registry check, path validation, CWD mapping.
    async fn exec_script(
        &self,
        host_exec_req: ur_rpc::proto::hostexec::HostExecRequest,
        worker_context: Option<crate::hostexec::WorkerContext>,
        process_id: String,
        inbound: Streaming<HostExecMessage>,
    ) -> Result<Response<CommandOutputStream>, Status> {
        // 1. Worker must have a project_key
        let project_key = worker_context
            .as_ref()
            .map(|ctx| ctx.project_key.as_str())
            .filter(|k| !k.is_empty())
            .ok_or_else(|| HostExecError::ScriptNotAllowed {
                script: host_exec_req.script_path.clone(),
                project: String::new(),
            })?
            .to_owned();

        // 2. Strip /workspace/ prefix; reject if missing or path contains ..
        let rel_path =
            validate_script_path(&host_exec_req.script_path).map_err(Status::invalid_argument)?;

        // 3. Registry allow check
        if !self.script_registry.allows(&project_key, rel_path) {
            warn!(
                script = host_exec_req.script_path,
                project = project_key,
                process_id,
                "host exec script denied: not in registry"
            );
            return Err(HostExecError::ScriptNotAllowed {
                script: host_exec_req.script_path.clone(),
                project: project_key,
            }
            .into());
        }

        // 4. Map script path to %WORKSPACE% template
        let slot_path = worker_context.as_ref().map(|ctx| ctx.slot_path.as_path());
        let script_template =
            map_script_path_impl(&host_exec_req.script_path, slot_path, &self.host_workspace)?;

        // 5. CWD mapping
        let host_working_dir = self.map_working_dir(&host_exec_req.working_dir, slot_path)?;

        let transform_result = crate::hostexec::lua_transform::TransformResult {
            command: script_template,
            args: host_exec_req.args,
            working_dir: host_working_dir,
            env: std::collections::HashMap::new(),
        };

        info!(
            script = host_exec_req.script_path,
            process_id,
            working_dir = transform_result.working_dir,
            args_count = transform_result.args.len(),
            "host exec script forwarding to builderd"
        );

        forward_to_builderd(
            &self.builderd_client,
            transform_result,
            false,
            false,
            inbound,
        )
        .await
    }

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
        self.resolve_request_context_from_metadata(req.metadata())
            .await
    }

    /// Resolve request context from pre-extracted metadata.
    ///
    /// This variant is needed when the request body type is not `Sync`
    /// (e.g., `Streaming<T>`), so metadata must be cloned before consumption.
    #[allow(clippy::result_large_err)]
    async fn resolve_request_context_from_metadata(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<
        (
            String,
            Option<crate::hostexec::WorkerContext>,
            HostExecConfigManager,
        ),
        HostExecError,
    > {
        let Some(worker_id_val) = metadata.get(ur_config::WORKER_ID_HEADER) else {
            // No worker ID header — host-server request (e.g., from `ur` CLI).
            return Ok((String::new(), None, self.project_registry.hostexec_config()));
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
                let branch = format!("{}{}", self.git_branch_prefix, worker_id_str);
                let lua_ctx = crate::hostexec::WorkerContext {
                    worker_id: worker_id_str.to_owned(),
                    process_id: process_id.clone(),
                    project_key: project_key.clone(),
                    slot_path: ctx.slot_path.clone(),
                    branch,
                };

                // Grant only defaults + project-granted commands
                let config = self.project_registry.hostexec_config();
                let extra = self
                    .project_registry
                    .get(project_key)
                    .map(|p| p.hostexec.clone())
                    .unwrap_or_default();
                let merged_config = config.with_project_commands(&extra);

                (Some(lua_ctx), merged_config)
            }
            Some(ref ctx) => {
                // Worker has a slot but no project — raw workspace mount, defaults only
                let lua_ctx = crate::hostexec::WorkerContext {
                    worker_id: worker_id_str.to_owned(),
                    process_id: process_id.clone(),
                    project_key: String::new(),
                    slot_path: ctx.slot_path.clone(),
                    branch: String::new(),
                };
                (
                    Some(lua_ctx),
                    self.project_registry.hostexec_config().defaults_only(),
                )
            }
            None => (
                None,
                self.project_registry.hostexec_config().defaults_only(),
            ),
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

/// Validate a container-relative script path and return the portion after `/workspace/`.
///
/// Returns `Err` with a human-readable message if the path does not start with
/// `/workspace/`, is empty after stripping the prefix, or contains `..` segments.
fn validate_script_path(script_path: &str) -> Result<&str, String> {
    let rel = script_path
        .strip_prefix("/workspace/")
        .ok_or_else(|| format!("script_path must start with /workspace/: {script_path}"))?;

    if rel.is_empty() {
        return Err(format!(
            "script_path must not be /workspace/ alone: {script_path}"
        ));
    }

    // Reject path traversal
    for segment in rel.split('/') {
        if segment == ".." || segment.is_empty() {
            return Err(format!(
                "script_path contains invalid segment: {script_path}"
            ));
        }
    }

    Ok(rel)
}

/// Map a container-absolute script path (`/workspace/<rel>`) to a `%WORKSPACE%` template.
///
/// Delegates to `map_working_dir_impl` — the path prefix logic is identical.
/// Pool mode produces `%WORKSPACE%/<slot_rel>/<rel>`; workspace mode produces `%WORKSPACE%/<rel>`.
#[allow(clippy::result_large_err)]
fn map_script_path_impl(
    script_path: &str,
    slot_path: Option<&std::path::Path>,
    host_workspace: &std::path::Path,
) -> Result<String, HostExecError> {
    map_working_dir_impl(script_path, slot_path, host_workspace)
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

    // For pool mounts, slot_path is under host_workspace — use %WORKSPACE%/<relative> template.
    // For workspace mounts (-w), slot_path is an arbitrary host path not under host_workspace —
    // use the absolute slot_path directly (builderd passes through absolute paths unchanged).
    if let Some(sp) = slot_path {
        let host_ws = host_workspace.to_string_lossy();
        let sp_str = sp.to_string_lossy();
        if let Some(rel) = sp_str.strip_prefix(host_ws.as_ref()) {
            // Pool mount: slot is under host_workspace
            let slot_relative = rel.trim_start_matches('/');
            let mut result = "%WORKSPACE%".to_string();
            if !slot_relative.is_empty() {
                result.push('/');
                result.push_str(slot_relative);
            }
            if !suffix.is_empty() {
                result.push_str(suffix);
            }
            Ok(result)
        } else {
            // Workspace mount: slot is an absolute path outside host_workspace
            let mut result = sp_str.into_owned();
            if !suffix.is_empty() {
                result.push_str(suffix);
            }
            Ok(result)
        }
    } else {
        // No slot_path: direct workspace mount
        let mut result = "%WORKSPACE%".to_string();
        if !suffix.is_empty() {
            result.push_str(suffix);
        }
        Ok(result)
    }
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
    fn test_map_working_dir_workspace_mount_with_slot_path() {
        // -w mount: slot_path is an arbitrary host dir, not under host_workspace
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        let result = map_working_dir_impl("/workspace", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/Users/foo/myproject");
    }

    #[test]
    fn test_map_working_dir_workspace_mount_with_slot_path_subdir() {
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        let result =
            map_working_dir_impl("/workspace/src", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/Users/foo/myproject/src");
    }

    #[test]
    fn test_map_working_dir_rejects_invalid() {
        assert!(map_working_dir_impl("/tmp", None, Path::new(HOST_WS)).is_err());
        assert!(map_working_dir_impl("/workspacefoo", None, Path::new(HOST_WS)).is_err());
    }

    #[test]
    fn test_map_working_dir_rejects_invalid_with_pool_slot() {
        let slot = std::path::PathBuf::from("/home/user/.ur/workspace/pool/proj/0");
        assert!(map_working_dir_impl("/tmp", Some(&slot), Path::new(HOST_WS)).is_err());
        assert!(map_working_dir_impl("/workspacefoo", Some(&slot), Path::new(HOST_WS)).is_err());
    }

    #[test]
    fn test_map_working_dir_rejects_invalid_with_workspace_slot() {
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        assert!(map_working_dir_impl("/other", Some(&slot), Path::new(HOST_WS)).is_err());
    }

    #[test]
    fn test_map_working_dir_pool_slot_is_host_workspace_root() {
        // Slot path is exactly the host_workspace directory (no relative suffix)
        let slot = std::path::PathBuf::from(HOST_WS);
        let result = map_working_dir_impl("/workspace", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%");
    }

    #[test]
    fn test_map_working_dir_pool_slot_is_host_workspace_root_subdir() {
        let slot = std::path::PathBuf::from(HOST_WS);
        let result =
            map_working_dir_impl("/workspace/src", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/src");
    }

    #[test]
    fn test_map_working_dir_pool_mount_deeply_nested_subdir() {
        let slot = std::path::PathBuf::from("/home/user/.ur/workspace/pool/proj/0");
        let result = map_working_dir_impl(
            "/workspace/src/lib/module/file",
            Some(&slot),
            Path::new(HOST_WS),
        )
        .unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/proj/0/src/lib/module/file");
    }

    #[test]
    fn test_map_working_dir_workspace_mount_deeply_nested_subdir() {
        let result = map_working_dir_impl("/workspace/a/b/c/d", None, Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/a/b/c/d");
    }

    #[test]
    fn test_map_working_dir_workspace_slot_deeply_nested_subdir() {
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        let result =
            map_working_dir_impl("/workspace/a/b/c/d", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/Users/foo/myproject/a/b/c/d");
    }

    #[test]
    fn test_map_working_dir_error_message_contains_path() {
        let err = map_working_dir_impl("/tmp/foo", None, Path::new(HOST_WS)).unwrap_err();
        match err {
            HostExecError::InvalidWorkingDir { path, .. } => {
                assert_eq!(path, "/tmp/foo");
            }
            _ => panic!("expected InvalidWorkingDir"),
        }
    }

    #[test]
    fn test_map_working_dir_workspacex_rejected() {
        // Paths like /workspaceXYZ should be rejected (no slash separator)
        let err = map_working_dir_impl("/workspacedata", None, Path::new(HOST_WS)).unwrap_err();
        match err {
            HostExecError::InvalidWorkingDir { path, reason } => {
                assert_eq!(path, "/workspacedata");
                assert_eq!(reason, "invalid working_dir");
            }
            _ => panic!("expected InvalidWorkingDir"),
        }
    }

    #[test]
    fn test_map_working_dir_different_host_workspace() {
        // Verify behavior with a non-default host_workspace path
        let custom_host_ws = "/opt/custom/workspace";
        let slot = std::path::PathBuf::from("/opt/custom/workspace/pool/myproj/1");
        let result =
            map_working_dir_impl("/workspace/src", Some(&slot), Path::new(custom_host_ws)).unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/myproj/1/src");
    }

    #[test]
    fn test_map_working_dir_slot_completely_different_prefix() {
        // Slot path with a completely different prefix from host_workspace
        let slot = std::path::PathBuf::from("/opt/other/projects/myapp");
        let result = map_working_dir_impl("/workspace", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/opt/other/projects/myapp");
    }

    #[test]
    fn test_map_working_dir_slot_completely_different_prefix_subdir() {
        let slot = std::path::PathBuf::from("/opt/other/projects/myapp");
        let result =
            map_working_dir_impl("/workspace/src", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/opt/other/projects/myapp/src");
    }

    // --- validate_script_path tests ---

    #[test]
    fn validate_script_path_valid() {
        assert_eq!(
            validate_script_path("/workspace/scripts/deploy.sh").unwrap(),
            "scripts/deploy.sh"
        );
    }

    #[test]
    fn validate_script_path_nested() {
        assert_eq!(
            validate_script_path("/workspace/a/b/c.sh").unwrap(),
            "a/b/c.sh"
        );
    }

    #[test]
    fn validate_script_path_rejects_no_workspace_prefix() {
        assert!(validate_script_path("/tmp/script.sh").is_err());
        assert!(validate_script_path("scripts/deploy.sh").is_err());
    }

    #[test]
    fn validate_script_path_rejects_workspace_root_only() {
        // /workspace/ alone (empty rel_path after stripping)
        assert!(validate_script_path("/workspace/").is_err());
    }

    #[test]
    fn validate_script_path_rejects_dotdot() {
        assert!(validate_script_path("/workspace/../etc/passwd").is_err());
        assert!(validate_script_path("/workspace/scripts/../other.sh").is_err());
    }

    #[test]
    fn validate_script_path_rejects_double_slash() {
        // Double slash produces an empty segment
        assert!(validate_script_path("/workspace/scripts//deploy.sh").is_err());
    }

    // --- map_script_path_impl tests ---

    #[test]
    fn map_script_path_workspace_mount() {
        let result =
            map_script_path_impl("/workspace/scripts/run.sh", None, Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/scripts/run.sh");
    }

    #[test]
    fn map_script_path_pool_mount() {
        let slot = std::path::PathBuf::from("/home/user/.ur/workspace/pool/proj/0");
        let result =
            map_script_path_impl("/workspace/scripts/run.sh", Some(&slot), Path::new(HOST_WS))
                .unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/proj/0/scripts/run.sh");
    }

    #[test]
    fn map_script_path_workspace_slot() {
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        let result =
            map_script_path_impl("/workspace/scripts/run.sh", Some(&slot), Path::new(HOST_WS))
                .unwrap();
        assert_eq!(result, "/Users/foo/myproject/scripts/run.sh");
    }

    // --- ScriptNotAllowed error variant status mapping ---

    #[test]
    fn script_not_allowed_maps_to_permission_denied() {
        let err = HostExecError::ScriptNotAllowed {
            script: "scripts/deploy.sh".into(),
            project: "myproject".into(),
        };
        let status: Status = err.into();
        assert_eq!(status.code(), Code::PermissionDenied);
    }

    #[test]
    fn script_not_allowed_status_has_error_info() {
        use tonic_types::StatusExt;

        let err = HostExecError::ScriptNotAllowed {
            script: "scripts/deploy.sh".into(),
            project: "myproject".into(),
        };
        let status: Status = err.into();
        let info = status
            .get_details_error_info()
            .expect("should have ErrorInfo");
        assert_eq!(info.reason, SCRIPT_NOT_ALLOWED);
        assert_eq!(info.domain, DOMAIN_HOSTEXEC);
        assert_eq!(
            info.metadata.get("script").map(|s| s.as_str()),
            Some("scripts/deploy.sh")
        );
        assert_eq!(
            info.metadata.get("project").map(|s| s.as_str()),
            Some("myproject")
        );
    }
}
