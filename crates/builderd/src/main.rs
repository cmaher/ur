use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;

use ur_rpc::proto::builder::builder_daemon_service_server::BuilderDaemonServiceServer;

mod handler;
mod logging;

#[derive(Parser)]
#[command(name = "builderd", about = "Ur builder execution daemon")]
struct Cli {
    #[arg(long, default_value_t = ur_config::DEFAULT_BUILDERD_PORT)]
    port: u16,

    /// Workspace root path for resolving %WORKSPACE% templates in working_dir.
    /// Overrides the BUILDERD_WORKSPACE environment variable.
    #[arg(long, env = "BUILDERD_WORKSPACE")]
    workspace: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_dir = ur_config::resolve_config_dir()?;
    let _log_guard = logging::init(&config_dir);

    let cli = Cli::parse();

    let addr = SocketAddr::from(([127, 0, 0, 1], cli.port));
    info!(
        %addr,
        config_dir = %config_dir.display(),
        workspace = ?cli.workspace,
        "builderd starting"
    );

    let handler = handler::BuilderDaemonHandler {
        workspace: cli.workspace,
    };

    Server::builder()
        .add_service(BuilderDaemonServiceServer::new(handler))
        .serve(addr)
        .await?;

    Ok(())
}
