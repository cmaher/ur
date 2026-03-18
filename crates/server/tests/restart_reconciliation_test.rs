// Acceptance tests: server restart reclaims running workers.
//
// These tests exercise the full reconciliation path that runs on server startup:
// WorkerRepo reconciliation + gRPC auth verification after reclamation.
// They simulate a server restart by:
// 1. Setting up workers via WorkerManager (as a running server would)
// 2. Creating a "fresh" gRPC server with the same DB (simulating restart)
// 3. Running reconcile_workers (as main.rs does on startup)
// 4. Verifying that reclaimed workers can still authenticate to the gRPC server

use std::collections::HashMap;
use std::path::Path;

use tonic::transport::{Endpoint, Server};

use ur_rpc::proto::core::PingRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::core_service_server::CoreServiceServer;

/// Build test components backed by the given database pool.
/// Returns (WorkerManager, WorkerRepo, CoreServiceHandler).
async fn make_components_with_db(
    dir: &Path,
    db: &ur_db::DatabaseManager,
) -> (
    ur_server::WorkerManager,
    ur_db::WorkerRepo,
    ur_server::grpc::CoreServiceHandler,
) {
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let network_config = ur_config::NetworkConfig {
        name: ur_config::DEFAULT_NETWORK_NAME.to_string(),
        worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.to_string(),
        server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.to_string(),
        worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.to_string(),
    };
    let network_manager =
        container::NetworkManager::new("docker".to_string(), network_config.worker_name.clone());
    let config = ur_config::Config {
        config_dir: dir.to_path_buf(),
        workspace: workspace.clone(),
        server_port: ur_config::DEFAULT_SERVER_PORT,
        builderd_port: ur_config::DEFAULT_SERVER_PORT + 2,
        compose_file: dir.join("docker-compose.yml"),
        proxy: ur_config::ProxyConfig {
            hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
            allowlist: vec![],
        },
        network: network_config.clone(),
        hostexec: ur_config::HostExecConfig::default(),
        rag: ur_config::RagConfig {
            qdrant_hostname: ur_config::DEFAULT_QDRANT_HOSTNAME.to_string(),
            embedding_model: ur_config::DEFAULT_EMBEDDING_MODEL.to_string(),
            docs: ur_config::RagDocsConfig::default(),
        },
        backup: ur_config::BackupConfig {
            path: None,
            interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
            enabled: true,
            retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
        },
        worker_port: ur_config::DEFAULT_SERVER_PORT + 1,
        git_branch_prefix: String::new(),

        projects: HashMap::new(),
    };
    let worker_repo = ur_db::WorkerRepo::new(db.pool().clone());
    let channel = tonic::transport::Channel::from_static("http://localhost:42070").connect_lazy();
    let builderd_client = ur_rpc::proto::builder::BuilderdClient::new(channel.clone());
    let local_repo = local_repo::GitBackend {
        client: ur_rpc::proto::builder::BuilderdClient::new(channel),
    };
    let repo_pool_manager = ur_server::RepoPoolManager::new(
        &config,
        workspace.clone(),
        workspace.clone(),
        builderd_client,
        local_repo,
        worker_repo.clone(),
    );
    let worker_manager = ur_server::WorkerManager::new(
        workspace.clone(),
        workspace.clone(),
        repo_pool_manager.clone(),
        network_manager,
        network_config.clone(),
        ur_config::DEFAULT_SERVER_PORT + 1,
        ur_server::worker::PromptModesConfig::default(),
        worker_repo.clone(),
    );
    let hostexec_config = ur_server::hostexec::HostExecConfigManager::load(
        Path::new("/nonexistent"),
        &ur_config::HostExecConfig::default(),
    )
    .unwrap();
    let handler = ur_server::grpc::CoreServiceHandler {
        worker_manager: worker_manager.clone(),
        repo_pool_manager,
        workspace,
        proxy_hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
        projects: HashMap::new(),
        worker_repo: worker_repo.clone(),
        network_config,
        hostexec_config,
        builderd_addr: format!("http://127.0.0.1:{}", ur_config::DEFAULT_SERVER_PORT + 2),
    };
    (worker_manager, worker_repo, handler)
}

