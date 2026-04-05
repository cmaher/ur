use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;

use ur_rpc::proto::builder::builder_daemon_service_server::BuilderDaemonServiceServer;

mod handler;
mod logging;
mod registry;

#[derive(Parser)]
#[command(name = "builderd", about = "Ur builder execution daemon")]
struct Cli {
    #[arg(long, default_value_t = ur_config::DEFAULT_BUILDERD_PORT)]
    port: u16,

    /// IP address to bind to. Defaults to 127.0.0.1. On Linux, `ur start`
    /// passes the Docker bridge gateway IP so containers can reach builderd.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,

    /// Workspace root path for resolving %WORKSPACE% templates in working_dir.
    /// Overrides the BUILDERD_WORKSPACE environment variable.
    #[arg(long, env = "BUILDERD_WORKSPACE")]
    workspace: Option<PathBuf>,

    /// Directory for log files. Defaults to `<config_dir>/logs`.
    #[arg(long)]
    logs_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config_dir = ur_config::resolve_config_dir()?;
    let logs_dir = cli.logs_dir.unwrap_or_else(|| config_dir.join("logs"));
    std::fs::create_dir_all(&logs_dir)?;
    let _log_guard = logging::init(&logs_dir);

    let addr = SocketAddr::new(cli.bind, cli.port);
    info!(
        %addr,
        config_dir = %config_dir.display(),
        workspace = ?cli.workspace,
        "builderd starting"
    );

    let registry = registry::ProcessRegistry::new();
    registry.spawn_reap_task();

    let handler = handler::BuilderDaemonHandler {
        workspace: cli.workspace,
        registry,
    };

    Server::builder()
        .add_service(BuilderDaemonServiceServer::new(handler))
        .serve(addr)
        .await?;

    Ok(())
}
