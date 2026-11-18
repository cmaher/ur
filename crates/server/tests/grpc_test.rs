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
    let backup = ur_config::BackupConfig {
        path: None,
        interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
        enabled: true,
        retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
    };
    let config = ur_config::Config {
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
        network: network_config.clone(),
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
        projects: std::collections::HashMap::new(),
        tui: ur_config::TuiConfig::default(),
        global_skills: ur_config::GlobalSkillsConfig::default(),
        workspace_brain_dir: None,
    };
    (config, network_config)
}

async fn make_grpc_handler(
    dir: &Path,
) -> (ur_server::grpc::CoreServiceHandler, ur_db_test::TestDb) {
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let (config, network_config) = make_test_config(dir, &workspace);
    let test_db = ur_db_test::TestDb::new().await;
    let ticket_pool = test_db.ticket_pool().clone();
    let workflow_pool = test_db.workflow_pool().clone();
    let worker_repo = workflow_db::WorkerRepo::new(workflow_pool.clone());
    let graph_manager = ticket_db::GraphManager::new(ticket_pool.clone());
    let ticket_repo = ticket_db::TicketRepo::new(ticket_pool, graph_manager);
    let workflow_repo = workflow_db::WorkflowRepo::new(workflow_pool);
    let channel = tonic::transport::Channel::from_static("http://localhost:12322").connect_lazy();
    let builder_container_client =
        ur_server::builder_container_client::BuilderContainerClient::new(channel.clone());
    let builder_pool_client = ur_server::BuilderPoolClient::new(channel);
    let network_manager = ur_server::network_manager::NetworkManager::new(
        builder_container_client.clone(),
        network_config.worker_name.clone(),
    );
    let project_registry = ur_server::ProjectRegistry::new(
        std::collections::HashMap::new(),
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
        worker_manager,
        repo_pool_manager,
        workspace: workspace.clone(),
        proxy_hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
        project_registry,
        worker_repo,
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
    (handler, test_db)
}

#[tokio::test]
async fn grpc_ping_over_tcp() {
    use ur_rpc::proto::core::PingRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let dir = tempfile::tempdir().unwrap();

    let (handler, _test_db) = make_grpc_handler(dir.path()).await;
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
    ticket_db::TicketRepo,
    workflow_db::WorkflowRepo,
    ur_db_test::TestDb,
) {
    let test_db = ur_db_test::TestDb::new().await;
    let ticket_pool = test_db.ticket_pool().clone();
    let workflow_pool = test_db.workflow_pool().clone();
    let worker_repo = workflow_db::WorkerRepo::new(workflow_pool.clone());
    let graph_manager = ticket_db::GraphManager::new(ticket_pool.clone());
    let ticket_repo = ticket_db::TicketRepo::new(ticket_pool, graph_manager);
    let workflow_repo = workflow_db::WorkflowRepo::new(workflow_pool);
    let (transition_tx, _transition_rx) = tokio::sync::mpsc::channel(16);

    let workspace = std::path::PathBuf::from("/tmp/test-worker-handler");
    let channel = tonic::transport::Channel::from_static("http://localhost:12322").connect_lazy();
    let builder_container_client =
        ur_server::builder_container_client::BuilderContainerClient::new(channel.clone());
    let builder_pool_client = ur_server::BuilderPoolClient::new(channel);
    let project_registry = ur_server::ProjectRegistry::new(
        std::collections::HashMap::new(),
        ur_server::hostexec::HostExecConfigManager::empty(),
    );
    let network_config = ur_config::NetworkConfig {
        name: ur_config::DEFAULT_NETWORK_NAME.into(),
        worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
        server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
        worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.into(),
    };
    let (config, _network_config) = make_test_config(&workspace, &workspace);
    let repo_pool_manager = ur_server::RepoPoolManager::new(
        &config,
        worker_repo.clone(),
        project_registry.clone(),
        builder_pool_client,
    );
    let network_manager = ur_server::network_manager::NetworkManager::new(
        builder_container_client.clone(),
        network_config.worker_name.clone(),
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
        worker_manager,
        repo_pool_manager,
        proxy_hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
        project_registry,
        worker_repo: worker_repo.clone(),
        ticket_repo: ticket_repo.clone(),
        workflow_repo: workflow_repo.clone(),
        network_config,
        workspace_brain_dir: None,
        config_dir: workspace.clone(),
        worker_image_tag: ur_config::DEFAULT_IMAGE_TAG.to_owned(),
        workerd_resolver: ur_server::WorkerdAddressResolver::new(
            String::new(),
            ur_config::DEFAULT_WORKER_PREFIX.to_owned(),
        ),
    };

    let handler = ur_server::grpc::WorkerCoreServiceHandler {
        worker_repo,
        ticket_repo: ticket_repo.clone(),
        workflow_repo: workflow_repo.clone(),
        worker_prefix: "ur-worker-".to_string(),
        transition_tx,
        launch_manager,
    };
    (handler, ticket_repo, workflow_repo, test_db)
}

#[tokio::test]
async fn link_comment_ticket_writes_row() {
    use ticket_db::{LifecycleStatus, NewTicket};
    use ur_rpc::proto::core::LinkCommentTicketRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let (handler, ticket_repo, workflow_repo, _test_db) = make_worker_handler().await;
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
    let pending = ticket_repo.get_pending_replies().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].comment_id, "12345");
    assert_eq!(pending[0].ticket_id, "ur-feedback1");
    assert_eq!(pending[0].pr_number, 42);
    assert_eq!(pending[0].gh_repo, "owner/repo");
    assert!(!pending[0].reply_posted);
}