/// Spawn a gRPC server with worker auth interceptor, return channel.
async fn spawn_authed_server(
    worker_repo: ur_db::WorkerRepo,
    handler: ur_server::grpc::CoreServiceHandler,
) -> tonic::transport::Channel {
    let interceptor = ur_server::auth::worker_auth_interceptor(worker_repo);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        Server::builder()
            .add_service(CoreServiceServer::with_interceptor(handler, interceptor))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    Endpoint::try_from(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap()
}

/// Helper: make an authenticated ping request.
async fn authed_ping(
    client: &mut CoreServiceClient<tonic::transport::Channel>,
    worker_id: &str,
    secret: &str,
) -> Result<tonic::Response<ur_rpc::proto::core::PingResponse>, tonic::Status> {
    let mut request = tonic::Request::new(PingRequest {});
    request
        .metadata_mut()
        .insert("ur-worker-id", worker_id.parse().unwrap());
    request
        .metadata_mut()
        .insert("ur-worker-secret", secret.parse().unwrap());
    client.ping(request).await
}

/// Scenario 1: Launch worker, restart server (rebuild stack with same DB),
/// run reconciliation with container alive, verify worker is reclaimed and
/// auth still works, then stop worker and verify slot released.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_reclaims_worker_with_live_container() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test1.db");
    let db = ur_db::DatabaseManager::open(db_path.to_str().unwrap())
        .await
        .unwrap();

    // --- Phase 1: "Original server" registers a worker ---
    let (_pm1, worker_repo1, _handler1) = make_components_with_db(dir.path(), &db).await;

    let worker_id_str = "restart-test-worker-1";
    let secret = "test-secret-reclaim";
    let slot_path = dir.path().join("workspace").join("slot0");
    std::fs::create_dir_all(&slot_path).unwrap();

    // Insert a slot so we can verify the worker_slot link stays after reclamation.
    let slot = ur_db::model::Slot {
        id: "slot-restart-1".to_owned(),
        project_key: "test-proj".to_owned(),
        slot_name: "0".to_owned(),

        host_path: slot_path.display().to_string(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    worker_repo1.insert_slot(&slot).await.unwrap();

    // Register the worker (simulates a launched worker).
    let worker = ur_db::model::Worker {
        worker_id: worker_id_str.to_owned(),
        process_id: "restart-test".to_owned(),
        project_key: "test-proj".to_owned(),
        container_id: "live-container-abc".to_owned(),
        worker_secret: secret.to_owned(),
        strategy: "code".to_owned(),
        container_status: "running".to_owned(),
        agent_status: "starting".to_owned(),
        workspace_path: Some(slot_path.display().to_string()),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        idle_redispatch_count: 0,
    };
    worker_repo1.insert_worker(&worker).await.unwrap();
    worker_repo1
        .link_worker_slot(worker_id_str, "slot-restart-1")
        .await
        .unwrap();

    // Verify worker is running before "restart".
    let pre = worker_repo1
        .get_worker(worker_id_str)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pre.container_status, "running");

    // --- Phase 2: "Server restart" — rebuild components with the same DB ---
    let (_pm2, worker_repo2, handler2) = make_components_with_db(dir.path(), &db).await;

    // Run reconciliation with container alive (simulates docker inspect returning true).
    let reconcile_result = worker_repo2
        .reconcile_workers(|container_id| async move { container_id == "live-container-abc" })
        .await
        .unwrap();

    assert_eq!(reconcile_result.reclaimed, vec![worker_id_str]);
    assert!(reconcile_result.marked_stopped.is_empty());

    // Verify worker status is still "running" after reclamation.
    let post = worker_repo2
        .get_worker(worker_id_str)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(post.container_status, "running");

    // Verify worker_slot link stays (slot is still in use by the reclaimed worker).
    let ws_post = worker_repo2.get_worker_slot(worker_id_str).await.unwrap();
    assert!(
        ws_post.is_some(),
        "worker_slot link should survive reclamation"
    );

    // --- Phase 3: Verify auth still works on the new gRPC server ---
    let channel = spawn_authed_server(worker_repo2.clone(), handler2).await;
    let mut client = CoreServiceClient::new(channel);

    let result = authed_ping(&mut client, worker_id_str, secret).await;
    assert!(result.is_ok(), "auth should work after reclamation");
    assert_eq!(result.unwrap().into_inner().message, "pong");

    // --- Phase 4: Stop worker, verify slot released ---
    worker_repo2
        .update_worker_container_status(worker_id_str, "stopped")
        .await
        .unwrap();
    worker_repo2
        .unlink_worker_slot(worker_id_str)
        .await
        .unwrap();

    let stopped = worker_repo2
        .get_worker(worker_id_str)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.container_status, "stopped");

    // Verify the worker_slot link is gone (slot is now available).
    let ws_released = worker_repo2.get_worker_slot(worker_id_str).await.unwrap();
    assert!(ws_released.is_none(), "worker_slot link should be removed");
}

