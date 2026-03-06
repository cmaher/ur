use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use futures::StreamExt;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;
use tracing::info;

use ur_rpc::*;

mod config;
pub use config::Config;

mod git_exec;
pub use git_exec::RepoRegistry;

#[derive(Parser)]
#[command(
    name = "urd",
    about = "Ur daemon — coordination server for containerized agents"
)]
struct Cli {
    /// Socket directory for agent UDS connections
    #[arg(long, default_value = "/tmp/ur/sockets")]
    socket_dir: PathBuf,
}

#[derive(Clone)]
struct BridgeServer {
    repo_registry: Arc<RepoRegistry>,
    socket_dir: PathBuf,
    /// Identity of the agent this server instance serves, determined by which
    /// per-agent socket accepted the connection. Passed server-side to
    /// `RepoRegistry` so the request payload never carries a process_id.
    process_id: String,
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
}

async fn accept_loop(socket_path: PathBuf, server: BridgeServer) -> anyhow::Result<()> {
    // Remove stale socket if it exists
    let _ = tokio::fs::remove_file(&socket_path).await;

    let mut listener = tarpc::serde_transport::unix::listen(&socket_path, Bincode::default).await?;
    info!("urd listening on {}", socket_path.display());

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cfg = Config::load()?;
    info!("config dir: {}", cfg.config_dir.display());
    info!("workspace:  {}", cfg.workspace.display());
    tokio::fs::create_dir_all(&cfg.workspace).await?;

    let repo_registry = Arc::new(RepoRegistry::new(cfg.workspace));

    let cli = Cli::parse();
    tokio::fs::create_dir_all(&cli.socket_dir).await?;

    let server = BridgeServer {
        repo_registry,
        socket_dir: cli.socket_dir.clone(),
        process_id: String::new(),
    };

    let socket_path = cli.socket_dir.join("ur.sock");
    accept_loop(socket_path, server).await
}
