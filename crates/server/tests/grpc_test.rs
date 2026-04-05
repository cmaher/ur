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
        db: ur_config::DatabaseConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_DB_NAME.to_string(),
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
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
            ui_event_fallback_interval_ms: ur_config::DEFAULT_UI_EVENT_FALLBACK_INTERVAL_MS,
        },
        projects: std::collections::HashMap::new(),
        tui: ur_config::TuiConfig::default(),
    };
    (config, network_config)
}

async fn make_grpc_handler(dir: &Path) -> ur_server::grpc::CoreServiceHandler {
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
    ur_server::grpc::CoreServiceHandler {
        worker_manager,
        repo_pool_manager,
        workspace,
        proxy_hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
        projects: std::collections::HashMap::new(),
        worker_repo,
        ticket_repo,
        workflow_repo,
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

/// Spawn a worker gRPC server (no auth interceptor) and return a connected channel.
async fn spawn_worker_grpc_server(
    handler: ur_server::grpc::WorkerCoreServiceHandler,
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

async fn make_worker_handler() -> (
    ur_server::grpc::WorkerCoreServiceHandler,
    ur_db::TicketRepo,
    ur_db::WorkflowRepo,
) {
    let db = ur_db::DatabaseManager::open(":memory:")
        .await
        .expect("failed to open in-memory db");
    let worker_repo = ur_db::WorkerRepo::new(db.pool().clone());
    let graph_manager = ur_db::GraphManager::new(db.pool().clone());
    let ticket_repo = ur_db::TicketRepo::new(db.pool().clone(), graph_manager);
    let workflow_repo = ur_db::WorkflowRepo::new(db.pool().clone());
    let (transition_tx, _transition_rx) = tokio::sync::mpsc::channel(16);

    let handler = ur_server::grpc::WorkerCoreServiceHandler {
        worker_repo,
        ticket_repo: ticket_repo.clone(),
        workflow_repo: workflow_repo.clone(),
        worker_prefix: "ur-worker-".to_string(),
        transition_tx,
    };
    (handler, ticket_repo, workflow_repo)
}

#[tokio::test]
async fn link_comment_ticket_writes_row() {
    use ur_db::model::{LifecycleStatus, NewTicket};
    use ur_rpc::proto::core::LinkCommentTicketRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let (handler, ticket_repo, workflow_repo) = make_worker_handler().await;
    let (channel, _addr) = spawn_worker_grpc_server(handler).await;
    let mut client = CoreServiceClient::new(channel);

    // Seed a ticket, workflow, and gh_repo metadata.
    let ticket_id = "ur-test1";
    let worker_id = "w-link1";
    ticket_repo
        .create_ticket(&NewTicket {
            id: Some(ticket_id.to_string()),
            project: "ur".to_string(),
            type_: "code".to_string(),
            priority: 0,
            parent_id: None,
            title: "test ticket".to_string(),
            body: String::new(),
            ..Default::default()
        })
        .await
        .unwrap();
    workflow_repo
        .create_workflow(ticket_id, LifecycleStatus::Implementing)
        .await
        .unwrap();
    workflow_repo
        .set_workflow_worker_id(ticket_id, worker_id)
        .await
        .unwrap();
    ticket_repo
        .set_meta(ticket_id, "ticket", "gh_repo", "owner/repo")
        .await
        .unwrap();

    // Create the feedback ticket (FK target for ticket_comments).
    ticket_repo
        .create_ticket(&NewTicket {
            id: Some("ur-feedback1".to_string()),
            project: "ur".to_string(),
            type_: "code".to_string(),
            priority: 0,
            parent_id: Some(ticket_id.to_string()),
            title: "feedback ticket".to_string(),
            body: String::new(),
            ..Default::default()
        })
        .await
        .unwrap();

    // Make the RPC call with worker-id header.
    let mut request = tonic::Request::new(LinkCommentTicketRequest {
        worker_id: worker_id.to_string(),
        pr_number: 42,
        comment_id: 12345,
        ticket_id: "ur-feedback1".to_string(),
    });
    request
        .metadata_mut()
        .insert(ur_config::WORKER_ID_HEADER, worker_id.parse().unwrap());

    client.link_comment_ticket(request).await.unwrap();

    // Verify the row was written.
    let pending = workflow_repo.get_pending_replies().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].comment_id, "12345");
    assert_eq!(pending[0].ticket_id, "ur-feedback1");
    assert_eq!(pending[0].pr_number, 42);
    assert_eq!(pending[0].gh_repo, "owner/repo");
    assert!(!pending[0].reply_posted);
}

#[tokio::test]
async fn link_comment_ticket_missing_gh_repo_returns_not_found() {
    use ur_db::model::{LifecycleStatus, NewTicket};
    use ur_rpc::proto::core::LinkCommentTicketRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let (handler, ticket_repo, workflow_repo) = make_worker_handler().await;
    let (channel, _addr) = spawn_worker_grpc_server(handler).await;
    let mut client = CoreServiceClient::new(channel);

    // Seed a ticket and workflow, but do NOT set gh_repo metadata.
    let ticket_id = "ur-test2";
    let worker_id = "w-link2";
    ticket_repo
        .create_ticket(&NewTicket {
            id: Some(ticket_id.to_string()),
            project: "ur".to_string(),
            type_: "code".to_string(),
            priority: 0,
            parent_id: None,
            title: "test ticket no repo".to_string(),
            body: String::new(),
            ..Default::default()
        })
        .await
        .unwrap();
    workflow_repo
        .create_workflow(ticket_id, LifecycleStatus::Implementing)
        .await
        .unwrap();
    workflow_repo
        .set_workflow_worker_id(ticket_id, worker_id)
        .await
        .unwrap();

    // Make the RPC call — should fail because gh_repo is missing.
    let mut request = tonic::Request::new(LinkCommentTicketRequest {
        worker_id: worker_id.to_string(),
        pr_number: 10,
        comment_id: 99,
        ticket_id: "ur-feedback2".to_string(),
    });
    request
        .metadata_mut()
        .insert(ur_config::WORKER_ID_HEADER, worker_id.parse().unwrap());

    let err = client.link_comment_ticket(request).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    assert!(err.message().contains("gh_repo"));
}
