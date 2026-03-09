use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing::info;

use urd::{Config, CredentialManager, ProcessManager, ProxyManager, RepoRegistry};

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
    info!("network:    {}", cfg.network.name);
    tokio::fs::create_dir_all(&cfg.workspace).await?;
    tokio::fs::create_dir_all(&cfg.config_dir).await?;

    let pid_file = cfg.config_dir.join(ur_config::URD_PID_FILE);
    tokio::fs::write(&pid_file, std::process::id().to_string()).await?;

    let repo_registry = Arc::new(RepoRegistry::new(cfg.workspace.clone()));

    let credential_manager = CredentialManager;
    let process_manager = ProcessManager::new(
        cfg.workspace.clone(),
        repo_registry.clone(),
        credential_manager,
        cfg.proxy.clone(),
    );

    // Start the forward proxy on 0.0.0.0 so containers on the Docker network
    // can reach it via the urd hostname resolved through Docker DNS.
    let proxy_addr: SocketAddr = SocketAddr::from(([0, 0, 0, 0], cfg.proxy.port));
    let allowlist = Arc::new(tokio::sync::RwLock::new(cfg.proxy.allowlist_set()));
    let proxy_manager = ProxyManager::new(allowlist);
    let _proxy_handle = proxy_manager.serve(proxy_addr).await?;

    let grpc_handler = urd::grpc::CoreServiceHandler {
        process_manager,
        repo_registry,
        workspace: cfg.workspace,
        network: cfg.network,
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], cfg.daemon_port));

    let result = urd::grpc_server::serve_grpc(addr, grpc_handler).await;

    let _ = tokio::fs::remove_file(&pid_file).await;

    result
}
