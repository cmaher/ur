// Integration tests for the full workflow lifecycle flow.
//
// These tests exercise the coordinator, WorkerCoreServiceHandler, and step router
// with mock handlers that replace the real workerd/builderd-dependent handlers.
// The mock handlers record calls and optionally auto-advance to the next state.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tempfile::TempDir;
use tokio::sync::{Mutex, watch};
use tonic::transport::{Endpoint, Server};

use ur_db::model::{LifecycleStatus, NewTicket, Worker};
use ur_db::{DatabaseManager, GraphManager, TicketRepo, WorkerRepo, WorkflowRepo};
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::{UpdateAgentStatusRequest, WorkflowStepCompleteRequest};
use ur_server::workflow::{
    HandlerEntry, HandlerFuture, TransitionRequest, WorkflowContext, WorkflowCoordinator,
    WorkflowHandler, coordinator_cancel_channel, coordinator_channel,
};

// ---------------------------------------------------------------------------
// Mock handler: records calls, optionally auto-advances to a next state
// ---------------------------------------------------------------------------

struct MockHandler {
    name: &'static str,
    call_count: Arc<AtomicU32>,
    ticket_ids: Arc<Mutex<Vec<String>>>,
    auto_advance_to: Option<LifecycleStatus>,
}

impl MockHandler {
    fn new(name: &'static str) -> (Self, Arc<AtomicU32>, Arc<Mutex<Vec<String>>>) {
        let call_count = Arc::new(AtomicU32::new(0));
        let ticket_ids = Arc::new(Mutex::new(Vec::new()));
        let handler = Self {
            name,
            call_count: call_count.clone(),
            ticket_ids: ticket_ids.clone(),
            auto_advance_to: None,
        };
        (handler, call_count, ticket_ids)
    }

    fn with_auto_advance(mut self, to: LifecycleStatus) -> Self {
        self.auto_advance_to = Some(to);
        self
    }
}

