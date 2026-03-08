use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing::info;

use urd::{Config, CredentialManager, ProcessManager, RepoRegistry};

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
    info!("daemon port: {}", cfg.daemon_port);
    tokio::fs::create_dir_all(&cfg.workspace).await?;
    tokio::fs::create_dir_all(&cfg.config_dir).await?;

    let repo_registry = Arc::new(RepoRegistry::new(cfg.workspace.clone()));

    let credential_manager = CredentialManager;
    let process_manager =
        ProcessManager::new(cfg.workspace.clone(), repo_registry.clone(), credential_manager);

    let grpc_handler = urd::grpc::CoreServiceHandler {
        process_manager,
        repo_registry,
        workspace: cfg.workspace,
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], cfg.daemon_port));

    urd::grpc_server::serve_grpc(addr, grpc_handler).await?;

    Ok(())
}
