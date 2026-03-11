use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing::info;

use container::NetworkManager;
use ur_server::process::PromptTemplatesConfig;
use ur_server::{Config, ProcessManager, RepoPoolManager, RepoRegistry};

#[derive(Parser)]
#[command(
    name = "ur-server",
    about = "Ur server — coordination server for containerized agents"
)]
struct Cli {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ur_server::logging::init();

    let _cli = Cli::parse();

    let cfg = Config::load()?;
    info!(
        config_dir = %cfg.config_dir.display(),
        daemon_port = cfg.daemon_port,
        network = cfg.network.name,
        workers = cfg.network.worker_name,
        "server config loaded"
    );

    // When running in a container, the workspace is mounted at /workspace.
    // Use UR_HOST_WORKSPACE for host-side paths (ur-hostd CWD mapping),
    // and the mount point for local filesystem operations (mkdir, git init).
    let host_workspace = std::env::var(ur_config::UR_HOST_WORKSPACE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| cfg.workspace.clone());
    let local_workspace = if std::env::var(ur_config::UR_HOST_WORKSPACE_ENV).is_ok() {
        PathBuf::from(ur_config::WORKSPACE_MOUNT)
    } else {
        cfg.workspace.clone()
    };
    info!(
        local_workspace = %local_workspace.display(),
        host_workspace = %host_workspace.display(),
        "workspace paths resolved"
    );

    tokio::fs::create_dir_all(&local_workspace).await?;
    tokio::fs::create_dir_all(&cfg.config_dir).await?;

    let pid_file = cfg.config_dir.join(ur_config::SERVER_PID_FILE);
    tokio::fs::write(&pid_file, std::process::id().to_string()).await?;

    let repo_registry = Arc::new(RepoRegistry::new(host_workspace.clone()));

    // Determine the Docker command from env (docker vs nerdctl)
    let docker_command = match std::env::var("UR_CONTAINER").as_deref() {
        Ok("nerdctl") | Ok("containerd") => "nerdctl".to_string(),
        _ => "docker".to_string(),
    };
    let network_manager = NetworkManager::new(docker_command, cfg.network.worker_name.clone());

    // UR_HOST_CONFIG is the host-side config directory path, needed for
    // constructing volume mounts in agent containers (which use host paths
    // via the Docker socket). Falls back to the server's own config_dir
    // (only correct when the server runs directly on the host, not in a container).
    let host_config_dir = std::env::var(ur_config::UR_HOST_CONFIG_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| cfg.config_dir.clone());
    info!(host_config_dir = %host_config_dir.display(), "host config resolved");

    // Load prompt templates from ur.toml (falls back to hardcoded defaults)
    let prompt_templates = {
        let toml_path = cfg.config_dir.join("ur.toml");
        match std::fs::read_to_string(&toml_path) {
            Ok(contents) => PromptTemplatesConfig::from_toml(&contents)
                .map_err(|e| anyhow::anyhow!("failed to parse prompt_templates: {e}"))?,
            Err(_) => PromptTemplatesConfig::default(),
        }
    };

    let hostd_addr = std::env::var(ur_config::HOSTD_ADDR_ENV)
        .unwrap_or_else(|_| format!("http://host.docker.internal:{}", cfg.hostd_port));

    let repo_pool_manager = RepoPoolManager::new(
        &cfg,
        local_workspace.clone(),
        host_workspace,
        hostd_addr.clone(),
    );
    let process_manager = ProcessManager::new(
        local_workspace,
        host_config_dir,
        repo_registry.clone(),
        repo_pool_manager.clone(),
        network_manager,
        cfg.network.clone(),
        prompt_templates,
    );

    #[cfg(feature = "hostexec")]
    let hostexec_config = ur_server::hostexec::HostExecConfigManager::load(&cfg.config_dir)
        .expect("failed to load hostexec config");

    let grpc_handler = ur_server::grpc::CoreServiceHandler {
        process_manager,
        repo_pool_manager,
        repo_registry,
        workspace: cfg.workspace,
        proxy_hostname: cfg.proxy.hostname,
        projects: cfg.projects,
        #[cfg(feature = "hostexec")]
        hostexec_config,
        #[cfg(feature = "hostexec")]
        hostd_addr,
    };
    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.daemon_port));

    let result = ur_server::grpc_server::serve_grpc(addr, grpc_handler).await;

    let _ = tokio::fs::remove_file(&pid_file).await;

    result
}
