use std::net::SocketAddr;
use std::path::Path;

use tonic::transport::{Endpoint, Server};

/// Helper: start a gRPC server on TCP and return a connected channel.
async fn spawn_grpc_server(
    handler: ur_server::grpc::CoreServiceHandler,
) -> (tonic::transport::Channel, SocketAddr) {
    use ur_rpc::proto::core::core_service_server::CoreServiceServer;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        Server::builder()
            .add_service(CoreServiceServer::new(handler))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let channel = Endpoint::try_from(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    (channel, addr)
}

/// Helper: create a CoreServiceHandler from a temp dir with workspace.
async fn make_grpc_handler(dir: &Path) -> ur_server::grpc::CoreServiceHandler {
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
        server: ur_config::ServerConfig {
            container_command: "docker".into(),
            stale_worker_ttl_days: 7,
            max_implement_cycles: Some(6),
            poll_interval_ms: 500,
            github_scan_interval_secs: 30,
            builderd_retry_count: ur_config::DEFAULT_BUILDERD_RETRY_COUNT,
            builderd_retry_backoff_ms: ur_config::DEFAULT_BUILDERD_RETRY_BACKOFF_MS,
        },
        projects: std::collections::HashMap::new(),
    };
    let db = ur_db::DatabaseManager::open(":memory:")
        .await
        .expect("failed to open in-memory db");
    let worker_repo = ur_db::WorkerRepo::new(db.pool().clone());
    let graph_manager = ur_db::GraphManager::new(db.pool().clone());
    let ticket_repo = ur_db::TicketRepo::new(db.pool().clone(), graph_manager);
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
    ur_server::grpc::CoreServiceHandler {
        worker_manager,
        repo_pool_manager,
        workspace,
        proxy_hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
        projects: std::collections::HashMap::new(),
        worker_repo,
        ticket_repo,
        network_config,
        hostexec_config,
        builderd_addr: format!("http://127.0.0.1:{}", ur_config::DEFAULT_SERVER_PORT + 2),
    }
}

#[tokio::test]
async fn grpc_ping_over_tcp() {
    use ur_rpc::proto::core::PingRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let dir = tempfile::tempdir().unwrap();

    let handler = make_grpc_handler(dir.path()).await;
    let (channel, _addr) = spawn_grpc_server(handler).await;

    let mut client = CoreServiceClient::new(channel);
    let resp = client.ping(PingRequest {}).await.unwrap();
    assert_eq!(resp.into_inner().message, "pong");
}
