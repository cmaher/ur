use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};
use tracing::{info, warn};

use ur_rpc::proto::builder::BuilderdClient;
use ur_rpc::proto::hostexec::host_exec_message::Payload as HostExecPayload;
use ur_rpc::proto::hostexec::host_exec_service_server::HostExecService;
use ur_rpc::proto::hostexec::{
    HostExecMessage, ListHostExecCommandsRequest, ListHostExecCommandsResponse,
};

use crate::{WorkerManager, WorkerStrategy};
use hostexec::{
    BRAIN_HOSTEXEC_COMMAND, CommandOutputStream, HostExecConfigManager, HostExecError,
    LuaTransformManager, ScriptRegistry, forward_to_builderd, is_brain_dir,
    map_brain_working_dir_impl, map_script_path_impl, map_working_dir_impl, validate_script_path,
};

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

impl HostExecServiceHandler {
    /// Handle the command branch of exec: allowlist check, CWD mapping, Lua transform.
    async fn exec_command(
        &self,
        host_exec_req: ur_rpc::proto::hostexec::HostExecRequest,
        worker_context: Option<hostexec::WorkerContext>,
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

        // 2. CWD mapping. `/brain` maps to the worker's host brain dir, for git only.
        let host_working_dir = if is_brain_dir(&host_exec_req.working_dir) {
            if host_exec_req.command != BRAIN_HOSTEXEC_COMMAND {
                return Err(HostExecError::InvalidWorkingDir {
                    path: host_exec_req.working_dir.clone(),
                    reason: format!("only {BRAIN_HOSTEXEC_COMMAND} may run in /brain"),
                }
                .into());
            }
            let brain = self
                .brain_host_path(worker_context.as_ref())
                .await
                .ok_or_else(|| HostExecError::InvalidWorkingDir {
                    path: host_exec_req.working_dir.clone(),
                    reason: "worker has no brain mount".into(),
                })?;
            map_brain_working_dir_impl(&host_exec_req.working_dir, &brain)?
        } else {
            let slot_path = worker_context.as_ref().map(|ctx| ctx.slot_path.as_path());
            self.map_working_dir(&host_exec_req.working_dir, slot_path)?
        };

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
            hostexec::lua_transform::TransformResult {
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
        worker_context: Option<hostexec::WorkerContext>,
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

        let transform_result = hostexec::lua_transform::TransformResult {
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
            Option<hostexec::WorkerContext>,
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
            Option<hostexec::WorkerContext>,
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
                let strategy = WorkerStrategy::from_name(&ctx.strategy).ok();
                let branch = if strategy == Some(WorkerStrategy::Manual) {
                    String::new()
                } else {
                    format!("{}{}", self.git_branch_prefix, worker_id_str)
                };
                // Grant only defaults + project-granted commands minus project-denied commands
                let config = self.project_registry.hostexec_config();
                let (extra, denied, allow_no_verify, forge) = self
                    .project_registry
                    .get(project_key)
                    .map(|p| {
                        (
                            p.hostexec.clone(),
                            p.hostexec_deny.clone(),
                            p.git.allow_no_verify,
                            p.resolve_forge(),
                        )
                    })
                    .unwrap_or_default();
                let merged_config = config.with_project_commands(&extra, &denied, forge);

                let lua_ctx = hostexec::WorkerContext {
                    worker_id: worker_id_str.to_owned(),
                    process_id: process_id.clone(),
                    project_key: project_key.clone(),
                    slot_path: ctx.slot_path.clone(),
                    branch,
                    allow_no_verify,
                };

                (Some(lua_ctx), merged_config)
            }
            Some(ref ctx) => {
                // Worker has a slot but no project — raw workspace mount, defaults only
                let lua_ctx = hostexec::WorkerContext {
                    worker_id: worker_id_str.to_owned(),
                    process_id: process_id.clone(),
                    project_key: String::new(),
                    slot_path: ctx.slot_path.clone(),
                    branch: String::new(),
                    allow_no_verify: false,
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

    /// Host path of the brain mounted at `/brain` for the requesting worker, if any.
    async fn brain_host_path(
        &self,
        worker_context: Option<&hostexec::WorkerContext>,
    ) -> Option<std::path::PathBuf> {
        let worker_id = crate::WorkerId::parse(&worker_context?.worker_id).ok()?;
        self.worker_manager
            .get_worker_context(&worker_id)
            .await?
            .brain_path
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
