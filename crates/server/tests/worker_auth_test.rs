use std::collections::HashMap;
use std::path::Path;

use tonic::transport::{Endpoint, Server};

use ur_rpc::proto::core::PingRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::core_service_server::CoreServiceServer;

fn make_test_config(dir: &Path, workspace: &Path) -> (ur_config::Config, ur_config::NetworkConfig) {
    let network_config = ur_config::NetworkConfig {
        name: ur_config::DEFAULT_NETWORK_NAME.to_string(),
        worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.to_string(),
        server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.to_string(),
        worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.to_string(),
    };
    let config = ur_config::Config {
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
        server: ur_config::ServerConfig {
            container_command: "docker".into(),
            stale_worker_ttl_days: 7,
            max_implement_cycles: Some(6),
            poll_interval_ms: 500,
            github_scan_interval_secs: 30,
            builderd_retry_count: ur_config::DEFAULT_BUILDERD_RETRY_COUNT,
            builderd_retry_backoff_ms: ur_config::DEFAULT_BUILDERD_RETRY_BACKOFF_MS,
            ui_event_poll_interval_ms: ur_config::DEFAULT_UI_EVENT_POLL_INTERVAL_MS,
        },
        projects: HashMap::new(),
        tui: ur_config::TuiConfig::default(),
    };
    (config, network_config)
}

/// Build a WorkerManager, WorkerRepo, and CoreServiceHandler for testing.
async fn make_test_components(
    dir: &Path,
) -> (
    ur_server::WorkerManager,
    ur_db::WorkerRepo,
    ur_server::grpc::CoreServiceHandler,
) {
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let (config, network_config) = make_test_config(dir, &workspace);
    let network_manager =
        container::NetworkManager::new("docker".to_string(), network_config.worker_name.clone());
    let db = ur_db::DatabaseManager::open(":memory:")
        .await
        .expect("failed to open in-memory db");
    let worker_repo = ur_db::WorkerRepo::new(db.pool().clone());
    let graph_manager = ur_db::GraphManager::new(db.pool().clone());
    let ticket_repo = ur_db::TicketRepo::new(db.pool().clone(), graph_manager);
    let workflow_repo = ur_db::WorkflowRepo::new(db.pool().clone());
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
        workspace.join("config"),
    );
    let worker_manager = ur_server::WorkerManager::new(
        workspace.clone(),
        workspace.clone(),
        workspace.join("logs"),
        workspace.join("logs"),
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
        ticket_repo,
        workflow_repo,
        network_config,
        hostexec_config,
        builderd_addr: format!("http://127.0.0.1:{}", ur_config::DEFAULT_SERVER_PORT + 2),
    };
    (worker_manager, worker_repo, handler)
}

/// Spawn a gRPC server with CoreService wrapped in the worker auth interceptor.
async fn spawn_authed_server(
    _worker_manager: ur_server::WorkerManager,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_server_rejects_requests_without_worker_headers() {
    let dir = tempfile::tempdir().unwrap();
    let (worker_manager, worker_repo, handler) = make_test_components(dir.path()).await;
    let channel = spawn_authed_server(worker_manager, worker_repo, handler).await;

    let mut client = CoreServiceClient::new(channel);
    let result = client.ping(PingRequest {}).await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert!(status.message().contains("missing ur-worker-id"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_server_rejects_requests_with_invalid_secret() {
    let dir = tempfile::tempdir().unwrap();
    let (worker_manager, worker_repo, handler) = make_test_components(dir.path()).await;

    // Register a real worker so the ID exists but use a different secret in the request
    let worker_id = worker_manager.generate_worker_id("authtest");
    let real_secret = "real-secret-value";
    worker_manager
        .register_worker(
            worker_id.clone(),
            "authtest".into(),
            String::new(),
            None,
            ur_server::WorkerStrategy::Code,
            "fake-cid".into(),
            real_secret.into(),
        )
        .await;

    let channel = spawn_authed_server(worker_manager, worker_repo, handler).await;

    let mut request = tonic::Request::new(PingRequest {});
    request
        .metadata_mut()
        .insert("ur-worker-id", worker_id.to_string().parse().unwrap());
    request
        .metadata_mut()
        .insert("ur-worker-secret", "wrong-secret".parse().unwrap());

    let mut client = CoreServiceClient::new(channel);
    let result = client.ping(request).await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert!(status.message().contains("worker authentication failed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_server_accepts_requests_with_valid_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let (worker_manager, worker_repo, handler) = make_test_components(dir.path()).await;

    let worker_id = worker_manager.generate_worker_id("validtest");
    let secret = "correct-secret-value";
    worker_manager
        .register_worker(
            worker_id.clone(),
            "validtest".into(),
            String::new(),
            None,
            ur_server::WorkerStrategy::Code,
            "fake-cid".into(),
            secret.into(),
        )
        .await;

    let channel = spawn_authed_server(worker_manager, worker_repo, handler).await;

    let mut request = tonic::Request::new(PingRequest {});
    request
        .metadata_mut()
        .insert("ur-worker-id", worker_id.to_string().parse().unwrap());
    request
        .metadata_mut()
        .insert("ur-worker-secret", secret.parse().unwrap());

    let mut client = CoreServiceClient::new(channel);
    let result = client.ping(request).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().into_inner().message, "pong");
}
