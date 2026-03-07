use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use futures::StreamExt;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;
use tracing::info;
use ur_rpc::UrAgentBridge;

use urd::bridge::BridgeServer;
use urd::{Config, ProcessManager, RepoRegistry};

#[derive(Parser)]
#[command(
    name = "urd",
    about = "Ur daemon — coordination server for containerized agents"
)]
struct Cli {}

async fn accept_loop(socket_path: PathBuf, server: BridgeServer) -> anyhow::Result<()> {
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
        ProcessManager::new(cfg.config_dir.clone(), cfg.workspace.clone(), repo_registry.clone());

    let server = BridgeServer {
        repo_registry: repo_registry.clone(),
        socket_dir: cfg.config_dir.clone(),
        process_id: String::new(),
        process_manager: process_manager.clone(),
    };

    let grpc_handler = urd::grpc::CoreServiceHandler {
        process_manager,
        repo_registry,
        config_dir: cfg.config_dir.clone(),
        workspace: cfg.workspace,
    };
    let grpc_socket = cfg.config_dir.join("ur-grpc.sock");

    tokio::try_join!(
        accept_loop(socket_path, server),
        urd::grpc_server::serve_grpc(&grpc_socket, grpc_handler),
    )?;

    Ok(())
}
