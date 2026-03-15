use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::watch;
use tracing::info;

use container::NetworkManager;
use ur_db::{DatabaseManager, GraphManager, SnapshotManager, TicketRepo};
use ur_server::process::PromptModesConfig;
use ur_server::{
    BackupTaskManager, BuilderdClient, Config, ProcessManager, RepoPoolManager, RepoRegistry,
};

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
        worker_port = cfg.worker_port,
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

    // Load prompt modes from ur.toml (falls back to hardcoded defaults)
    let prompt_modes = {
        let toml_path = cfg.config_dir.join("ur.toml");
        match std::fs::read_to_string(&toml_path) {
            Ok(contents) => PromptModesConfig::from_toml(&contents)
                .map_err(|e| anyhow::anyhow!("failed to parse prompt_modes: {e}"))?,
            Err(_) => PromptModesConfig::default(),
        }
    };

    // Initialize SQLite database
    let db_path = cfg.config_dir.join("ur.db");
    let db_path_str = db_path.to_string_lossy();
    let db = DatabaseManager::open(&db_path_str)
        .await
        .map_err(|e| anyhow::anyhow!("failed to open database: {e}"))?;
    info!(db_path = %db_path.display(), "database initialized");

    // Start periodic backup task (if configured)
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let snapshot_manager = SnapshotManager::new(db.pool().clone());
    let backup_task_manager = BackupTaskManager::new(snapshot_manager, cfg.backup.clone());
    let backup_handle = backup_task_manager
        .spawn(shutdown_rx)
        .map_err(|e| anyhow::anyhow!("backup configuration error: {e}"))?;

    let builderd_addr = std::env::var(ur_config::HOSTD_ADDR_ENV)
        .unwrap_or_else(|_| format!("http://host.docker.internal:{}", cfg.hostd_port));

    let builderd_client = BuilderdClient::new(builderd_addr.clone());
    let repo_pool_manager =
        RepoPoolManager::new(&cfg, local_workspace.clone(), host_workspace, builderd_client);
    let process_manager = ProcessManager::new(
        local_workspace,
        host_config_dir,
        repo_registry.clone(),
        repo_pool_manager.clone(),
        network_manager,
        cfg.network.clone(),
        cfg.worker_port,
        prompt_modes,
    );

    #[cfg(feature = "hostexec")]
    let hostexec_config =
        ur_server::hostexec::HostExecConfigManager::load(&cfg.config_dir, &cfg.hostexec)
            .expect("failed to load hostexec config");

    #[cfg(feature = "rag")]
    let rag_handler = {
        use std::sync::Arc;

        let model = rag::model::model_info(&cfg.rag.embedding_model).unwrap_or_else(|| {
            let supported = ur_config::supported_model_names().join(", ");
            panic!(
                "unknown embedding model '{}' — supported models: {supported}",
                cfg.rag.embedding_model,
            );
        });

        let qdrant_url = format!(
            "http://{}:{}",
            cfg.rag.qdrant_hostname,
            ur_config::DEFAULT_QDRANT_PORT,
        );
        info!(qdrant_url = %qdrant_url, "connecting to Qdrant");

        let qdrant = Arc::new(
            qdrant_client::Qdrant::from_url(&qdrant_url)
                .build()
                .expect("failed to create Qdrant client"),
        );

        let embedding_model = Arc::new(
            fastembed::TextEmbedding::try_new(
                fastembed::InitOptions::new(model.fastembed_model.clone())
                    .with_show_download_progress(false),
            )
            .expect("failed to load embedding model — run `ur rag model download`"),
        );

        let rag_manager = rag::RagManager::new(
            qdrant,
            embedding_model,
            model.download.vector_size,
            cfg.rag.embedding_model.clone(),
        );

        ur_server::rag::RagServiceHandler {
            rag_manager,
            config_dir: cfg.config_dir.clone(),
        }
    };

    #[cfg(feature = "ticket")]
    let ticket_handler = {
        let graph_manager = GraphManager::new(db.pool().clone());
        let ticket_repo = TicketRepo::new(db.pool().clone(), graph_manager);
        ur_server::grpc_ticket::TicketServiceHandler { ticket_repo }
    };

    let grpc_handler = ur_server::grpc::CoreServiceHandler {
        process_manager: process_manager.clone(),
        repo_pool_manager,
        repo_registry: repo_registry.clone(),
        workspace: cfg.workspace,
        proxy_hostname: cfg.proxy.hostname,
        projects: cfg.projects.clone(),
        #[cfg(feature = "hostexec")]
        hostexec_config: hostexec_config.clone(),
        #[cfg(feature = "hostexec")]
        builderd_addr: builderd_addr.clone(),
    };

    let host_addr = SocketAddr::from(([0, 0, 0, 0], cfg.daemon_port));
    let worker_addr = SocketAddr::from(([0, 0, 0, 0], cfg.worker_port));

    let host_server = ur_server::grpc_server::serve_grpc(
        host_addr,
        grpc_handler,
        #[cfg(feature = "rag")]
        rag_handler.clone(),
        #[cfg(feature = "ticket")]
        ticket_handler.clone(),
    );

    let worker_server = ur_server::grpc_server::serve_worker_grpc(
        worker_addr,
        process_manager,
        repo_registry,
        cfg.projects,
        #[cfg(feature = "hostexec")]
        hostexec_config,
        #[cfg(feature = "hostexec")]
        builderd_addr,
        #[cfg(feature = "rag")]
        rag_handler,
        #[cfg(feature = "ticket")]
        ticket_handler,
    );

    let result = tokio::try_join!(host_server, worker_server).map(|_| ());

    // Signal backup task to stop and wait for it
    let _ = shutdown_tx.send(true);
    if let Some(handle) = backup_handle {
        let _ = handle.await;
    }

    let _ = tokio::fs::remove_file(&pid_file).await;

    result
}
