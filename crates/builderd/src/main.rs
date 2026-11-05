use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;

use ur_rpc::proto::builder::builder_daemon_service_server::BuilderDaemonServiceServer;
use ur_rpc::proto::builder_container::builder_container_service_server::BuilderContainerServiceServer;
use ur_rpc::proto::builder_local_worker::builder_local_worker_service_server::BuilderLocalWorkerServiceServer;
use ur_rpc::proto::builder_pool::builder_pool_service_server::BuilderPoolServiceServer;
use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonServiceServer;

mod barrier;
mod container_handler;
mod handler;
mod local_settings;
mod local_worker;
mod local_worker_handler;
mod local_workspace;
mod logging;
mod overlay;
mod pool_handler;
mod registry;
mod workerd_router;

/// How often a local worker's exit watcher polls its agent pane.
const LOCAL_EXIT_POLL: std::time::Duration = std::time::Duration::from_secs(5);

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

    /// Root config directory for project config files (e.g., projects/<key>/local/).
    /// Defaults to `~/.ur` (same as $UR_CONFIG). Overrides the UR_CONFIG env var.
    #[arg(long)]
    config_dir: Option<PathBuf>,

    /// Agent asset root for local workers (`potential-skills/`, `instructions/`,
    /// `shared-instructions/`), installed by `scripts/build/install.sh`.
    /// Defaults to `<config_dir>/agent-shared`.
    #[arg(long)]
    agent_shared: Option<PathBuf>,
}

/// builderd's address as a host process dials it: an unspecified bind
/// (`0.0.0.0` / `::`) is reached on loopback.
fn host_workerd_addr(addr: SocketAddr) -> String {
    let ip = if addr.ip().is_unspecified() {
        IpAddr::from([127, 0, 0, 1])
    } else {
        addr.ip()
    };
    SocketAddr::new(ip, addr.port()).to_string()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config_dir = match cli.config_dir {
        Some(dir) => dir,
        None => ur_config::resolve_config_dir()?,
    };
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

    let pool_workspace = cli
        .workspace
        .clone()
        .unwrap_or_else(|| config_dir.join("workspace"));
    let overlays = overlay::LocalOverlayManager {
        workspace: pool_workspace.clone(),
        config_dir: config_dir.clone(),
        active: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let local_registry = local_worker::LocalWorkerRegistry::default();
    let local_workers = local_worker::LocalWorkerManager::new(
        local_workspace::LocalWorkspaceManager::new(
            config_dir.clone(),
            pool_workspace.clone(),
            home,
            cli.agent_shared
                .unwrap_or_else(|| config_dir.join("agent-shared")),
            local_settings::LocalSettingsManager,
        ),
        local_registry.clone(),
        ur_config::LOCAL_TMUX_SOCKET.to_owned(),
        LOCAL_EXIT_POLL,
        host_workerd_addr(addr),
        None,
        barrier::WorkerBarrier::new(barrier::TOMBSTONE_TTL),
    );
    let pool_handler = pool_handler::BuilderPoolHandler {
        workspace: pool_workspace,
        overlays: overlays.clone(),
        slot_locks: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };

    let handler = handler::BuilderDaemonHandler {
        workspace: cli.workspace,
        registry,
        overlays,
    };

    let container_handler = container_handler::BuilderContainerHandler::new(
        barrier::WorkerBarrier::new(barrier::TOMBSTONE_TTL),
    );

    // Before serving: ListLocalWorkers must never under-report live sessions
    // left by a previous builderd (the server releases unlisted slots).
    local_workers
        .adopt_existing()
        .await
        .map_err(|s| anyhow::anyhow!("adopt local workers: {}", s.message()))?;

    Server::builder()
        .add_service(BuilderDaemonServiceServer::new(handler))
        .add_service(BuilderContainerServiceServer::new(container_handler))
        .add_service(BuilderPoolServiceServer::new(pool_handler))
        .add_service(BuilderLocalWorkerServiceServer::new(
            local_worker_handler::BuilderLocalWorkerHandler {
                manager: local_workers,
            },
        ))
        .add_service(WorkerDaemonServiceServer::new(
            workerd_router::WorkerdRouter {
                registry: local_registry,
            },
        ))
        .serve(addr)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_workerd_addr_maps_unspecified_binds_to_loopback() {
        let port = 12323;
        for (bind, want) in [
            ("0.0.0.0", "127.0.0.1:12323"),
            ("::", "127.0.0.1:12323"),
            ("127.0.0.1", "127.0.0.1:12323"),
            ("172.17.0.1", "172.17.0.1:12323"),
        ] {
            let addr = SocketAddr::new(bind.parse().unwrap(), port);
            assert_eq!(host_workerd_addr(addr), want, "{bind}");
        }
    }
}