impl WorkflowHandler for MockHandler {
    fn handle(&self, ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let ticket_ids = self.ticket_ids.clone();
        let tid = ticket_id.to_owned();
        let name = self.name;
        let auto_advance = self.auto_advance_to;
        let transition_tx = ctx.transition_tx.clone();

        Box::pin(async move {
            tracing::info!(
                handler = name,
                ticket_id = tid.as_str(),
                "mock handler invoked"
            );
            ticket_ids.lock().await.push(tid.clone());

            if let Some(to) = auto_advance {
                transition_tx
                    .send(TransitionRequest {
                        ticket_id: tid,
                        target_status: to,
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("auto-advance failed: {e}"))?;
            }

            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// Test harness: coordinator + gRPC server in one bundle
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct TestHarness {
    client: CoreServiceClient<tonic::transport::Channel>,
    ticket_repo: TicketRepo,
    workflow_repo: WorkflowRepo,
    transition_tx: tokio::sync::mpsc::Sender<TransitionRequest>,
    shutdown_tx: watch::Sender<bool>,
    coord_handle: tokio::task::JoinHandle<()>,
    _cancel_tx: tokio::sync::mpsc::Sender<String>,
}

impl TestHarness {
    async fn new(
        ticket_repo: TicketRepo,
        workflow_repo: WorkflowRepo,
        worker_repo: WorkerRepo,
        handlers: Vec<HandlerEntry>,
    ) -> Self {
        let (transition_tx, transition_rx) = coordinator_channel(64);
        let (_cancel_tx, cancel_rx) = coordinator_cancel_channel(16);

        let ctx = WorkflowContext {
            ticket_repo: ticket_repo.clone(),
            workflow_repo: workflow_repo.clone(),
            worker_repo: worker_repo.clone(),
            worker_prefix: "ur-worker-".to_string(),
            builderd_client: dummy_builderd_client(),
            config: dummy_config(),
            transition_tx: transition_tx.clone(),
            worker_manager: dummy_worker_manager(worker_repo.clone()),
        };

        let coordinator = WorkflowCoordinator::new(transition_rx, cancel_rx, ctx, &handlers);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let coord_handle = coordinator.spawn(shutdown_rx);

        let worker_handler = ur_server::grpc::WorkerCoreServiceHandler {
            worker_repo,
            ticket_repo: ticket_repo.clone(),
            workflow_repo: workflow_repo.clone(),
            worker_prefix: "ur-worker-".to_string(),
            transition_tx: transition_tx.clone(),
        };

        let (channel, _addr) = spawn_worker_server(worker_handler).await;
        let client = CoreServiceClient::new(channel);

        Self {
            client,
            ticket_repo,
            workflow_repo,
            transition_tx,
            shutdown_tx,
            coord_handle,
            _cancel_tx,
        }
    }

    /// Send UpdateAgentStatus(idle) for a worker.
    async fn send_idle(&mut self, worker_id: &str) {
        let req = worker_request(
            UpdateAgentStatusRequest {
                worker_id: worker_id.to_string(),
                status: "idle".to_string(),
                message: String::new(),
            },
            worker_id,
        );
        self.client.update_agent_status(req).await.unwrap();
    }

    /// Send WorkflowStepComplete for a worker.
    async fn send_step_complete(&mut self, worker_id: &str) {
        let req = worker_request(
            WorkflowStepCompleteRequest {
                worker_id: worker_id.to_string(),
            },
            worker_id,
        );
        self.client.workflow_step_complete(req).await.unwrap();
    }

    /// Wait for a workflow to reach a specific status, with timeout.
    async fn wait_for_status(
        &self,
        ticket_id: &str,
        expected: LifecycleStatus,
        timeout_ms: u64,
    ) -> LifecycleStatus {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            if let Ok(Some(wf)) = self.workflow_repo.get_workflow_by_ticket(ticket_id).await
                && wf.status == expected
            {
                return wf.status;
            }
            if tokio::time::Instant::now() >= deadline {
                let wf = self
                    .workflow_repo
                    .get_workflow_by_ticket(ticket_id)
                    .await
                    .unwrap();
                return wf.map(|w| w.status).unwrap_or(LifecycleStatus::Open);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    async fn shutdown(self) {
        self.shutdown_tx.send(true).unwrap();
        self.coord_handle.await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// Test infrastructure helpers
// ---------------------------------------------------------------------------

async fn setup_db() -> (TempDir, TicketRepo, WorkflowRepo, WorkerRepo) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DatabaseManager::open(&db_path.to_string_lossy())
        .await
        .expect("open test db");
    let graph = GraphManager::new(db.pool().clone());
    let ticket_repo = TicketRepo::new(db.pool().clone(), graph);
    let workflow_repo = WorkflowRepo::new(db.pool().clone());
    let worker_repo = WorkerRepo::new(db.pool().clone());
    (tmp, ticket_repo, workflow_repo, worker_repo)
}

fn dummy_worker_manager(worker_repo: WorkerRepo) -> ur_server::WorkerManager {
    let builderd_client = dummy_builderd_client();
    let config = dummy_config();
    let local_repo = local_repo::GitBackend {
        client: builderd_client.clone(),
    };
    let pool = ur_server::RepoPoolManager::new(
        &config,
        std::path::PathBuf::from("/tmp/test/workspace"),
        std::path::PathBuf::from("/tmp/test/workspace"),
        builderd_client,
        local_repo,
        worker_repo.clone(),
    );
    let network_manager = container::NetworkManager::new("docker".into(), "ur-workers".into());
    ur_server::WorkerManager::new(
        std::path::PathBuf::from("/tmp/test/workspace"),
        std::path::PathBuf::from("/tmp/test"),
        pool,
        network_manager,
        config.network.clone(),
        config.worker_port,
        Default::default(),
        worker_repo,
    )
}

fn dummy_builderd_client() -> ur_rpc::proto::builder::BuilderdClient {
    let channel = tonic::transport::Endpoint::from_static("http://localhost:50051").connect_lazy();
    ur_rpc::proto::builder::BuilderdClient::new(channel)
}

fn dummy_config() -> Arc<ur_config::Config> {
    Arc::new(ur_config::Config {
        config_dir: std::path::PathBuf::from("/tmp/test"),
        logs_dir: std::path::PathBuf::from("/tmp/test/logs"),
        workspace: std::path::PathBuf::from("/tmp/test/workspace"),
        server_port: ur_config::DEFAULT_SERVER_PORT,
        builderd_port: ur_config::DEFAULT_SERVER_PORT + 2,
        worker_port: ur_config::DEFAULT_SERVER_PORT + 1,
        compose_file: std::path::PathBuf::from("/tmp/test/docker-compose.yml"),
        proxy: ur_config::ProxyConfig {
            hostname: ur_config::DEFAULT_PROXY_HOSTNAME.into(),
            allowlist: vec![],
        },
        network: ur_config::NetworkConfig {
            name: ur_config::DEFAULT_NETWORK_NAME.into(),
            worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
            server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
            worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.into(),
        },
        hostexec: ur_config::HostExecConfig::default(),
        rag: ur_config::RagConfig {
            qdrant_hostname: ur_config::DEFAULT_QDRANT_HOSTNAME.into(),
            embedding_model: ur_config::DEFAULT_EMBEDDING_MODEL.into(),
            docs: ur_config::RagDocsConfig::default(),
        },
        backup: ur_config::BackupConfig {
            path: None,
            interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
            enabled: true,
            retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
        },
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
        projects: std::collections::HashMap::new(),
        tui: ur_config::TuiConfig::default(),
    })
}

/// Spawn a worker gRPC server serving the WorkerCoreServiceHandler WITHOUT auth.
async fn spawn_worker_server(
    handler: ur_server::grpc::WorkerCoreServiceHandler,
) -> (tonic::transport::Channel, std::net::SocketAddr) {
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

/// Create a gRPC request with worker auth metadata headers.
fn worker_request<T>(inner: T, worker_id: &str) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let md = req.metadata_mut();
    md.insert(ur_config::WORKER_ID_HEADER, worker_id.parse().unwrap());
    md.insert(
        ur_config::WORKER_SECRET_HEADER,
        format!("secret-{worker_id}").parse().unwrap(),
    );
    req
}

/// Create test ticket, worker, and workflow records in the DB.
async fn seed_ticket_and_worker(
    ticket_repo: &TicketRepo,
    workflow_repo: &WorkflowRepo,
    worker_repo: &WorkerRepo,
    ticket_id: &str,
    worker_id: &str,
) {
    let ticket = NewTicket {
        id: Some(ticket_id.to_string()),
        project: "ur".to_string(),
        type_: "task".to_string(),
        priority: 2,
        title: "Integration test ticket".to_string(),
        body: String::new(),
        lifecycle_status: Some(LifecycleStatus::Open),
        ..Default::default()
    };
    ticket_repo.create_ticket(&ticket).await.unwrap();

    workflow_repo
        .create_workflow(ticket_id, LifecycleStatus::AwaitingDispatch)
        .await
        .unwrap();

    let now = chrono::Utc::now().to_rfc3339();
    let worker = Worker {
        worker_id: worker_id.to_string(),
        process_id: ticket_id.to_string(),
        project_key: "ur".to_string(),
        container_id: "test-container-id".to_string(),
        worker_secret: format!("secret-{worker_id}"),
        strategy: "code".to_string(),
        container_status: "running".to_string(),
        agent_status: "starting".to_string(),
        workspace_path: Some("/tmp/test/workspace".to_string()),
        created_at: now.clone(),
        updated_at: now,
        idle_redispatch_count: 0,
    };
    worker_repo.insert_worker(&worker).await.unwrap();

    workflow_repo
        .set_workflow_worker_id(ticket_id, worker_id)
        .await
        .unwrap();
}

/// Mock handler counters for the full lifecycle.
struct LifecycleCounters {
    handlers: Vec<HandlerEntry>,
    awaiting_dispatch: Arc<AtomicU32>,
    implementing: Arc<AtomicU32>,
    verifying: Arc<AtomicU32>,
    pushing: Arc<AtomicU32>,
    in_review: Arc<AtomicU32>,
    feedback_creating: Arc<AtomicU32>,
    merging: Arc<AtomicU32>,
}

/// Build the full set of mock handlers for lifecycle testing.
///
/// Verifying, Pushing, and InReview handlers auto-advance to simulate the real flow.
fn build_lifecycle_handlers() -> LifecycleCounters {
    let (await_h, await_count, _) = MockHandler::new("awaiting_dispatch");
    let (impl_h, impl_count, _) = MockHandler::new("implementing");
    let (verify_h, verify_count, _) = MockHandler::new("verifying");
    let verify_h = verify_h.with_auto_advance(LifecycleStatus::Pushing);
    let (push_h, push_count, _) = MockHandler::new("pushing");
    let push_h = push_h.with_auto_advance(LifecycleStatus::InReview);
    let (review_h, review_count, _) = MockHandler::new("in_review");
    let review_h = review_h.with_auto_advance(LifecycleStatus::FeedbackCreating);
    let (feedback_h, feedback_count, _) = MockHandler::new("feedback_creating");
    let (merge_h, merge_count, _) = MockHandler::new("merging");

    LifecycleCounters {
        handlers: vec![
            (LifecycleStatus::AwaitingDispatch, Arc::new(await_h)),
            (LifecycleStatus::Implementing, Arc::new(impl_h)),
            (LifecycleStatus::Verifying, Arc::new(verify_h)),
            (LifecycleStatus::Pushing, Arc::new(push_h)),
            (LifecycleStatus::InReview, Arc::new(review_h)),
            (LifecycleStatus::FeedbackCreating, Arc::new(feedback_h)),
            (LifecycleStatus::Merging, Arc::new(merge_h)),
        ],
        awaiting_dispatch: await_count,
        implementing: impl_count,
        verifying: verify_count,
        pushing: push_count,
        in_review: review_count,
        feedback_creating: feedback_count,
        merging: merge_count,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test the full workflow lifecycle from AwaitingDispatch through Merging.
///
/// Exercises the coordinator's pending-request dequeue (the bug fix), the
/// WorkerCoreServiceHandler's idle detection and step-complete routing, and
/// the step router's status-based routing.
///
/// Flow:
///   AwaitingDispatch → (idle signal) → Implementing → (step complete) →
///   Verifying → (auto) → Pushing → (auto) → InReview → (auto) →
///   FeedbackCreating → (step complete + feedback_mode=later) → Merging
#[tokio::test]
async fn full_lifecycle_awaiting_dispatch_through_merging() {
    let (_tmp, ticket_repo, workflow_repo, worker_repo) = setup_db().await;

    let ticket_id = "ur-integ1";
    let worker_id = "w-integ1";

    seed_ticket_and_worker(
        &ticket_repo,
        &workflow_repo,
        &worker_repo,
        ticket_id,
        worker_id,
    )
    .await;
    workflow_repo
        .set_workflow_feedback_mode(ticket_id, ur_rpc::feedback_mode::LATER)
        .await
        .unwrap();

    let lc = build_lifecycle_handlers();

    let mut h = TestHarness::new(ticket_repo, workflow_repo, worker_repo, lc.handlers).await;

    // Phase 1: Worker reports idle → AwaitingDispatch → Implementing
    h.send_idle(worker_id).await;
    let status = h
        .wait_for_status(ticket_id, LifecycleStatus::Implementing, 2000)
        .await;
    assert_eq!(status, LifecycleStatus::Implementing);
    assert_eq!(lc.implementing.load(Ordering::SeqCst), 1);

    // Phase 2: Step complete cascades: Implementing → Verifying → Pushing → InReview → FeedbackCreating
    h.send_step_complete(worker_id).await;
    let status = h
        .wait_for_status(ticket_id, LifecycleStatus::FeedbackCreating, 3000)
        .await;
    assert_eq!(status, LifecycleStatus::FeedbackCreating);
    assert_eq!(lc.verifying.load(Ordering::SeqCst), 1);
    assert_eq!(lc.pushing.load(Ordering::SeqCst), 1);
    assert_eq!(lc.in_review.load(Ordering::SeqCst), 1);
    assert_eq!(lc.feedback_creating.load(Ordering::SeqCst), 1);

    // Phase 3: Step complete → FeedbackCreating (feedback_mode=later) → Merging
    h.send_step_complete(worker_id).await;
    let status = h
        .wait_for_status(ticket_id, LifecycleStatus::Merging, 2000)
        .await;
    assert_eq!(status, LifecycleStatus::Merging);
    assert_eq!(lc.merging.load(Ordering::SeqCst), 1);
    assert_eq!(lc.awaiting_dispatch.load(Ordering::SeqCst), 0);

    h.shutdown().await;
}

/// Test the FeedbackCreating → Implementing path (feedback_mode=now).
#[tokio::test]
async fn feedback_mode_now_routes_back_to_implementing() {
    let (_tmp, ticket_repo, workflow_repo, worker_repo) = setup_db().await;

    let ticket_id = "ur-integ2";
    let worker_id = "w-integ2";

    seed_ticket_and_worker(
        &ticket_repo,
        &workflow_repo,
        &worker_repo,
        ticket_id,
        worker_id,
    )
    .await;
    workflow_repo
        .update_workflow_status(ticket_id, LifecycleStatus::FeedbackCreating)
        .await
        .unwrap();
    workflow_repo
        .set_workflow_feedback_mode(ticket_id, ur_rpc::feedback_mode::NOW)
        .await
        .unwrap();

    let (impl_h, impl_count, _) = MockHandler::new("implementing");
    let (feedback_h, _feedback_count, _) = MockHandler::new("feedback_creating");
    let handlers: Vec<HandlerEntry> = vec![
        (LifecycleStatus::Implementing, Arc::new(impl_h)),
        (LifecycleStatus::FeedbackCreating, Arc::new(feedback_h)),
    ];

    let mut h = TestHarness::new(ticket_repo, workflow_repo, worker_repo, handlers).await;

    h.send_step_complete(worker_id).await;
    let status = h
        .wait_for_status(ticket_id, LifecycleStatus::Implementing, 2000)
        .await;
    assert_eq!(status, LifecycleStatus::Implementing);
    assert_eq!(impl_count.load(Ordering::SeqCst), 1);

    h.shutdown().await;
}

/// Test that the coordinator correctly dequeues pending requests after
/// handler completion (the specific bug this patch fixes).
#[tokio::test]
async fn coordinator_dequeues_pending_across_grpc_boundary() {
    let (_tmp, ticket_repo, workflow_repo, worker_repo) = setup_db().await;

    let ticket_id = "ur-integ3";
    let worker_id = "w-integ3";

    seed_ticket_and_worker(
        &ticket_repo,
        &workflow_repo,
        &worker_repo,
        ticket_id,
        worker_id,
    )
    .await;

    let (await_h, await_count, _) = MockHandler::new("awaiting_dispatch");
    let (impl_h, impl_count, _) = MockHandler::new("implementing");
    let handlers: Vec<HandlerEntry> = vec![
        (LifecycleStatus::AwaitingDispatch, Arc::new(await_h)),
        (LifecycleStatus::Implementing, Arc::new(impl_h)),
    ];

    let h = TestHarness::new(ticket_repo, workflow_repo, worker_repo, handlers).await;

    // Send both transitions directly — the second should be queued as pending
    // and processed after the first completes.
    h.transition_tx
        .send(TransitionRequest {
            ticket_id: ticket_id.to_string(),
            target_status: LifecycleStatus::AwaitingDispatch,
        })
        .await
        .unwrap();
    h.transition_tx
        .send(TransitionRequest {
            ticket_id: ticket_id.to_string(),
            target_status: LifecycleStatus::Implementing,
        })
        .await
        .unwrap();

    let status = h
        .wait_for_status(ticket_id, LifecycleStatus::Implementing, 2000)
        .await;
    assert_eq!(status, LifecycleStatus::Implementing);
    assert_eq!(await_count.load(Ordering::SeqCst), 1);
    assert_eq!(impl_count.load(Ordering::SeqCst), 1);

    h.shutdown().await;
}
