use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing::info;

use container::ContainerRuntime;
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

    // Start the forward proxy on the host gateway IP.
    // The proxy binds to the gateway IP so containers can reach it, but localhost cannot.
    let rt = container::runtime_from_env();
    let host_ip = rt.host_gateway_ip()?;
    let proxy_addr: SocketAddr = format!("{}:{}", host_ip, cfg.proxy.port).parse()?;
    let allowlist = Arc::new(tokio::sync::RwLock::new(cfg.proxy.allowlist_set()));
    let proxy_manager = ProxyManager::new(allowlist);
    let _proxy_handle = proxy_manager.serve(proxy_addr).await?;

    let grpc_handler = urd::grpc::CoreServiceHandler {
        process_manager,
        repo_registry,
        workspace: cfg.workspace,
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], cfg.daemon_port));

    let result = urd::grpc_server::serve_grpc(addr, grpc_handler).await;

    let _ = tokio::fs::remove_file(&pid_file).await;

    result
}
