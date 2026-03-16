use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

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
async fn make_grpc_handler(
    dir: &Path,
) -> (
    ur_server::grpc::CoreServiceHandler,
    Arc<ur_server::RepoRegistry>,
) {
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let repo_registry = Arc::new(ur_server::RepoRegistry::new(workspace.clone()));
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
        projects: std::collections::HashMap::new(),
    };
    let repo_pool_manager = ur_server::RepoPoolManager::new(
        &config,
        workspace.clone(),
        workspace.clone(),
        ur_server::BuilderdClient::new(format!(
            "http://127.0.0.1:{}",
            ur_config::DEFAULT_DAEMON_PORT + 2
        )),
    );
    let db = ur_db::DatabaseManager::open(":memory:")
        .await
        .expect("failed to open in-memory db");
    let agent_repo = ur_db::AgentRepo::new(db.pool().clone());
    let process_manager = ur_server::ProcessManager::new(
        workspace.clone(),
        workspace.clone(),
        repo_registry.clone(),
        repo_pool_manager.clone(),
        network_manager,
        network_config,
        ur_config::DEFAULT_DAEMON_PORT + 1,
        ur_server::process::PromptModesConfig::default(),
        agent_repo,
    );
    let hostexec_config = ur_server::hostexec::HostExecConfigManager::load(
        Path::new("/nonexistent"),
        &ur_config::HostExecConfig::default(),
    )
    .unwrap();
    let handler = ur_server::grpc::CoreServiceHandler {
        process_manager,
        repo_pool_manager,
        repo_registry: repo_registry.clone(),
        workspace,
        proxy_hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
        projects: std::collections::HashMap::new(),
        hostexec_config,
        builderd_addr: format!("http://127.0.0.1:{}", ur_config::DEFAULT_DAEMON_PORT + 2),
    };
    (handler, repo_registry)
}

#[tokio::test]
async fn grpc_ping_over_tcp() {
    use ur_rpc::proto::core::PingRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let dir = tempfile::tempdir().unwrap();

    let (handler, _repo_registry) = make_grpc_handler(dir.path()).await;
    let (channel, _addr) = spawn_grpc_server(handler).await;

    let mut client = CoreServiceClient::new(channel);
    let resp = client.ping(PingRequest {}).await.unwrap();
    assert_eq!(resp.into_inner().message, "pong");
}
