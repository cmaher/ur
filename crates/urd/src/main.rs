use std::sync::Arc;

use clap::Parser;
use tracing::info;

use urd::{Config, ProcessManager, RepoRegistry};

#[derive(Parser)]
#[command(
    name = "urd",
    about = "Ur daemon — coordination server for containerized agents"
)]
struct Cli {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let _cli = Cli::parse();

    let cfg = Config::load()?;
    info!("config dir: {}", cfg.config_dir.display());
    info!("workspace:  {}", cfg.workspace.display());
    tokio::fs::create_dir_all(&cfg.workspace).await?;
    tokio::fs::create_dir_all(&cfg.config_dir).await?;

    let repo_registry = Arc::new(RepoRegistry::new(cfg.workspace.clone()));

    let process_manager = ProcessManager::new(cfg.workspace.clone(), repo_registry.clone());

    let grpc_handler = urd::grpc::CoreServiceHandler {
        process_manager,
        repo_registry,
        workspace: cfg.workspace,
        agent_grpc_port: cfg.agent_grpc_port,
    };
    let grpc_socket = cfg.config_dir.join("ur-grpc.sock");

    urd::grpc_server::serve_grpc(&grpc_socket, grpc_handler).await?;

    Ok(())
}
