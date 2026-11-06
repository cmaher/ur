// Server restart: slot reconciliation against the pool directory on disk.
// Worker rows are reconciled against builderd's lists by
// `WorkerReconcileManager` (tests in `local_launch_test.rs`).

use std::collections::HashMap;
use std::path::Path;

fn test_config(dir: &Path, workspace: &Path) -> ur_config::Config {
    let backup = ur_config::BackupConfig {
        path: None,
        interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
        enabled: true,
        retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
    };
    ur_config::Config {
        agent: ur_config::AgentType::Claude,
        runtime: ur_config::WorkerRuntime::Container,
        config_dir: dir.to_path_buf(),
        logs_dir: dir.join("logs"),
        workspace: workspace.to_path_buf(),
        server_port: ur_config::DEFAULT_SERVER_PORT,
        builderd_port: ur_config::DEFAULT_SERVER_PORT + 2,
        compose_file: dir.join("docker-compose.yml"),
        proxy: ur_config::ProxyConfig {
            hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
            allowlist: vec![],
        },
        network: test_network_config(),
        db: ur_config::DatabaseConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_DB_NAME.to_string(),
            bind_address: None,
        },
        ticket_db: ur_config::TicketDbConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_TICKET_DB_NAME.to_string(),
            bind_address: None,
            backup: backup.clone(),
        },
        workflow_db: ur_config::WorkflowDbConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_WORKFLOW_DB_NAME.to_string(),
            bind_address: None,
            backup,
        },
        worker_port: ur_config::DEFAULT_SERVER_PORT + 1,
        git_branch_prefix: String::new(),
        server: ur_config::ServerConfig {
            container_command: "docker".into(),
            stale_worker_ttl_days: 7,
            max_implement_cycles: Some(6u32),
            poll_interval_ms: 500,
            github_scan_interval_secs: 30,
            builderd_retry_count: ur_config::DEFAULT_BUILDERD_RETRY_COUNT,
            builderd_retry_backoff_ms: ur_config::DEFAULT_BUILDERD_RETRY_BACKOFF_MS,
            ui_event_fallback_interval_ms: ur_config::DEFAULT_UI_EVENT_FALLBACK_INTERVAL_MS,
        },
        projects: HashMap::new(),
        tui: ur_config::TuiConfig::default(),
        global_skills: ur_config::GlobalSkillsConfig::default(),
        workspace_brain_dir: None,
    }
}

fn test_network_config() -> ur_config::NetworkConfig {
    ur_config::NetworkConfig {
        name: ur_config::DEFAULT_NETWORK_NAME.to_string(),
        worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.to_string(),
        server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.to_string(),
        worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.to_string(),
    }
}

/// Build test components backed by the given database pools.
/// Returns (WorkerManager, WorkerRepo, CoreServiceHandler).
async fn make_components_with_db(
    dir: &Path,
    ticket_pool: &sqlx::PgPool,
    workflow_pool: &sqlx::PgPool,
) -> (
    ur_server::WorkerManager,
    workflow_db::WorkerRepo,
    ur_server::grpc::CoreServiceHandler,
) {
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let network_config = test_network_config();
    let config = test_config(dir, &workspace);
    let worker_repo = workflow_db::WorkerRepo::new(workflow_pool.clone());
    let graph_manager = ticket_db::GraphManager::new(ticket_pool.clone());
    let ticket_repo = ticket_db::TicketRepo::new(ticket_pool.clone(), graph_manager);
    let workflow_repo = workflow_db::WorkflowRepo::new(workflow_pool.clone());
    let channel = tonic::transport::Channel::from_static("http://localhost:12322").connect_lazy();
    let builder_container_client =
        ur_server::builder_container_client::BuilderContainerClient::new(channel.clone());
    let builder_pool_client = ur_server::BuilderPoolClient::new(channel);
    let network_manager = ur_server::network_manager::NetworkManager::new(
        builder_container_client.clone(),
        network_config.worker_name.clone(),
    );
    let project_registry = ur_server::ProjectRegistry::new(
        HashMap::new(),
        ur_server::hostexec::HostExecConfigManager::empty(),
    );
    let repo_pool_manager = ur_server::RepoPoolManager::new(
        &config,
        worker_repo.clone(),
        project_registry.clone(),
        builder_pool_client,
    );
    let worker_manager = ur_server::WorkerManager::new(
        workspace.clone(),
        workspace.clone(),
        workspace.join("logs"),
        workspace.join("logs"),
        workspace.join("logs"),
        network_manager,
        network_config.clone(),
        ur_config::DEFAULT_SERVER_PORT + 1,
        ur_server::worker::WorkerModesConfig::default(),
        worker_repo.clone(),
        ur_config::GlobalSkillsConfig::default(),
        builder_container_client,
        ur_server::builder_local_worker_client::BuilderLocalWorkerClient::new(
            tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy(),
        ),
    );
    let launch_manager = ur_server::grpc::LaunchManager {
        worker_manager: worker_manager.clone(),
        repo_pool_manager: repo_pool_manager.clone(),
        proxy_hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
        project_registry: project_registry.clone(),
        worker_repo: worker_repo.clone(),
        ticket_repo: ticket_repo.clone(),
        workflow_repo: workflow_repo.clone(),
        network_config: network_config.clone(),
        workspace_brain_dir: None,
        config_dir: workspace.clone(),
        worker_image_tag: ur_config::DEFAULT_IMAGE_TAG.to_owned(),
        workerd_resolver: ur_server::WorkerdAddressResolver::new(
            String::new(),
            ur_config::DEFAULT_WORKER_PREFIX.to_owned(),
        ),
    };
    let handler = ur_server::grpc::CoreServiceHandler {
        launch_manager,
        worker_manager: worker_manager.clone(),
        repo_pool_manager,
        workspace: workspace.clone(),
        proxy_hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
        project_registry,
        worker_repo: worker_repo.clone(),
        ticket_repo,
        workflow_repo,
        network_config,
        builderd_addr: format!("http://127.0.0.1:{}", ur_config::DEFAULT_SERVER_PORT + 2),
        config_dir: workspace,
        workerd_resolver: ur_server::WorkerdAddressResolver::new(
            String::new(),
            ur_config::DEFAULT_WORKER_PREFIX.to_owned(),
        ),
    };
    (worker_manager, worker_repo, handler)
}

