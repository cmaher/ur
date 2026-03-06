use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;
use tracing::info;

use ur_rpc::*;

use crate::{ProcessManager, RepoRegistry};

// ---------------------------------------------------------------------------
// AgentBridge — serves per-agent sockets (exec_git, ping, etc.)
// ---------------------------------------------------------------------------

/// Lightweight server for per-agent sockets. Handles agent-facing RPCs
/// (git, tickets, etc.) but rejects process management calls. Using a
/// separate type from `BridgeServer` breaks the recursive opaque-type chain
/// that tarpc's `#[service]` macro cannot verify for Send.
#[derive(Clone)]
pub struct AgentBridge {
    pub repo_registry: Arc<RepoRegistry>,
    pub socket_dir: PathBuf,
    pub process_id: String,
}

impl UrAgentBridge for AgentBridge {
    async fn ping(self, _ctx: tarpc::context::Context) -> String {
        "pong".into()
    }

    async fn ask_human(
        self,
        _ctx: tarpc::context::Context,
        _req: AskHumanRequest,
    ) -> Result<AskHumanResponse, String> {
        Err("ask_human not yet implemented".into())
    }

    async fn exec_git(
        self,
        _ctx: tarpc::context::Context,
        req: ExecGitRequest,
    ) -> Result<GitResponse, String> {
        self.repo_registry
            .exec_git(&self.process_id, &req.args)
            .await
    }

    async fn exec_git_stream(
        self,
        _ctx: tarpc::context::Context,
        req: ExecGitRequest,
    ) -> Result<StreamingExecResponse, String> {
        self.repo_registry
            .exec_git_stream(&self.socket_dir, &self.process_id, &req.args)
            .await
    }

    async fn report_status(
        self,
        _ctx: tarpc::context::Context,
        _req: ReportStatusRequest,
    ) -> Result<(), String> {
        Err("report_status not yet implemented".into())
    }

    async fn ticket_read(
        self,
        _ctx: tarpc::context::Context,
        _req: TicketReadRequest,
    ) -> Result<TicketReadResponse, String> {
        Err("ticket_read not yet implemented".into())
    }

    async fn ticket_spawn(
        self,
        _ctx: tarpc::context::Context,
        _req: TicketSpawnRequest,
    ) -> Result<TicketSpawnResponse, String> {
        Err("ticket_spawn not yet implemented".into())
    }

    async fn ticket_note(
        self,
        _ctx: tarpc::context::Context,
        _req: TicketNoteRequest,
    ) -> Result<(), String> {
        Err("ticket_note not yet implemented".into())
    }

    async fn container_build(
        self,
        _ctx: tarpc::context::Context,
        _req: ContainerBuildRequest,
    ) -> Result<ContainerBuildResponse, String> {
        Err("container_build not available on agent socket".into())
    }

    async fn container_run(
        self,
        _ctx: tarpc::context::Context,
        _req: ContainerRunRequest,
    ) -> Result<ContainerRunResponse, String> {
        Err("container_run not available on agent socket".into())
    }

    async fn container_stop(
        self,
        _ctx: tarpc::context::Context,
        _req: ContainerIdRequest,
    ) -> Result<(), String> {
        Err("container_stop not available on agent socket".into())
    }

    async fn container_rm(
        self,
        _ctx: tarpc::context::Context,
        _req: ContainerIdRequest,
    ) -> Result<(), String> {
        Err("container_rm not available on agent socket".into())
    }

    async fn container_exec(
        self,
        _ctx: tarpc::context::Context,
        _req: ContainerExecRequest,
    ) -> Result<ContainerExecResponse, String> {
        Err("container_exec not available on agent socket".into())
    }

    async fn process_launch(
        self,
        _ctx: tarpc::context::Context,
        _req: ProcessLaunchRequest,
    ) -> Result<ProcessLaunchResponse, String> {
        Err("process_launch not available on agent socket".into())
    }

    async fn process_stop(
        self,
        _ctx: tarpc::context::Context,
        _req: ProcessStopRequest,
    ) -> Result<(), String> {
        Err("process_stop not available on agent socket".into())
    }
}

