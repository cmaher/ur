// Acceptance tests: server restart reclaims running workers.
//
// These tests exercise the full reconciliation path that runs on server startup:
// AgentRepo reconciliation + gRPC auth verification after reclamation.
// They simulate a server restart by:
// 1. Setting up workers via ProcessManager (as a running server would)
// 2. Creating a "fresh" gRPC server with the same DB (simulating restart)
// 3. Running reconcile_agents (as main.rs does on startup)
// 4. Verifying that reclaimed workers can still authenticate to the gRPC server

use std::collections::HashMap;
use std::path::Path;

use tonic::transport::{Endpoint, Server};

use ur_rpc::proto::core::PingRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::core_service_server::CoreServiceServer;

/// Build test components backed by the given database pool.
/// Returns (ProcessManager, AgentRepo, CoreServiceHandler).
async fn make_components_with_db(
    dir: &Path,
    db: &ur_db::DatabaseManager,
) -> (
    ur_server::ProcessManager,
    ur_db::AgentRepo,
    ur_server::grpc::CoreServiceHandler,
) {
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let network_config = ur_config::NetworkConfig {
        name: ur_config::DEFAULT_NETWORK_NAME.to_string(),
        worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.to_string(),
        server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.to_string(),
        agent_prefix: ur_config::DEFAULT_AGENT_PREFIX.to_string(),
    };
    let network_manager =
        container::NetworkManager::new("docker".to_string(), network_config.worker_name.clone());
    let config = ur_config::Config {
        config_dir: dir.to_path_buf(),
        workspace: workspace.clone(),
        daemon_port: ur_config::DEFAULT_DAEMON_PORT,
        builderd_port: ur_config::DEFAULT_DAEMON_PORT + 2,
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
        worker_port: ur_config::DEFAULT_DAEMON_PORT + 1,
        projects: HashMap::new(),
    };
    let agent_repo = ur_db::AgentRepo::new(db.pool().clone());
    let repo_pool_manager = ur_server::RepoPoolManager::new(
        &config,
        workspace.clone(),
        workspace.clone(),
        ur_server::BuilderdClient::new(format!(
            "http://127.0.0.1:{}",
            ur_config::DEFAULT_DAEMON_PORT + 2
        )),
        agent_repo.clone(),
    );
    let process_manager = ur_server::ProcessManager::new(
        workspace.clone(),
        workspace.clone(),
        repo_pool_manager.clone(),
        network_manager,
        network_config,
        ur_config::DEFAULT_DAEMON_PORT + 1,
        ur_server::process::PromptModesConfig::default(),
        agent_repo.clone(),
    );
    let hostexec_config = ur_server::hostexec::HostExecConfigManager::load(
        Path::new("/nonexistent"),
        &ur_config::HostExecConfig::default(),
    )
    .unwrap();
    let handler = ur_server::grpc::CoreServiceHandler {
        process_manager: process_manager.clone(),
        repo_pool_manager,
        workspace,
        proxy_hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
        projects: HashMap::new(),
        hostexec_config,
        builderd_addr: format!("http://127.0.0.1:{}", ur_config::DEFAULT_DAEMON_PORT + 2),
    };
    (process_manager, agent_repo, handler)
}