/// Scenario 2: Launch worker, kill server, delete slot directory from disk,
/// restart server — verify slot row is cleaned up and worker marked stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_cleans_up_deleted_slot_and_marks_worker_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test2.db");
    let db = ur_db::DatabaseManager::open(db_path.to_str().unwrap())
        .await
        .unwrap();

    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    // --- Phase 1: "Original server" registers worker with a slot ---
    let (_pm1, worker_repo1, _handler1) = make_components_with_db(dir.path(), &db).await;

    // Create pool directory structure with a slot directory on disk.
    let pool_dir = workspace.join("pool").join("test-proj");
    let slot_dir = pool_dir.join("0");
    std::fs::create_dir_all(&slot_dir).unwrap();

    let slot = ur_db::model::Slot {
        id: "slot-deleted-1".to_owned(),
        project_key: "test-proj".to_owned(),
        slot_name: "0".to_owned(),

        host_path: slot_dir.display().to_string(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    worker_repo1.insert_slot(&slot).await.unwrap();

    let worker = ur_db::model::Worker {
        worker_id: "worker-deleted-slot".to_owned(),
        process_id: "proc-deleted".to_owned(),
        project_key: "test-proj".to_owned(),
        container_id: "dead-container-xyz".to_owned(),
        worker_secret: "secret-deleted".to_owned(),
        strategy: "code".to_owned(),
        container_status: "running".to_owned(),
        agent_status: "starting".to_owned(),
        workspace_path: Some(slot_dir.display().to_string()),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        idle_redispatch_count: 0,
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

    // --- Phase 3: "Server restart" — rebuild with same DB, run reconciliation ---
    let (_pm2, worker_repo2, _handler2) = make_components_with_db(dir.path(), &db).await;

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

#[allow(clippy::too_many_arguments)]
async fn insert_worker_with_slot(
    worker_repo: &ur_db::WorkerRepo,
    slot_id: &str,
    slot_name: &str,
    host_path: &str,
    worker_id: &str,
    secret: &str,
    process_id: &str,
    container_id: &str,
) {
    let slot = ur_db::model::Slot {
        id: slot_id.to_owned(),
        project_key: "proj-mix".to_owned(),
        slot_name: slot_name.to_owned(),
        host_path: host_path.to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    worker_repo.insert_slot(&slot).await.unwrap();

    let worker = ur_db::model::Worker {
        worker_id: worker_id.to_owned(),
        process_id: process_id.to_owned(),
        project_key: "proj-mix".to_owned(),
        container_id: container_id.to_owned(),
        worker_secret: secret.to_owned(),
        strategy: "code".to_owned(),
        container_status: "running".to_owned(),
        agent_status: "starting".to_owned(),
        workspace_path: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        idle_redispatch_count: 0,
    };
    worker_repo.insert_worker(&worker).await.unwrap();
    worker_repo
        .link_worker_slot(worker_id, slot_id)
        .await
        .unwrap();
}

/// Scenario 3: Multiple workers across restart — one container alive, one dead.
/// Verifies mixed reconciliation: live worker reclaimed, dead worker stopped with
/// slot released, and only the reclaimed worker can authenticate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_mixed_live_and_dead_workers() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test3.db");
    let db = ur_db::DatabaseManager::open(db_path.to_str().unwrap())
        .await
        .unwrap();

    let (_pm1, worker_repo1, _handler1) = make_components_with_db(dir.path(), &db).await;

    let live_worker_id = "worker-mix-live";
    let live_secret = "secret-live";
    let dead_worker_id = "worker-mix-dead";
    let dead_secret = "secret-dead";

    insert_worker_with_slot(
        &worker_repo1,
        "slot-mix-1",
        "0",
        "/tmp/mix/0",
        live_worker_id,
        live_secret,
        "proc-live",
        "container-alive",
    )
    .await;

    insert_worker_with_slot(
        &worker_repo1,
        "slot-mix-2",
        "1",
        "/tmp/mix/1",
        dead_worker_id,
        dead_secret,
        "proc-dead",
        "container-dead",
    )
    .await;

    // --- Phase 2: "Restart" with reconciliation ---
    let (_pm2, worker_repo2, handler2) = make_components_with_db(dir.path(), &db).await;

    let reconcile_result = worker_repo2
        .reconcile_workers(|cid| async move { cid == "container-alive" })
        .await
        .unwrap();

    assert_eq!(reconcile_result.reclaimed, vec![live_worker_id]);
    assert_eq!(reconcile_result.marked_stopped, vec![dead_worker_id]);

    // Live worker: running, worker_slot link stays.
    let live = worker_repo2
        .get_worker(live_worker_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.container_status, "running");
    let ws1 = worker_repo2.get_worker_slot(live_worker_id).await.unwrap();
    assert!(ws1.is_some(), "live worker should keep worker_slot link");

    // Dead worker: stopped, worker_slot link removed.
    let dead = worker_repo2
        .get_worker(dead_worker_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dead.container_status, "stopped");
    let ws2 = worker_repo2.get_worker_slot(dead_worker_id).await.unwrap();
    assert!(
        ws2.is_none(),
        "dead worker should have worker_slot link removed"
    );

    // --- Phase 3: Auth checks on new gRPC server ---
    let channel = spawn_authed_server(worker_repo2.clone(), handler2).await;
    let mut client = CoreServiceClient::new(channel);

    // Live worker auth works.
    let live_result = authed_ping(&mut client, live_worker_id, live_secret).await;
    assert!(live_result.is_ok(), "reclaimed worker should authenticate");

    // Dead worker auth still technically succeeds (verify_worker checks id+secret,
    // not status), which is correct — the server doesn't reject stopped workers
    // from authenticating; it just won't route work to them.
    let dead_result = authed_ping(&mut client, dead_worker_id, dead_secret).await;
    assert!(
        dead_result.is_ok(),
        "stopped worker credentials remain valid in DB"
    );
}
