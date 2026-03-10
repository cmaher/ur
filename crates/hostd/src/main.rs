use std::net::SocketAddr;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;

use ur_rpc::proto::hostd::host_daemon_service_server::HostDaemonServiceServer;

mod handler;
mod logging;

#[derive(Parser)]
#[command(name = "ur-hostd", about = "Ur host execution daemon")]
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
        "ur-hostd starting"
    );

    Server::builder()
        .add_service(HostDaemonServiceServer::new(handler::HostDaemonHandler))
        .serve(addr)
        .await?;

    Ok(())
}
