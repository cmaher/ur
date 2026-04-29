use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use tokio::sync::watch;
use tracing::info;

use container::NetworkManager;

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;
use ticket_db::{GraphManager, TicketRepo};
use ur_server::SnapshotManager;
use ur_server::worker::WorkerModesConfig;
use ur_server::workflow::handlers::build_handlers;
use ur_server::{
    BackupTaskManager, Config, GithubPollerManager, LogCleanupManager, ProjectRegistry,
    RepoPoolManager, WorkerManager, WorkflowEngine,
};
use workflow_db::{WorkerRepo, WorkflowRepo};

/// Both database pools opened at server startup.
struct DatabasePools {
    ticket_pool: PgPool,
    workflow_pool: PgPool,
}

#[derive(Parser)]
#[command(
    name = "ur-server",
    about = "Ur server — coordination server for containerized agents"
)]
struct Cli {}

fn resolve_logs_dir(cfg: &Config) -> anyhow::Result<PathBuf> {
    let logs_dir = if std::path::Path::new("/logs").exists() {
        PathBuf::from("/logs")
    } else {
        cfg.logs_dir.clone()
    };
    std::fs::create_dir_all(&logs_dir)?;
    Ok(logs_dir)
}

fn resolve_workspace_paths(cfg: &Config) -> (PathBuf, PathBuf) {
    let host_workspace = std::env::var(ur_config::UR_HOST_WORKSPACE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| cfg.workspace.clone());
    let local_workspace = if std::env::var(ur_config::UR_HOST_WORKSPACE_ENV).is_ok() {
        PathBuf::from(ur_config::WORKSPACE_MOUNT)
    } else {
        cfg.workspace.clone()
    };
    (host_workspace, local_workspace)
}