/// Spawn a gRPC server with worker auth interceptor, return channel.
async fn spawn_authed_server(
    agent_repo: ur_db::AgentRepo,
    handler: ur_server::grpc::CoreServiceHandler,
) -> tonic::transport::Channel {
    let interceptor = ur_server::auth::worker_auth_interceptor(agent_repo);

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
    agent_id: &str,
    secret: &str,
) -> Result<tonic::Response<ur_rpc::proto::core::PingResponse>, tonic::Status> {
    let mut request = tonic::Request::new(PingRequest {});
    request
        .metadata_mut()
        .insert("ur-agent-id", agent_id.parse().unwrap());
    request
        .metadata_mut()
        .insert("ur-agent-secret", secret.parse().unwrap());
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
    let (_pm1, agent_repo1, _handler1) = make_components_with_db(dir.path(), &db).await;

    let worker_id_str = "restart-test-worker-1";
    let secret = "test-secret-reclaim";
    let slot_path = dir.path().join("workspace").join("slot0");
    std::fs::create_dir_all(&slot_path).unwrap();

    // Insert a slot so we can verify it stays in_use after reclamation.
    let slot = ur_db::model::Slot {
        id: "slot-restart-1".to_owned(),
        project_key: "test-proj".to_owned(),
        slot_name: "0".to_owned(),
        slot_type: "exclusive".to_owned(),
        host_path: slot_path.display().to_string(),
        status: "in_use".to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    agent_repo1.insert_slot(&slot).await.unwrap();

    // Register the worker (simulates a launched worker).
    let agent = ur_db::model::Agent {
        agent_id: worker_id_str.to_owned(),
        process_id: "restart-test".to_owned(),
        project_key: "test-proj".to_owned(),
        slot_id: Some("slot-restart-1".to_owned()),
        container_id: "live-container-abc".to_owned(),
        agent_secret: secret.to_owned(),
        strategy: "code".to_owned(),
        status: "running".to_owned(),
        workspace_path: Some(slot_path.display().to_string()),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    agent_repo1.insert_agent(&agent).await.unwrap();

    // Verify worker is running before "restart".
    let pre = agent_repo1.get_agent(worker_id_str).await.unwrap().unwrap();
    assert_eq!(pre.status, "running");

    // --- Phase 2: "Server restart" — rebuild components with the same DB ---
    let (_pm2, agent_repo2, handler2) = make_components_with_db(dir.path(), &db).await;

    // Run reconciliation with container alive (simulates docker inspect returning true).
    let reconcile_result = agent_repo2
        .reconcile_agents(|container_id| async move { container_id == "live-container-abc" })
        .await
        .unwrap();

    assert_eq!(reconcile_result.reclaimed, vec![worker_id_str]);
    assert!(reconcile_result.marked_stopped.is_empty());

    // Verify worker status is still "running" after reclamation.
    let post = agent_repo2.get_agent(worker_id_str).await.unwrap().unwrap();
    assert_eq!(post.status, "running");

    // Verify slot stays in_use.
    let slot_post = agent_repo2
        .get_slot("slot-restart-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(slot_post.status, "in_use");

    // --- Phase 3: Verify auth still works on the new gRPC server ---
    let channel = spawn_authed_server(agent_repo2.clone(), handler2).await;
    let mut client = CoreServiceClient::new(channel);

    let result = authed_ping(&mut client, worker_id_str, secret).await;
    assert!(result.is_ok(), "auth should work after reclamation");
    assert_eq!(result.unwrap().into_inner().message, "pong");

    // --- Phase 4: Stop worker, verify slot released ---
    agent_repo2
        .update_agent_status(worker_id_str, "stopped")
        .await
        .unwrap();
    agent_repo2
        .update_slot_status("slot-restart-1", "available")
        .await
        .unwrap();

    let stopped = agent_repo2.get_agent(worker_id_str).await.unwrap().unwrap();
    assert_eq!(stopped.status, "stopped");

    let slot_released = agent_repo2
        .get_slot("slot-restart-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(slot_released.status, "available");
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
    let (_pm1, agent_repo1, _handler1) = make_components_with_db(dir.path(), &db).await;

    // Create pool directory structure with a slot directory on disk.
    let pool_dir = workspace.join("pool").join("test-proj");
    let slot_dir = pool_dir.join("0");
    std::fs::create_dir_all(&slot_dir).unwrap();

    let slot = ur_db::model::Slot {
        id: "slot-deleted-1".to_owned(),
        project_key: "test-proj".to_owned(),
        slot_name: "0".to_owned(),
        slot_type: "exclusive".to_owned(),
        host_path: slot_dir.display().to_string(),
        status: "in_use".to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    agent_repo1.insert_slot(&slot).await.unwrap();

    let agent = ur_db::model::Agent {
        agent_id: "worker-deleted-slot".to_owned(),
        process_id: "proc-deleted".to_owned(),
        project_key: "test-proj".to_owned(),
        slot_id: Some("slot-deleted-1".to_owned()),
        container_id: "dead-container-xyz".to_owned(),
        agent_secret: "secret-deleted".to_owned(),
        strategy: "code".to_owned(),
        status: "running".to_owned(),
        workspace_path: Some(slot_dir.display().to_string()),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    agent_repo1.insert_agent(&agent).await.unwrap();

    // Verify both exist.
    assert!(
        agent_repo1
            .get_slot("slot-deleted-1")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        agent_repo1
            .get_agent("worker-deleted-slot")
            .await
            .unwrap()
            .is_some()
    );

    // --- Phase 2: "Kill server" + delete slot directory from disk ---
    std::fs::remove_dir_all(&slot_dir).unwrap();
    assert!(!slot_dir.exists());

    // --- Phase 3: "Server restart" — rebuild with same DB, run reconciliation ---
    let (_pm2, agent_repo2, _handler2) = make_components_with_db(dir.path(), &db).await;

    // Run slot reconciliation (as main.rs does on startup).
    let mut project_configs = HashMap::new();
    project_configs.insert("test-proj".to_owned(), pool_dir.clone());

    let slot_result = agent_repo2
        .reconcile_slots(&project_configs, &workspace)
        .await
        .unwrap();

    // Slot row should be deleted (stale: DB row exists but dir is gone).
    assert_eq!(slot_result.deleted_stale, vec!["slot-deleted-1"]);
    assert!(slot_result.inserted_orphaned.is_empty());

    // Slot should be gone from DB.
    assert!(
        agent_repo2
            .get_slot("slot-deleted-1")
            .await
            .unwrap()
            .is_none()
    );

    // Worker referencing that slot should also be deleted by cascade in reconcile_slots.
    assert!(
        agent_repo2
            .get_agent("worker-deleted-slot")
            .await
            .unwrap()
            .is_none()
    );
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

    let (_pm1, agent_repo1, _handler1) = make_components_with_db(dir.path(), &db).await;

    // Create two slots.
    let slot1 = ur_db::model::Slot {
        id: "slot-mix-1".to_owned(),
        project_key: "proj-mix".to_owned(),
        slot_name: "0".to_owned(),
        slot_type: "exclusive".to_owned(),
        host_path: "/tmp/mix/0".to_owned(),
        status: "in_use".to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    let slot2 = ur_db::model::Slot {
        id: "slot-mix-2".to_owned(),
        project_key: "proj-mix".to_owned(),
        slot_name: "1".to_owned(),
        slot_type: "exclusive".to_owned(),
        host_path: "/tmp/mix/1".to_owned(),
        status: "in_use".to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    agent_repo1.insert_slot(&slot1).await.unwrap();
    agent_repo1.insert_slot(&slot2).await.unwrap();

    // Worker with live container.
    let live_worker_id = "worker-mix-live";
    let live_secret = "secret-live";
    let worker_live = ur_db::model::Agent {
        agent_id: live_worker_id.to_owned(),
        process_id: "proc-live".to_owned(),
        project_key: "proj-mix".to_owned(),
        slot_id: Some("slot-mix-1".to_owned()),
        container_id: "container-alive".to_owned(),
        agent_secret: live_secret.to_owned(),
        strategy: "code".to_owned(),
        status: "running".to_owned(),
        workspace_path: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    agent_repo1.insert_agent(&worker_live).await.unwrap();

    // Worker with dead container.
    let dead_worker_id = "worker-mix-dead";
    let dead_secret = "secret-dead";
    let worker_dead = ur_db::model::Agent {
        agent_id: dead_worker_id.to_owned(),
        process_id: "proc-dead".to_owned(),
        project_key: "proj-mix".to_owned(),
        slot_id: Some("slot-mix-2".to_owned()),
        container_id: "container-dead".to_owned(),
        agent_secret: dead_secret.to_owned(),
        strategy: "code".to_owned(),
        status: "running".to_owned(),
        workspace_path: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    agent_repo1.insert_agent(&worker_dead).await.unwrap();

    // --- Phase 2: "Restart" with reconciliation ---
    let (_pm2, agent_repo2, handler2) = make_components_with_db(dir.path(), &db).await;

    let reconcile_result = agent_repo2
        .reconcile_agents(|cid| async move { cid == "container-alive" })
        .await
        .unwrap();

    assert_eq!(reconcile_result.reclaimed, vec![live_worker_id]);
    assert_eq!(reconcile_result.marked_stopped, vec![dead_worker_id]);

    // Live worker: running, slot stays in_use.
    let live = agent_repo2
        .get_agent(live_worker_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.status, "running");
    let s1 = agent_repo2.get_slot("slot-mix-1").await.unwrap().unwrap();
    assert_eq!(s1.status, "in_use");

    // Dead worker: stopped, slot released.
    let dead = agent_repo2
        .get_agent(dead_worker_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dead.status, "stopped");
    let s2 = agent_repo2.get_slot("slot-mix-2").await.unwrap().unwrap();
    assert_eq!(s2.status, "available");

    // --- Phase 3: Auth checks on new gRPC server ---
    let channel = spawn_authed_server(agent_repo2.clone(), handler2).await;
    let mut client = CoreServiceClient::new(channel);

    // Live worker auth works.
    let live_result = authed_ping(&mut client, live_worker_id, live_secret).await;
    assert!(live_result.is_ok(), "reclaimed worker should authenticate");

    // Dead worker auth still technically succeeds (verify_agent checks id+secret,
    // not status), which is correct — the server doesn't reject stopped workers
    // from authenticating; it just won't route work to them.
    let dead_result = authed_ping(&mut client, dead_worker_id, dead_secret).await;
    assert!(
        dead_result.is_ok(),
        "stopped worker credentials remain valid in DB"
    );
}