/// Accept loop for per-agent sockets using `AgentBridge`.
pub async fn agent_accept_loop(socket_path: PathBuf, server: AgentBridge) -> anyhow::Result<()> {
    let _ = tokio::fs::remove_file(&socket_path).await;

    let mut listener = tarpc::serde_transport::unix::listen(&socket_path, Bincode::default).await?;
    info!("agent listening on {}", socket_path.display());

    while let Some(transport) = listener.next().await {
        let transport = transport?;
        let channel = server::BaseChannel::with_defaults(transport);
        let srv = server.clone();
        tokio::spawn(channel.execute(srv.serve()).for_each(|response| async {
            tokio::spawn(response);
        }));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// BridgeServer — serves the main control socket (ur CLI)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct BridgeServer {
    pub repo_registry: Arc<RepoRegistry>,
    pub socket_dir: PathBuf,
    pub process_id: String,
    pub process_manager: ProcessManager,
}

impl UrAgentBridge for BridgeServer {
    async fn ping(self, _ctx: tarpc::context::Context) -> String {
        "pong".into()
    }

    async fn ask_human(
        self,
        _ctx: tarpc::context::Context,
        _req: AskHumanRequest,
    ) -> Result<AskHumanResponse, String> {
        Err("ask_human not yet implemented".into())
    }

    async fn exec_git(
        self,
        _ctx: tarpc::context::Context,
        req: ExecGitRequest,
    ) -> Result<GitResponse, String> {
        self.repo_registry
            .exec_git(&self.process_id, &req.args)
            .await
    }

    async fn exec_git_stream(
        self,
        _ctx: tarpc::context::Context,
        req: ExecGitRequest,
    ) -> Result<StreamingExecResponse, String> {
        self.repo_registry
            .exec_git_stream(&self.socket_dir, &self.process_id, &req.args)
            .await
    }

    async fn report_status(
        self,
        _ctx: tarpc::context::Context,
        _req: ReportStatusRequest,
    ) -> Result<(), String> {
        Err("report_status not yet implemented".into())
    }

    async fn ticket_read(
        self,
        _ctx: tarpc::context::Context,
        _req: TicketReadRequest,
    ) -> Result<TicketReadResponse, String> {
        Err("ticket_read not yet implemented".into())
    }

    async fn ticket_spawn(
        self,
        _ctx: tarpc::context::Context,
        _req: TicketSpawnRequest,
    ) -> Result<TicketSpawnResponse, String> {
        Err("ticket_spawn not yet implemented".into())
    }

    async fn ticket_note(
        self,
        _ctx: tarpc::context::Context,
        _req: TicketNoteRequest,
    ) -> Result<(), String> {
        Err("ticket_note not yet implemented".into())
    }

    async fn container_build(
        self,
        _ctx: tarpc::context::Context,
        req: ContainerBuildRequest,
    ) -> Result<ContainerBuildResponse, String> {
        let rt = container::runtime_from_env();
        let opts = container::BuildOpts {
            tag: req.tag,
            dockerfile: PathBuf::from(req.dockerfile),
            context: PathBuf::from(req.context),
        };
        let image = rt.build(&opts).map_err(|e| e.to_string())?;
        Ok(ContainerBuildResponse { image_id: image.0 })
    }

    async fn container_run(
        self,
        _ctx: tarpc::context::Context,
        req: ContainerRunRequest,
    ) -> Result<ContainerRunResponse, String> {
        let rt = container::runtime_from_env();
        let opts = container::RunOpts {
            image: container::ImageId(req.image_id),
            name: req.name,
            cpus: req.cpus,
            memory: req.memory,
            volumes: req
                .volumes
                .into_iter()
                .map(|(h, g)| (PathBuf::from(h), PathBuf::from(g)))
                .collect(),
            socket_mounts: req
                .socket_mounts
                .into_iter()
                .map(|(h, g)| (PathBuf::from(h), PathBuf::from(g)))
                .collect(),
            workdir: req.workdir.map(PathBuf::from),
            command: req.command,
        };
        let id = rt.run(&opts).map_err(|e| e.to_string())?;
        Ok(ContainerRunResponse { container_id: id.0 })
    }

    async fn container_stop(
        self,
        _ctx: tarpc::context::Context,
        req: ContainerIdRequest,
    ) -> Result<(), String> {
        let rt = container::runtime_from_env();
        rt.stop(&container::ContainerId(req.container_id))
            .map_err(|e| e.to_string())
    }

    async fn container_rm(
        self,
        _ctx: tarpc::context::Context,
        req: ContainerIdRequest,
    ) -> Result<(), String> {
        let rt = container::runtime_from_env();
        rt.rm(&container::ContainerId(req.container_id))
            .map_err(|e| e.to_string())
    }

    async fn container_exec(
        self,
        _ctx: tarpc::context::Context,
        req: ContainerExecRequest,
    ) -> Result<ContainerExecResponse, String> {
        let rt = container::runtime_from_env();
        let opts = container::ExecOpts {
            command: req.command,
            workdir: req.workdir.map(PathBuf::from),
        };
        let output = rt
            .exec(&container::ContainerId(req.container_id), &opts)
            .map_err(|e| e.to_string())?;
        Ok(ContainerExecResponse {
            exit_code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    async fn process_launch(
        self,
        _ctx: tarpc::context::Context,
        req: ProcessLaunchRequest,
    ) -> Result<ProcessLaunchResponse, String> {
        // Phase 1: create repo, git init, register
        let socket_path = self.process_manager.prepare(&req.process_id).await?;

        // Spawn per-agent accept_loop using AgentBridge (not BridgeServer,
        // which would create a recursive opaque-type chain).
        let agent = AgentBridge {
            repo_registry: self.repo_registry.clone(),
            socket_dir: self.socket_dir.clone(),
            process_id: req.process_id.clone(),
        };
        let sp = socket_path.clone();
        let accept_handle = tokio::spawn(async move {
            if let Err(e) = agent_accept_loop(sp, agent).await {
                tracing::warn!("per-agent accept_loop error: {e}");
            }
        });

        // Phase 2: wait for socket, run container, record
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
            .await?;

        Ok(ProcessLaunchResponse { container_id })
    }

    async fn process_stop(
        self,
        _ctx: tarpc::context::Context,
        req: ProcessStopRequest,
    ) -> Result<(), String> {
        self.process_manager.stop(&req.process_id).await
    }
}