async fn open_pool(url: &str, label: &str) -> anyhow::Result<PgPool> {
    let options = PgConnectOptions::from_str(url)
        .map_err(|e| anyhow::anyhow!("invalid {label} database URL: {e}"))?;
    let mut last_err = None;
    for attempt in 1..=10u32 {
        match PgPoolOptions::new()
            .max_connections(5)
            .connect_with(options.clone())
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(e) => {
                tracing::warn!("attempt {attempt}/10: failed to open {label} database: {e}");
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
    Err(anyhow::anyhow!(
        "failed to open {label} database after 10 attempts: {}",
        last_err.unwrap()
    ))
}

async fn init_databases(cfg: &Config) -> anyhow::Result<DatabasePools> {
    let ticket_url = std::env::var(ur_config::UR_TICKET_DB_URL_ENV)
        .unwrap_or_else(|_| cfg.ticket_db.database_url());
    let workflow_url = std::env::var(ur_config::UR_WORKFLOW_DB_URL_ENV)
        .unwrap_or_else(|_| cfg.workflow_db.database_url());

    let (ticket_pool, workflow_pool) = tokio::try_join!(
        open_pool(&ticket_url, "ticket_db"),
        open_pool(&workflow_url, "workflow_db"),
    )?;

    tokio::try_join!(
        async {
            ticket_db::migrate(&ticket_pool)
                .await
                .map_err(|e| anyhow::anyhow!("ticket_db migration failed: {e}"))
        },
        async {
            workflow_db::migrate(&workflow_pool)
                .await
                .map_err(|e| anyhow::anyhow!("workflow_db migration failed: {e}"))
        },
    )?;

    info!(
        ticket_url = %ticket_url,
        workflow_url = %workflow_url,
        "databases initialized"
    );
    Ok(DatabasePools {
        ticket_pool,
        workflow_pool,
    })
}

fn load_worker_modes(cfg: &Config) -> anyhow::Result<WorkerModesConfig> {
    let toml_path = cfg.config_dir.join("ur.toml");
    match std::fs::read_to_string(&toml_path) {
        Ok(contents) => WorkerModesConfig::from_toml(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse worker_modes: {e}")),
        Err(_) => Ok(WorkerModesConfig::default()),
    }
}

struct BackupHandles {
    ticket_shutdown_tx: watch::Sender<bool>,
    ticket_handle: Option<tokio::task::JoinHandle<()>>,
    workflow_shutdown_tx: watch::Sender<bool>,
    workflow_handle: Option<tokio::task::JoinHandle<()>>,
}

fn spawn_backup(
    container_command: &str,
    db_name: &str,
    backup_config: ur_config::BackupConfig,
) -> anyhow::Result<(watch::Sender<bool>, Option<tokio::task::JoinHandle<()>>)> {
    let snapshot_manager = SnapshotManager::new(
        container_command.to_owned(),
        ur_config::DEFAULT_DB_HOST.to_string(),
        db_name.to_owned(),
    );
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let backup_task_manager = BackupTaskManager::new(snapshot_manager, backup_config);
    let handle = backup_task_manager
        .spawn(shutdown_rx)
        .map_err(|e| anyhow::anyhow!("backup configuration error for {db_name}: {e}"))?;
    Ok((shutdown_tx, handle))
}

fn init_backups(cfg: &Config) -> anyhow::Result<BackupHandles> {
    let (ticket_shutdown_tx, ticket_handle) = spawn_backup(
        &cfg.server.container_command,
        &cfg.ticket_db.name,
        cfg.ticket_db.backup.clone(),
    )?;
    let (workflow_shutdown_tx, workflow_handle) = spawn_backup(
        &cfg.server.container_command,
        &cfg.workflow_db.name,
        cfg.workflow_db.backup.clone(),
    )?;
    Ok(BackupHandles {
        ticket_shutdown_tx,
        ticket_handle,
        workflow_shutdown_tx,
        workflow_handle,
    })
}

fn init_log_cleanup(
    logs_dir: &std::path::Path,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let manager = LogCleanupManager::new(
        logs_dir.to_path_buf(),
        std::time::Duration::from_secs(10 * 60),
        std::time::Duration::from_secs(7 * 86400),
    );
    let handle = manager.spawn(shutdown_rx);
    (shutdown_tx, handle)
}

async fn reconcile_slots(
    worker_repo: &WorkerRepo,
    cfg: &Config,
    local_workspace: &std::path::Path,
    host_workspace: &std::path::Path,
) -> anyhow::Result<()> {
    let pool_root = local_workspace.join("pool");
    let project_pool_dirs: std::collections::HashMap<String, PathBuf> = cfg
        .projects
        .keys()
        .map(|k| (k.clone(), pool_root.join(k)))
        .collect();
    let slot_result = worker_repo
        .reconcile_slots(&project_pool_dirs, local_workspace, host_workspace)
        .await
        .map_err(|e| anyhow::anyhow!("slot reconciliation failed: {e}"))?;
    info!(
        deleted = ?slot_result.deleted_stale,
        inserted = ?slot_result.inserted_orphaned,
        "slot reconciliation complete"
    );
    Ok(())
}

async fn reconcile_workers(worker_repo: &WorkerRepo, docker_command: &str) -> anyhow::Result<()> {
    let docker_cmd = docker_command.to_owned();
    let worker_result = worker_repo
        .reconcile_workers(|container_id| {
            let cmd = docker_cmd.clone();
            async move {
                tokio::process::Command::new(&cmd)
                    .args(["inspect", "--format", "{{.State.Running}}", &container_id])
                    .output()
                    .await
                    .map(|o| o.stdout.starts_with(b"true"))
                    .unwrap_or(false)
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("worker reconciliation failed: {e}"))?;
    info!(
        reclaimed = ?worker_result.reclaimed,
        stopped = ?worker_result.marked_stopped,
        "worker reconciliation complete"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn init_and_serve(
    cfg: &Config,
    ticket_pool: &PgPool,
    workflow_pool: &PgPool,
    worker_manager: WorkerManager,
    repo_pool_manager: RepoPoolManager,
    worker_repo: WorkerRepo,
    builderd_addr: String,
    host_workspace: PathBuf,
    project_registry: ProjectRegistry,
) -> anyhow::Result<()> {
    let graph_manager = GraphManager::new(ticket_pool.clone());
    let ticket_repo = TicketRepo::new(ticket_pool.clone(), graph_manager);
    let workflow_repo = WorkflowRepo::new(workflow_pool.clone());

    let fallback_interval =
        std::time::Duration::from_millis(cfg.server.ui_event_fallback_interval_ms);
    let ticket_url = std::env::var(ur_config::UR_TICKET_DB_URL_ENV)
        .unwrap_or_else(|_| cfg.ticket_db.database_url());
    let workflow_url = std::env::var(ur_config::UR_WORKFLOW_DB_URL_ENV)
        .unwrap_or_else(|_| cfg.workflow_db.database_url());
    let ui_event_poller = ur_server::UiEventPoller::new(
        ticket_pool.clone(),
        ticket_url,
        workflow_pool.clone(),
        workflow_url,
        fallback_interval,
    );

    let ticket_handler = ur_server::grpc_ticket::TicketServiceHandler {
        ticket_repo: ticket_repo.clone(),
        workflow_repo: workflow_repo.clone(),
        project_registry: project_registry.clone(),
        transition_tx: None, // set in serve_grpc_servers after builderd connects
        cancel_tx: None,     // set in serve_grpc_servers after builderd connects
        ui_event_poller: Some(ui_event_poller.clone()),
        worker_manager: Some(worker_manager.clone()),
    };

    let launch_manager = ur_server::grpc::LaunchManager {
        worker_manager: worker_manager.clone(),
        repo_pool_manager: repo_pool_manager.clone(),
        proxy_hostname: cfg.proxy.hostname.clone(),
        project_registry: project_registry.clone(),
        worker_repo: worker_repo.clone(),
        ticket_repo: ticket_repo.clone(),
        workflow_repo: workflow_repo.clone(),
        network_config: cfg.network.clone(),
    };

    let grpc_handler = ur_server::grpc::CoreServiceHandler {
        launch_manager: launch_manager.clone(),
        worker_manager: worker_manager.clone(),
        repo_pool_manager,
        workspace: cfg.workspace.clone(),
        proxy_hostname: cfg.proxy.hostname.clone(),
        project_registry: project_registry.clone(),
        worker_repo: worker_repo.clone(),
        ticket_repo: ticket_repo.clone(),
        workflow_repo: workflow_repo.clone(),
        network_config: cfg.network.clone(),
        builderd_addr: builderd_addr.clone(),
        config_dir: cfg.config_dir.clone(),
    };

    serve_grpc_servers(
        cfg.server_port,
        cfg.worker_port,
        cfg.network.worker_prefix.clone(),
        project_registry,
        grpc_handler,
        ticket_handler,
        worker_manager,
        worker_repo,
        ticket_repo,
        workflow_repo,
        builderd_addr,
        host_workspace,
        Arc::new(cfg.clone()),
        ui_event_poller,
        launch_manager,
    )
    .await
}

struct WorkflowServices {
    transition_tx: tokio::sync::mpsc::Sender<ur_server::workflow::TransitionRequest>,
    cancel_tx: tokio::sync::mpsc::Sender<String>,
    shutdown_tx: watch::Sender<bool>,
    engine_handle: tokio::task::JoinHandle<()>,
    coordinator_handle: tokio::task::JoinHandle<()>,
    poller_handle: tokio::task::JoinHandle<()>,
    ui_poller_handle: tokio::task::JoinHandle<()>,
}

#[allow(clippy::too_many_arguments)]
fn spawn_workflow_services(
    network_prefix: &str,
    ticket_repo: &TicketRepo,
    workflow_repo: &WorkflowRepo,
    worker_repo: &WorkerRepo,
    worker_manager: &WorkerManager,
    ticket_handler: &ur_server::grpc_ticket::TicketServiceHandler,
    builderd_addr: &str,
    config: &Arc<ur_config::Config>,
    ui_event_poller: ur_server::UiEventPoller,
) -> WorkflowServices {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let workflow_retry_channel =
        ur_rpc::retry::RetryChannel::new(builderd_addr, ur_rpc::retry::RetryConfig::default())
            .expect("failed to create builderd retry channel for workflow engine");
    let workflow_builderd_client =
        ur_rpc::proto::builder::BuilderdClient::new(workflow_retry_channel.channel().clone());

    let workflow_ticket_client =
        ur_server::workflow::ticket_client::TicketClient::new(ticket_handler.clone());
    let poller_ticket_client = workflow_ticket_client.clone();
    let handlers = build_handlers(workflow_ticket_client);

    let (transition_tx, coordinator_rx) = ur_server::workflow::coordinator_channel(256);
    let (cancel_tx, cancel_rx) = ur_server::workflow::coordinator_cancel_channel(256);

    let engine = WorkflowEngine::new(
        ticket_repo.clone(),
        workflow_repo.clone(),
        worker_repo.clone(),
        network_prefix.to_owned(),
        workflow_builderd_client.clone(),
        config.clone(),
        handlers.clone(),
        transition_tx.clone(),
        worker_manager.clone(),
    );
    let engine_handle = engine.spawn(shutdown_rx.clone());

    let coordinator_ctx = ur_server::workflow::WorkflowContext {
        ticket_repo: ticket_repo.clone(),
        workflow_repo: workflow_repo.clone(),
        worker_repo: worker_repo.clone(),
        worker_prefix: network_prefix.to_owned(),
        builderd_client: workflow_builderd_client.clone(),
        config: config.clone(),
        transition_tx: transition_tx.clone(),
        worker_manager: worker_manager.clone(),
    };
    let coordinator = ur_server::workflow::WorkflowCoordinator::new(
        coordinator_rx,
        cancel_rx,
        coordinator_ctx,
        &handlers,
    );
    let coordinator_handle = coordinator.spawn(shutdown_rx.clone());

    let scan_interval = std::time::Duration::from_secs(config.server.github_scan_interval_secs);
    let poller = GithubPollerManager::new(
        ticket_repo.clone(),
        workflow_repo.clone(),
        workflow_builderd_client,
        scan_interval,
        transition_tx.clone(),
        poller_ticket_client,
        worker_manager.clone(),
        (**config).clone(),
    );
    let poller_handle = poller.spawn(shutdown_rx.clone());

    let ui_poller_handle = ui_event_poller.spawn(shutdown_rx);

    WorkflowServices {
        transition_tx,
        cancel_tx,
        shutdown_tx,
        engine_handle,
        coordinator_handle,
        poller_handle,
        ui_poller_handle,
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_grpc_servers(
    server_port: u16,
    worker_port: u16,
    network_prefix: String,
    project_registry: ProjectRegistry,
    grpc_handler: ur_server::grpc::CoreServiceHandler,
    mut ticket_handler: ur_server::grpc_ticket::TicketServiceHandler,
    worker_manager: WorkerManager,
    worker_repo: WorkerRepo,
    ticket_repo: TicketRepo,
    workflow_repo: WorkflowRepo,
    builderd_addr: String,
    host_workspace: PathBuf,
    config: Arc<ur_config::Config>,
    ui_event_poller: ur_server::UiEventPoller,
    launch_manager: ur_server::grpc::LaunchManager,
) -> anyhow::Result<()> {
    let host_addr = SocketAddr::from(([0, 0, 0, 0], server_port));
    let worker_addr = SocketAddr::from(([0, 0, 0, 0], worker_port));

    let wf = spawn_workflow_services(
        &network_prefix,
        &ticket_repo,
        &workflow_repo,
        &worker_repo,
        &worker_manager,
        &ticket_handler,
        &builderd_addr,
        &config,
        ui_event_poller,
    );

    ticket_handler.transition_tx = Some(wf.transition_tx.clone());
    ticket_handler.cancel_tx = Some(wf.cancel_tx);

    let host_server =
        ur_server::grpc_server::serve_grpc(host_addr, grpc_handler, ticket_handler.clone());

    let remote_repo_handler = {
        let retry_channel =
            ur_rpc::retry::RetryChannel::new(&builderd_addr, ur_rpc::retry::RetryConfig::default())
                .expect("failed to create builderd retry channel for remote_repo service");
        let builderd_client =
            ur_rpc::proto::builder::BuilderdClient::new(retry_channel.channel().clone());
        ur_server::grpc_remote_repo::RemoteRepoServiceHandler { builderd_client }
    };

    let mut worker_ticket_handler = ticket_handler;
    worker_ticket_handler.ui_event_poller = None;

    let worker_server = ur_server::grpc_server::serve_worker_grpc(
        worker_addr,
        worker_manager,
        worker_repo,
        ticket_repo,
        workflow_repo,
        network_prefix,
        project_registry,
        builderd_addr,
        host_workspace,
        config.git_branch_prefix.clone(),
        worker_ticket_handler,
        remote_repo_handler,
        wf.transition_tx,
        launch_manager,
    );

    let server_result = tokio::try_join!(host_server, worker_server).map(|_| ());

    // Signal workflow engine, coordinator, github poller, and UI event poller to shut down.
    let _ = wf.shutdown_tx.send(true);
    let _ = wf.engine_handle.await;
    let _ = wf.coordinator_handle.await;
    let _ = wf.poller_handle.await;
    let _ = wf.ui_poller_handle.await;

    server_result
}

#[allow(clippy::too_many_arguments)]
async fn init_managers(
    cfg: &Config,
    workflow_pool: &PgPool,
    local_workspace: &Path,
    host_workspace: &Path,
    host_config_dir: &Path,
    logs_dir: &Path,
    worker_modes: WorkerModesConfig,
    network_manager: NetworkManager,
    docker_command: &str,
) -> anyhow::Result<(
    String,
    WorkerRepo,
    RepoPoolManager,
    WorkerManager,
    ProjectRegistry,
)> {
    let builderd_addr = std::env::var(ur_config::BUILDERD_ADDR_ENV)
        .unwrap_or_else(|_| format!("http://host.docker.internal:{}", cfg.builderd_port));

    let builderd_retry_channel =
        ur_rpc::retry::RetryChannel::new(&builderd_addr, ur_rpc::retry::RetryConfig::default())
            .map_err(|e| anyhow::anyhow!("failed to create builderd retry channel: {e}"))?;
    let builderd_client =
        ur_rpc::proto::builder::BuilderdClient::new(builderd_retry_channel.channel().clone());
    let local_repo = local_repo::GitBackend {
        client: builderd_client.clone(),
    };
    let worker_repo = WorkerRepo::new(workflow_pool.clone());

    reconcile_slots(&worker_repo, cfg, local_workspace, host_workspace).await?;

    ur_server::hostexec::materialize_shim(&cfg.config_dir)
        .map_err(|e| anyhow::anyhow!("failed to materialize hostexec script shim: {e}"))?;

    let hostexec_config =
        ur_server::hostexec::HostExecConfigManager::load(&cfg.config_dir, &cfg.hostexec)
            .expect("failed to load hostexec config");
    let project_registry = ProjectRegistry::new(cfg.projects.clone(), hostexec_config);

    let repo_pool_manager = RepoPoolManager::new(
        cfg,
        local_workspace.to_path_buf(),
        host_workspace.to_path_buf(),
        builderd_client,
        local_repo,
        worker_repo.clone(),
        host_config_dir.to_path_buf(),
        project_registry.clone(),
    );
    let host_logs_dir = std::env::var("UR_HOST_LOGS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| logs_dir.to_path_buf());
    let worker_manager = WorkerManager::new(
        local_workspace.to_path_buf(),
        host_config_dir.to_path_buf(),
        logs_dir.to_path_buf(),
        host_logs_dir,
        repo_pool_manager.clone(),
        network_manager,
        cfg.network.clone(),
        cfg.worker_port,
        worker_modes,
        worker_repo.clone(),
        cfg.global_skills.clone(),
    );

    reconcile_workers(&worker_repo, docker_command).await?;

    let stale_deleted = worker_repo
        .cleanup_stale_workers(cfg.server.stale_worker_ttl_days)
        .await
        .map_err(|e| anyhow::anyhow!("stale worker cleanup failed: {e}"))?;
    info!(
        count = stale_deleted.len(),
        deleted = ?stale_deleted,
        ttl_days = cfg.server.stale_worker_ttl_days,
        "stale worker cleanup complete"
    );

    Ok((
        builderd_addr,
        worker_repo,
        repo_pool_manager,
        worker_manager,
        project_registry,
    ))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();

    let cfg = Config::load()?;

    let logs_dir = resolve_logs_dir(&cfg)?;
    let _log_guard = ur_server::logging::init(&logs_dir);
    info!(
        config_dir = %cfg.config_dir.display(),
        server_port = cfg.server_port,
        worker_port = cfg.worker_port,
        network = cfg.network.name,
        workers = cfg.network.worker_name,
        "server config loaded"
    );

    let (host_workspace, local_workspace) = resolve_workspace_paths(&cfg);
    info!(
        local_workspace = %local_workspace.display(),
        host_workspace = %host_workspace.display(),
        "workspace paths resolved"
    );

    tokio::fs::create_dir_all(&local_workspace).await?;
    tokio::fs::create_dir_all(&cfg.config_dir).await?;

    let pid_file = cfg.config_dir.join(ur_config::SERVER_PID_FILE);
    tokio::fs::write(&pid_file, std::process::id().to_string()).await?;

    let docker_command = cfg.server.container_command.clone();
    let network_manager =
        NetworkManager::new(docker_command.clone(), cfg.network.worker_name.clone());

    let host_config_dir = std::env::var(ur_config::UR_HOST_CONFIG_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| cfg.config_dir.clone());
    info!(host_config_dir = %host_config_dir.display(), "host config resolved");

    let worker_modes = load_worker_modes(&cfg)?;
    let pools = init_databases(&cfg).await?;
    let backup_handles = init_backups(&cfg)?;

    let (log_cleanup_shutdown_tx, log_cleanup_handle) = init_log_cleanup(&logs_dir);

    let (builderd_addr, worker_repo, repo_pool_manager, worker_manager, project_registry) =
        init_managers(
            &cfg,
            &pools.workflow_pool,
            &local_workspace,
            &host_workspace,
            &host_config_dir,
            &logs_dir,
            worker_modes,
            network_manager,
            &docker_command,
        )
        .await?;

    let result = init_and_serve(
        &cfg,
        &pools.ticket_pool,
        &pools.workflow_pool,
        worker_manager,
        repo_pool_manager,
        worker_repo,
        builderd_addr,
        host_workspace,
        project_registry,
    )
    .await;

    // Signal background tasks to stop and wait for them
    let _ = backup_handles.ticket_shutdown_tx.send(true);
    if let Some(handle) = backup_handles.ticket_handle {
        let _ = handle.await;
    }
    let _ = backup_handles.workflow_shutdown_tx.send(true);
    if let Some(handle) = backup_handles.workflow_handle {
        let _ = handle.await;
    }
    let _ = log_cleanup_shutdown_tx.send(true);
    let _ = log_cleanup_handle.await;

    let _ = tokio::fs::remove_file(&pid_file).await;

    result
}
