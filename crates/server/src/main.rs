use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing::info;

use container::NetworkManager;
use ur_server::{Config, CredentialManager, ProcessManager, RepoRegistry};

#[derive(Parser)]
#[command(
    name = "ur-server",
    about = "Ur server — coordination server for containerized agents"
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
    info!("network:    {}", cfg.network.name);
    info!("workers:    {}", cfg.network.worker_name);
    tokio::fs::create_dir_all(&cfg.workspace).await?;
    tokio::fs::create_dir_all(&cfg.config_dir).await?;

    let pid_file = cfg.config_dir.join(ur_config::SERVER_PID_FILE);
    tokio::fs::write(&pid_file, std::process::id().to_string()).await?;

    let repo_registry = Arc::new(RepoRegistry::new(cfg.workspace.clone()));

    // Determine the Docker command from env (docker vs nerdctl)
    let docker_command = match std::env::var("UR_CONTAINER").as_deref() {
        Ok("nerdctl") | Ok("containerd") => "nerdctl".to_string(),
        _ => "docker".to_string(),
    };
    let network_manager = NetworkManager::new(docker_command, cfg.network.worker_name.clone());

    let credential_manager = CredentialManager;
    let process_manager = ProcessManager::new(
        cfg.workspace.clone(),
        repo_registry.clone(),
        credential_manager,
        network_manager,
        cfg.network.clone(),
    );

    let grpc_handler = ur_server::grpc::CoreServiceHandler {
        process_manager,
        repo_registry,
        workspace: cfg.workspace,
        proxy_hostname: cfg.proxy.hostname,
    };
    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.daemon_port));

    let result = ur_server::grpc_server::serve_grpc(addr, grpc_handler).await;

    let _ = tokio::fs::remove_file(&pid_file).await;

    result
}
