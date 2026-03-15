use std::net::SocketAddr;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;

use ur_rpc::proto::builder::builder_daemon_service_server::BuilderDaemonServiceServer;

mod handler;
mod logging;

#[derive(Parser)]
#[command(name = "builderd", about = "Ur builder execution daemon")]
struct Cli {
    #[arg(long, default_value_t = ur_config::DEFAULT_HOSTD_PORT)]
    port: u16,
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
        "builderd starting"
    );

    Server::builder()
        .add_service(BuilderDaemonServiceServer::new(handler::BuilderDaemonHandler))
        .serve(addr)
        .await?;

    Ok(())
}
