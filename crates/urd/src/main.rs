use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use futures::StreamExt;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;
use tracing::info;
use ur_rpc::UrAgentBridge;

mod bridge;
pub use bridge::{AgentBridge, BridgeServer};

mod config;
pub use config::Config;

mod git_exec;
pub use git_exec::RepoRegistry;

mod process;
pub use process::ProcessManager;

#[derive(Parser)]
#[command(
    name = "urd",
    about = "Ur daemon — coordination server for containerized agents"
)]
struct Cli {}

pub async fn accept_loop(socket_path: PathBuf, server: BridgeServer) -> anyhow::Result<()> {
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

    let _cli = Cli::parse();

    let cfg = Config::load()?;
    info!("config dir: {}", cfg.config_dir.display());
    info!("workspace:  {}", cfg.workspace.display());
    tokio::fs::create_dir_all(&cfg.workspace).await?;
    tokio::fs::create_dir_all(&cfg.config_dir).await?;

    let socket_path = cfg.socket_path();
    info!("socket:     {}", socket_path.display());

    let repo_registry = Arc::new(RepoRegistry::new(cfg.workspace.clone()));

    let process_manager =
        ProcessManager::new(cfg.config_dir.clone(), cfg.workspace, repo_registry.clone());

    let server = BridgeServer {
        repo_registry,
        socket_dir: cfg.config_dir.clone(),
        process_id: String::new(),
        process_manager,
    };

    accept_loop(socket_path, server).await
}