/// Launch worker, kill server, delete slot directory from disk,
/// restart server — verify slot row is cleaned up and worker marked stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_cleans_up_deleted_slot_and_marks_worker_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let test_db = ur_db_test::TestDb::new().await;
    let ticket_pool = test_db.ticket_pool();
    let workflow_pool = test_db.workflow_pool();

    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    // --- Phase 1: "Original server" registers worker with a slot ---
    let (_pm1, worker_repo1, _handler1) =
        make_components_with_db(dir.path(), ticket_pool, workflow_pool).await;

    // Create pool directory structure with a slot directory on disk.
    let pool_dir = workspace.join("pool").join("test-proj");
    let slot_dir = pool_dir.join("0");
    std::fs::create_dir_all(&slot_dir).unwrap();

    let slot = workflow_db::model::Slot {
        id: "slot-deleted-1".to_owned(),
        project_key: "test-proj".to_owned(),
        slot_name: "0".to_owned(),
        host_path: slot_dir.display().to_string(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    worker_repo1.insert_slot(&slot).await.unwrap();

    let worker = workflow_db::model::Worker {
        worker_id: "worker-deleted-slot".to_owned(),
        process_id: "proc-deleted".to_owned(),
        project_key: "test-proj".to_owned(),
        container_id: "dead-container-xyz".to_owned(),
        worker_secret: "secret-deleted".to_owned(),
        strategy: "code".to_owned(),
        agent_type: "claude".to_owned(),
        container_status: "running".to_owned(),
        agent_status: "starting".to_owned(),
        workspace_path: Some(slot_dir.display().to_string()),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        idle_redispatch_count: 0,
        brain_path: None,
        runtime: workflow_db::model::WorkerRuntime::Container,
    };
    worker_repo1.insert_worker(&worker).await.unwrap();
    worker_repo1
        .link_worker_slot("worker-deleted-slot", "slot-deleted-1")
        .await
        .unwrap();

    // Verify both exist.
    assert!(
        worker_repo1
            .get_slot("slot-deleted-1")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        worker_repo1
            .get_worker("worker-deleted-slot")
            .await
            .unwrap()
            .is_some()
    );

    // --- Phase 2: "Kill server" + delete slot directory from disk ---
    std::fs::remove_dir_all(&slot_dir).unwrap();
    assert!(!slot_dir.exists());

    // --- Phase 3: "Server restart" — rebuild with same DBs, run reconciliation ---
    let (_pm2, worker_repo2, _handler2) =
        make_components_with_db(dir.path(), ticket_pool, workflow_pool).await;

    // Run slot reconciliation (as main.rs does on startup).
    let mut project_configs = HashMap::new();
    project_configs.insert("test-proj".to_owned(), pool_dir.clone());

    let slot_result = worker_repo2
        .reconcile_slots(&project_configs, &workspace, &workspace)
        .await
        .unwrap();

    // Slot row should be deleted (stale: DB row exists but dir is gone).
    assert_eq!(slot_result.deleted_stale, vec!["slot-deleted-1"]);
    assert!(slot_result.inserted_orphaned.is_empty());

    // Slot should be gone from DB.
    assert!(
        worker_repo2
            .get_slot("slot-deleted-1")
            .await
            .unwrap()
            .is_none()
    );

    // Worker referencing that slot should also be deleted by cascade in reconcile_slots.
    assert!(
        worker_repo2
            .get_worker("worker-deleted-slot")
            .await
            .unwrap()
            .is_none()
    );
}