#[tokio::test]
async fn link_comment_ticket_missing_gh_repo_returns_not_found() {
    use ticket_db::{LifecycleStatus, NewTicket};
    use ur_rpc::proto::core::LinkCommentTicketRequest;
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;

    let (handler, ticket_repo, workflow_repo, _test_db) = make_worker_handler().await;
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

/// A worker row for `process_id`, created at `created_at`.
fn worker_row(
    worker_id: &str,
    process_id: &str,
    created_at: &str,
    agent_status: &str,
    runtime: workflow_db::model::WorkerRuntime,
) -> workflow_db::model::Worker {
    workflow_db::model::Worker {
        worker_id: worker_id.into(),
        process_id: process_id.into(),
        project_key: String::new(),
        container_id: "cid".into(),
        worker_secret: "secret".into(),
        strategy: "code".into(),
        agent_type: ur_rpc::agent::CLAUDE.into(),
        container_status: "running".into(),
        agent_status: agent_status.into(),
        workspace_path: Some(format!("/ws/{worker_id}")),
        created_at: created_at.into(),
        updated_at: created_at.into(),
        idle_redispatch_count: 0,
        brain_path: None,
        runtime,
    }
}

async fn worker_info_for(
    handler: &ur_server::grpc::CoreServiceHandler,
    id: &str,
) -> ur_rpc::proto::core::WorkerInfoResponse {
    use ur_rpc::proto::core::core_service_server::CoreService;
    handler
        .worker_info(tonic::Request::new(
            ur_rpc::proto::core::WorkerInfoRequest {
                worker_id: id.into(),
            },
        ))
        .await
        .unwrap()
        .into_inner()
}

/// A Local worker row comes back as `runtime: "local"` from the list RPC and
/// from the info RPC, whether it is asked by process id (as the CLI does) or
/// by full worker id.
#[tokio::test]
async fn worker_list_and_info_return_local_runtime() {
    use ur_rpc::proto::core::WorkerListRequest;
    use ur_rpc::proto::core::core_service_server::CoreService;

    let dir = tempfile::tempdir().unwrap();
    let (handler, _test_db) = make_grpc_handler(dir.path()).await;
    let now = chrono::Utc::now().to_rfc3339();
    handler
        .worker_repo
        .insert_worker(&worker_row(
            "local-w1-abcd",
            "local-w1",
            &now,
            "idle",
            workflow_db::model::WorkerRuntime::Local,
        ))
        .await
        .unwrap();

    let list = handler
        .worker_list(tonic::Request::new(WorkerListRequest {}))
        .await
        .unwrap()
        .into_inner();
    let summary = list
        .workers
        .iter()
        .find(|w| w.worker_id == "local-w1")
        .expect("worker listed");
    assert_eq!(summary.runtime, ur_rpc::runtime::LOCAL);

    for id in ["local-w1", "local-w1-abcd"] {
        let info = worker_info_for(&handler, id).await;
        assert_eq!(info.runtime, ur_rpc::runtime::LOCAL, "by {id}");
        assert_eq!(info.agent_status, "idle", "by {id}");
        assert_eq!(info.workspace_dir, "/ws/local-w1-abcd", "by {id}");
    }
}

/// A process id names a slot every worker that occupied it shares, so the
/// info RPC answers for the newest active row, not a stale older one.
#[tokio::test]
async fn worker_info_by_process_id_prefers_newest_row() {
    let dir = tempfile::tempdir().unwrap();
    let (handler, _test_db) = make_grpc_handler(dir.path()).await;
    let older = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let newer = chrono::Utc::now().to_rfc3339();
    // Insert the newer row first so insertion order cannot stand in for age.
    for row in [
        worker_row(
            "w1-neww",
            "w1",
            &newer,
            "working",
            workflow_db::model::WorkerRuntime::Local,
        ),
        worker_row(
            "w1-oldd",
            "w1",
            &older,
            "idle",
            workflow_db::model::WorkerRuntime::Container,
        ),
    ] {
        handler.worker_repo.insert_worker(&row).await.unwrap();
    }

    let info = worker_info_for(&handler, "w1").await;
    assert_eq!(info.agent_status, "working");
    assert_eq!(info.runtime, ur_rpc::runtime::LOCAL);
    assert_eq!(info.workspace_dir, "/ws/w1-neww");
}

/// Records what a fake worker daemon was asked to do.
#[derive(Clone, Default)]
struct FakeWorkerd {
    keys: std::sync::Arc<std::sync::Mutex<Vec<i32>>>,
    messages: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    interrupts: std::sync::Arc<std::sync::Mutex<u32>>,
    routed_ids: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl FakeWorkerd {
    fn record<T>(&self, req: &tonic::Request<T>) {
        let id = req
            .metadata()
            .get(ur_config::WORKER_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        self.routed_ids.lock().unwrap().push(id);
    }
}

/// The fake's impl: the listed methods answer Unimplemented, the rest are
/// written out. (`async_trait` does not expand macros inside an impl, so the
/// whole impl is generated.)
macro_rules! fake_workerd_impl {
    (unimplemented { $($method:ident($req:ident) -> $resp:ident;)* } $($custom:tt)*) => {
        #[tonic::async_trait]
        impl ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonService
            for FakeWorkerd
        {
            $(
                async fn $method(
                    &self,
                    _r: tonic::Request<ur_rpc::proto::workerd::$req>,
                ) -> Result<tonic::Response<ur_rpc::proto::workerd::$resp>, tonic::Status> {
                    Err(tonic::Status::unimplemented("fake"))
                }
            )*
            $($custom)*
        }
    };
}

fake_workerd_impl! {
    unimplemented {
        notify_idle(NotifyIdleRequest) -> NotifyIdleResponse;
        implement(ImplementRequest) -> ImplementResponse;
        design(DesignRequest) -> DesignResponse;
        address_feedback_tickets(AddressFeedbackRequest) -> AddressFeedbackResponse;
        step_complete(StepCompleteRequest) -> StepCompleteResponse;
        pause_nudge(PauseNudgeRequest) -> PauseNudgeResponse;
        set_ticket(SetTicketRequest) -> SetTicketResponse;
        dispatch_ticket(DispatchTicketRequest) -> DispatchTicketResponse;
        set_status_left(SetStatusLeftRequest) -> SetStatusLeftResponse;
    }

    async fn capture_pane(
        &self,
        r: tonic::Request<ur_rpc::proto::workerd::CapturePaneRequest>,
    ) -> Result<tonic::Response<ur_rpc::proto::workerd::CapturePaneResponse>, tonic::Status> {
        self.record(&r);
        Ok(tonic::Response::new(
            ur_rpc::proto::workerd::CapturePaneResponse {
                text: format!("screen+{}", r.into_inner().lines),
            },
        ))
    }

    async fn read_transcript(
        &self,
        _r: tonic::Request<ur_rpc::proto::workerd::ReadTranscriptRequest>,
    ) -> Result<tonic::Response<ur_rpc::proto::workerd::ReadTranscriptResponse>, tonic::Status>
    {
        Err(tonic::Status::failed_precondition(
            "agent 'agy' has no readable session log; use `ur worker read --screen`",
        ))
    }

    async fn send_message(
        &self,
        r: tonic::Request<ur_rpc::proto::workerd::SendMessageRequest>,
    ) -> Result<tonic::Response<ur_rpc::proto::workerd::SendMessageResponse>, tonic::Status> {
        self.record(&r);
        self.messages.lock().unwrap().push(r.into_inner().message);
        Ok(tonic::Response::new(
            ur_rpc::proto::workerd::SendMessageResponse {
                success: true,
                error: String::new(),
            },
        ))
    }

    async fn interrupt(
        &self,
        r: tonic::Request<ur_rpc::proto::workerd::InterruptRequest>,
    ) -> Result<tonic::Response<ur_rpc::proto::workerd::InterruptResponse>, tonic::Status> {
        self.record(&r);
        *self.interrupts.lock().unwrap() += 1;
        Ok(tonic::Response::new(ur_rpc::proto::workerd::InterruptResponse {}))
    }

    async fn send_keys(
        &self,
        r: tonic::Request<ur_rpc::proto::workerd::SendKeysRequest>,
    ) -> Result<tonic::Response<ur_rpc::proto::workerd::SendKeysResponse>, tonic::Status> {
        self.keys.lock().unwrap().extend(r.into_inner().keys);
        Ok(tonic::Response::new(ur_rpc::proto::workerd::SendKeysResponse {}))
    }

    async fn wait_idle(
        &self,
        r: tonic::Request<ur_rpc::proto::workerd::WaitIdleRequest>,
    ) -> Result<tonic::Response<ur_rpc::proto::workerd::WaitIdleResponse>, tonic::Status> {
        let req = r.into_inner();
        Ok(tonic::Response::new(
            ur_rpc::proto::workerd::WaitIdleResponse {
                timed_out: true,
                screen_tail: format!("{}s/{} lines", req.timeout_secs, req.screen_lines),
            },
        ))
    }
}

/// Serve `fake` and return a resolver that routes local workers to it (as
/// builderd would).
async fn fake_workerd_resolver(fake: &FakeWorkerd) -> ur_server::WorkerdAddressResolver {
    use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonServiceServer;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake_addr = listener.local_addr().unwrap();
    let svc = fake.clone();
    tokio::spawn(async move {
        Server::builder()
            .add_service(WorkerDaemonServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    ur_server::WorkerdAddressResolver::new(
        format!("http://{fake_addr}"),
        ur_config::DEFAULT_WORKER_PREFIX.to_owned(),
    )
}

/// A host handler whose builderd address is a [`FakeWorkerd`], with one
/// running local worker `ur-sc1` (full id `ur-sc1-x1`).
async fn handler_with_local_worker(
    dir: &Path,
    fake: &FakeWorkerd,
) -> (ur_server::grpc::CoreServiceHandler, ur_db_test::TestDb) {
    let (mut handler, test_db) = make_grpc_handler(dir).await;
    handler.workerd_resolver = fake_workerd_resolver(fake).await;
    handler
        .worker_repo
        .insert_worker(&workflow_db::model::Worker {
            worker_id: "ur-sc1-x1".into(),
            process_id: "ur-sc1".into(),
            project_key: "ur".into(),
            container_id: "ur-sc1-x1".into(),
            worker_secret: "secret".into(),
            strategy: "code".into(),
            agent_type: "claude".into(),
            container_status: "running".into(),
            agent_status: "idle".into(),
            workspace_path: None,
            created_at: String::new(),
            updated_at: String::new(),
            idle_redispatch_count: 0,
            brain_path: None,
            runtime: ur_config::WorkerRuntime::Local,
        })
        .await
        .unwrap();
    (handler, test_db)
}

/// Session-control RPCs reach a running local worker's daemon (through the
/// builderd address with the routing header), keep the daemon's status code,
/// and apply server defaults; unknown workers and bad input are rejected.
#[tokio::test]
async fn session_control_routes_to_worker_daemon() {
    use ur_rpc::proto::core::core_service_client::CoreServiceClient;
    use ur_rpc::proto::core::{
        CaptureWorkerPaneRequest, ReadWorkerTranscriptRequest, SendWorkerKeysRequest,
        WaitWorkerIdleRequest, WorkerKey,
    };

    let fake = FakeWorkerd::default();
    let dir = tempfile::tempdir().unwrap();
    let (handler, _test_db) = handler_with_local_worker(dir.path(), &fake).await;
    let (channel, _addr) = spawn_grpc_server(handler).await;
    let mut client = CoreServiceClient::new(channel);

    let screen = client
        .capture_worker_pane(CaptureWorkerPaneRequest {
            worker_id: "ur-sc1".into(),
            lines: 7,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(screen.text, "screen+7");
    assert_eq!(*fake.routed_ids.lock().unwrap(), ["ur-sc1-x1"]);

    let err = client
        .read_worker_transcript(ReadWorkerTranscriptRequest {
            worker_id: "ur-sc1".into(),
            turns: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(err.message().contains("ur-sc1"), "{}", err.message());
    assert!(err.message().contains("--screen"), "{}", err.message());

    client
        .send_worker_keys(SendWorkerKeysRequest {
            worker_id: "ur-sc1".into(),
            keys: vec![WorkerKey::Digit2 as i32, WorkerKey::Enter as i32],
        })
        .await
        .unwrap();
    assert_eq!(
        *fake.keys.lock().unwrap(),
        [WorkerKey::Digit2 as i32, WorkerKey::Enter as i32]
    );
    let bad_key = client
        .send_worker_keys(SendWorkerKeysRequest {
            worker_id: "ur-sc1".into(),
            keys: vec![999],
        })
        .await
        .unwrap_err();
    assert_eq!(bad_key.code(), tonic::Code::InvalidArgument);

    let wait = client
        .wait_worker_idle(WaitWorkerIdleRequest {
            worker_id: "ur-sc1".into(),
            timeout_secs: 0,
            screen_lines: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(wait.timed_out);
    assert_eq!(wait.screen_tail, "600s/40 lines");

    let missing = client
        .capture_worker_pane(CaptureWorkerPaneRequest {
            worker_id: "ur-nope".into(),
            lines: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(missing.code(), tonic::Code::NotFound);
}

// ---- Worker-port session control (policy: docs/plans/ur-o15mo-session-control.md) ----

/// Callers and targets seeded by [`worker_port_with_peers`]. Every row is a
/// running local worker, so its daemon is the fake behind builderd's address.
const MANUAL: &str = "ur-man1-a1";
const CODE: &str = "ur-cod1-a1";
const DESIGN: &str = "ur-des1-a1";
const PEER: &str = "ur-tgt1";
const PEER_FULL: &str = "ur-tgt1-a1";
const OTHER_PROJECT: &str = "ox-tgt2";

fn peer_row(worker_id: &str, project_key: &str, strategy: &str) -> workflow_db::model::Worker {
    let process_id = worker_id.rsplit_once('-').unwrap().0;
    workflow_db::model::Worker {
        worker_id: worker_id.into(),
        process_id: process_id.into(),
        project_key: project_key.into(),
        container_id: worker_id.into(),
        worker_secret: "secret".into(),
        strategy: strategy.into(),
        agent_type: "claude".into(),
        container_status: "running".into(),
        agent_status: "idle".into(),
        workspace_path: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        idle_redispatch_count: 0,
        brain_path: None,
        runtime: ur_config::WorkerRuntime::Local,
    }
}

/// A worker-port client plus the fake daemon all seeded workers route to:
/// manual, code and design callers and a peer in project `ur`, and one
/// worker in project `ox`.
async fn worker_port_with_peers() -> (
    ur_rpc::proto::core::core_service_client::CoreServiceClient<tonic::transport::Channel>,
    FakeWorkerd,
    ur_db_test::TestDb,
) {
    let fake = FakeWorkerd::default();
    let (mut handler, _ticket_repo, _workflow_repo, test_db) = make_worker_handler().await;
    handler.launch_manager.workerd_resolver = fake_workerd_resolver(&fake).await;
    for row in [
        peer_row(MANUAL, "ur", "manual"),
        peer_row(CODE, "ur", "code"),
        peer_row(DESIGN, "ur", "design"),
        peer_row(PEER_FULL, "ur", "code"),
        peer_row("ox-tgt2-a1", "ox", "manual"),
    ] {
        handler.worker_repo.insert_worker(&row).await.unwrap();
    }
    let (channel, _addr) = spawn_worker_grpc_server(handler).await;
    let client = ur_rpc::proto::core::core_service_client::CoreServiceClient::new(channel);
    (client, fake, test_db)
}

/// `msg` as sent by worker `caller` (the auth interceptor, which checks the
/// secret, is not in front of the test server).
fn as_worker<T>(caller: &str, msg: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    req.metadata_mut()
        .insert(ur_config::WORKER_ID_HEADER, caller.parse().unwrap());
    req
}

type WorkerPortClient =
    ur_rpc::proto::core::core_service_client::CoreServiceClient<tonic::transport::Channel>;
type VerbResults = Vec<(&'static str, Result<(), tonic::Status>)>;

/// Every worker-callable verb that names a target, plus stop and launch,
/// each as `(verb, result)`.
async fn every_targeted_verb(
    client: &mut WorkerPortClient,
    caller: &str,
    target: &str,
) -> VerbResults {
    let mut results = session_verbs(client, caller, target).await;
    results.extend(lifecycle_verbs(client, caller, target).await);
    results
}

/// The six session-control verbs on `target`.
async fn session_verbs(client: &mut WorkerPortClient, caller: &str, target: &str) -> VerbResults {
    use ur_rpc::proto::core::{
        CaptureWorkerPaneRequest, InterruptWorkerRequest, ReadWorkerTranscriptRequest,
        SendWorkerKeysRequest, SendWorkerMessageRequest, WaitWorkerIdleRequest, WorkerKey,
    };
    let id = || target.to_string();
    vec![
        (
            "read --screen",
            client
                .capture_worker_pane(as_worker(
                    caller,
                    CaptureWorkerPaneRequest {
                        worker_id: id(),
                        lines: 3,
                    },
                ))
                .await
                .map(drop),
        ),
        (
            "read",
            client
                .read_worker_transcript(as_worker(
                    caller,
                    ReadWorkerTranscriptRequest {
                        worker_id: id(),
                        turns: 1,
                    },
                ))
                .await
                .map(drop)
                // The fake has no transcript; reaching it is what counts.
                .or_else(|s| match s.code() {
                    tonic::Code::FailedPrecondition => Ok(()),
                    _ => Err(s),
                }),
        ),
        (
            "send",
            client
                .send_worker_message(as_worker(
                    caller,
                    SendWorkerMessageRequest {
                        worker_id: id(),
                        message: "hello".into(),
                        submit: None,
                    },
                ))
                .await
                .map(drop),
        ),
        (
            "keys",
            client
                .send_worker_keys(as_worker(
                    caller,
                    SendWorkerKeysRequest {
                        worker_id: id(),
                        keys: vec![WorkerKey::Enter as i32],
                    },
                ))
                .await
                .map(drop),
        ),
        (
            "interrupt",
            client
                .interrupt_worker(as_worker(
                    caller,
                    InterruptWorkerRequest { worker_id: id() },
                ))
                .await
                .map(drop),
        ),
        (
            "wait-idle",
            client
                .wait_worker_idle(as_worker(
                    caller,
                    WaitWorkerIdleRequest {
                        worker_id: id(),
                        timeout_secs: 1,
                        screen_lines: 1,
                    },
                ))
                .await
                .map(drop),
        ),
    ]
}

/// Launch into `target`'s project (`ur` unless it starts with `ox`), then
/// stop `target`.
async fn lifecycle_verbs(client: &mut WorkerPortClient, caller: &str, target: &str) -> VerbResults {
    use ur_rpc::proto::core::{WorkerLaunchRequest, WorkerStopRequest};
    let project = if target.starts_with("ox") { "ox" } else { "ur" };
    vec![
        (
            "launch",
            client
                .worker_launch(as_worker(
                    caller,
                    WorkerLaunchRequest {
                        worker_id: format!("{project}-new1"),
                        project_key: project.into(),
                        mode: "no-such-mode".into(),
                        ..Default::default()
                    },
                ))
                .await
                .map(drop)
                // Past the policy, the bogus mode stops the launch.
                .or_else(|s| match s.code() {
                    tonic::Code::InvalidArgument => Ok(()),
                    _ => Err(s),
                }),
        ),
        (
            "stop",
            client
                .worker_stop(as_worker(
                    caller,
                    WorkerStopRequest {
                        worker_id: target.to_string(),
                    },
                ))
                .await
                .map(drop)
                // Past the policy, the unreachable builderd fails the stop.
                .or_else(|s| match s.code() {
                    tonic::Code::Internal => Ok(()),
                    _ => Err(s),
                }),
        ),
    ]
}

fn list_ids(resp: ur_rpc::proto::core::WorkerListResponse) -> Vec<String> {
    let mut ids: Vec<_> = resp.workers.into_iter().map(|w| w.worker_id).collect();
    ids.sort();
    ids
}

/// A manual worker can list its project's workers and use every verb on a
/// same-project peer (by process id or full id); the daemon sees the peer.
#[tokio::test]
async fn worker_port_manual_caller_drives_same_project_peer() {
    use ur_rpc::proto::core::{CaptureWorkerPaneRequest, WorkerListRequest};

    let (mut client, fake, _db) = worker_port_with_peers().await;

    let listed = client
        .worker_list(as_worker(MANUAL, WorkerListRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        list_ids(listed),
        ["ur-cod1", "ur-des1", "ur-man1", "ur-tgt1"],
        "only project ur"
    );

    let screen = client
        .capture_worker_pane(as_worker(
            MANUAL,
            CaptureWorkerPaneRequest {
                worker_id: PEER_FULL.into(),
                lines: 3,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(screen.text, "screen+3");

    for (verb, result) in every_targeted_verb(&mut client, MANUAL, PEER).await {
        assert!(result.is_ok(), "{verb}: {:?}", result.unwrap_err());
    }
    assert_eq!(*fake.messages.lock().unwrap(), ["hello"]);
    assert_eq!(*fake.interrupts.lock().unwrap(), 1);
    let routed = fake.routed_ids.lock().unwrap();
    assert!(!routed.is_empty());
    assert!(routed.iter().all(|id| id == PEER_FULL), "{routed:?}");
}

/// Another project's worker looks exactly like a missing one, for every verb.
#[tokio::test]
async fn worker_port_cross_project_target_is_not_found_for_every_verb() {
    let (mut client, fake, _db) = worker_port_with_peers().await;

    for target in [OTHER_PROJECT, "ox-tgt2-a1", "ur-nope"] {
        let results = every_targeted_verb(&mut client, MANUAL, target).await;
        // A launch "into ur-nope" is a same-project launch, not a lookup.
        let lookups = results
            .into_iter()
            .filter(|(verb, _)| !(target == "ur-nope" && *verb == "launch"));
        for (verb, result) in lookups {
            let err = result.expect_err(verb);
            assert_eq!(
                err.code(),
                tonic::Code::NotFound,
                "{target} {verb}: {err:?}"
            );
        }
    }
    assert!(fake.routed_ids.lock().unwrap().is_empty());
}

/// What a non-manual caller gets: `PermissionDenied`, except a design
/// caller's launch, which passes the policy into its own project only.
fn assert_non_manual_outcome(
    caller: &str,
    target: &str,
    verb: &str,
    result: Result<(), tonic::Status>,
) {
    if caller == DESIGN && verb == "launch" {
        let expected = if target == PEER {
            Ok(())
        } else {
            Err(tonic::Code::NotFound)
        };
        assert_eq!(
            result.map_err(|s| s.code()),
            expected,
            "design launch into {target}"
        );
        return;
    }
    let err = result.expect_err(verb);
    assert_eq!(
        err.code(),
        tonic::Code::PermissionDenied,
        "{caller} {verb} {target}: {err:?}"
    );
    assert!(err.message().contains("only manual workers"), "{err:?}");
}

/// Only manual workers may drive other workers: code and design callers are
/// refused on every verb before any lookup. Design keeps same-project launch
/// (`workflow dispatch`), and any worker may still stop itself.
#[tokio::test]
async fn worker_port_non_manual_caller_is_denied_every_verb() {
    use ur_rpc::proto::core::{WorkerListRequest, WorkerStopRequest};

    let (mut client, fake, _db) = worker_port_with_peers().await;

    for caller in [CODE, DESIGN] {
        let err = client
            .worker_list(as_worker(caller, WorkerListRequest {}))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied, "{caller} list");
        assert!(err.message().contains("only manual workers"), "{err:?}");

        for target in [PEER, OTHER_PROJECT] {
            let results = every_targeted_verb(&mut client, caller, target).await;
            results
                .into_iter()
                .for_each(|(verb, result)| assert_non_manual_outcome(caller, target, verb, result));
        }
    }
    assert!(fake.routed_ids.lock().unwrap().is_empty());

    // Self-stop (workerd's exit sink) passes the policy; the unreachable
    // builderd then fails it.
    let err = client
        .worker_stop(as_worker(
            CODE,
            WorkerStopRequest {
                worker_id: CODE.into(),
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal, "{err:?}");
}

/// A caller with no worker row (or no header) is unauthenticated.
#[tokio::test]
async fn worker_port_unknown_caller_is_unauthenticated() {
    use ur_rpc::proto::core::WorkerListRequest;

    let (mut client, _fake, _db) = worker_port_with_peers().await;
    let err = client
        .worker_list(as_worker("ur-ghost-a1", WorkerListRequest {}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    let err = client
        .worker_list(tonic::Request::new(WorkerListRequest {}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}
