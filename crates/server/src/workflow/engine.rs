use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{error, info, warn};

use ur_db::TicketRepo;
use ur_db::WorkerRepo;
use ur_db::WorkflowRepo;

use ur_db::model::LifecycleStatus;

use crate::WorkerManager;

use super::{HandlerEntry, WorkflowContext, WorkflowHandler};

/// Drives workflow transitions by polling the `workflow_event` table and
/// dispatching to registered handlers.
///
/// Implements `Clone` and follows the manager pattern: holds references to
/// dependencies injected via the constructor.
#[derive(Clone)]
pub struct WorkflowEngine {
    ctx: WorkflowContext,
    handlers: HashMap<LifecycleStatus, Arc<dyn WorkflowHandler>>,
    poll_interval: Duration,
}

impl WorkflowEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ticket_repo: TicketRepo,
        workflow_repo: WorkflowRepo,
        worker_repo: WorkerRepo,
        worker_prefix: String,
        builderd_client: ur_rpc::proto::builder::BuilderdClient,
        config: Arc<ur_config::Config>,
        handler_entries: Vec<HandlerEntry>,
        transition_tx: tokio::sync::mpsc::Sender<super::TransitionRequest>,
        worker_manager: WorkerManager,
    ) -> Self {
        let poll_interval = Duration::from_millis(config.server.poll_interval_ms);
        let ctx = WorkflowContext {
            ticket_repo,
            workflow_repo,
            worker_repo,
            worker_prefix,
            builderd_client,
            config,
            transition_tx,
            worker_manager,
        };
        let mut handlers = HashMap::new();
        for (target, handler) in handler_entries {
            handlers.insert(target, handler);
        }
        Self {
            ctx,
            handlers,
            poll_interval,
        }
    }

    /// Spawn the polling loop as a background tokio task.
    ///
    /// The loop runs until `shutdown_rx` signals `true`. Returns a join handle
    /// for the spawned task.
    pub fn spawn(self, shutdown_rx: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(self.run(shutdown_rx))
    }

    /// Internal polling loop.
    async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        info!("workflow engine started");
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.poll_interval) => {
                    self.poll_once().await;
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("workflow engine shutting down");
                        return;
                    }
                }
            }
        }
    }

    /// Poll for and process a single workflow event.
    async fn poll_once(&self) {
        let event = match self.ctx.workflow_repo.poll_workflow_event().await {
            Ok(Some(event)) => event,
            Ok(None) => return,
            Err(e) => {
                error!(error = %e, "failed to poll workflow events");
                return;
            }
        };

        // Idempotency check: the SQLite trigger fires AFTER UPDATE, so the
        // ticket's lifecycle_status has already been set to new_lifecycle_status
        // by the time we poll.  We verify the ticket still has that status —
        // if it has moved on (another transition happened), this event is stale.
        let ticket = match self.ctx.ticket_repo.get_ticket(&event.ticket_id).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                warn!(
                    event_id = %event.id,
                    ticket_id = %event.ticket_id,
                    "ticket not found for workflow event — deleting stale event"
                );
                self.delete_event(&event.id).await;
                return;
            }
            Err(e) => {
                error!(
                    error = %e,
                    ticket_id = %event.ticket_id,
                    "failed to fetch ticket for idempotency check"
                );
                return;
            }
        };

        // If the ticket is not lifecycle-managed, delete the event and skip.
        if !ticket.lifecycle_managed {
            info!(
                event_id = %event.id,
                ticket_id = %event.ticket_id,
                "ticket is not lifecycle-managed — deleting workflow event"
            );
            self.delete_event(&event.id).await;
            return;
        }

        // If the ticket's lifecycle_status doesn't match the transition's
        // target status, a newer transition has superseded this one — skip it.
        if ticket.lifecycle_status != event.new_lifecycle_status {
            warn!(
                event_id = %event.id,
                ticket_id = %event.ticket_id,
                expected = %event.new_lifecycle_status,
                actual = %ticket.lifecycle_status,
                "lifecycle status moved past this transition — deleting stale event"
            );
            if let Err(e) = self
                .ctx
                .workflow_repo
                .delete_workflow_event(&event.id)
                .await
            {
                error!(error = %e, "failed to delete stale workflow event");
            }
            return;
        }

        let target = event.new_lifecycle_status;

        let handler = match self.handlers.get(&target) {
            Some(h) => h,
            None => {
                warn!(
                    event_id = %event.id,
                    target = %target,
                    "no handler registered for target status — deleting event"
                );
                self.delete_event(&event.id).await;
                return;
            }
        };

        match handler.handle(&self.ctx, &event.ticket_id).await {
            Ok(()) => {
                self.handle_success(&event.id, &event.ticket_id, target)
                    .await
            }
            Err(handler_err) => self.handle_failure(&event, target, handler_err).await,
        }
    }

    /// Clean up after a successful handler execution.
    async fn handle_success(&self, event_id: &str, ticket_id: &str, target: LifecycleStatus) {
        info!(
            event_id = %event_id,
            ticket_id = %ticket_id,
            target = %target,
            "workflow handler completed successfully"
        );
        self.delete_event(event_id).await;
    }

    /// Handle a failed handler: stall the workflow immediately.
    async fn handle_failure(
        &self,
        event: &ur_db::model::WorkflowEvent,
        target: LifecycleStatus,
        handler_err: anyhow::Error,
    ) {
        error!(
            event_id = %event.id,
            ticket_id = %event.ticket_id,
            target = %target,
            error = %handler_err,
            "workflow handler failed — stalling workflow"
        );

        if let Err(e) = self
            .ctx
            .workflow_repo
            .set_workflow_stalled(&event.ticket_id, &format!("{handler_err}"))
            .await
        {
            error!(error = %e, "failed to set workflow stalled");
        }

        self.delete_event(&event.id).await;
    }

    /// Delete a workflow event, logging any errors.
    async fn delete_event(&self, event_id: &str) {
        if let Err(e) = self.ctx.workflow_repo.delete_workflow_event(event_id).await {
            error!(error = %e, event_id = %event_id, "failed to delete workflow event");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tempfile::TempDir;
    use ur_db::model::{LifecycleStatus, NewTicket};
    use ur_db::{DatabaseManager, GraphManager, TicketRepo, WorkerRepo, WorkflowRepo};

    async fn setup_test_db() -> (TempDir, TicketRepo, WorkflowRepo, WorkerRepo) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = DatabaseManager::open(&db_path.to_string_lossy())
            .await
            .expect("open test db");
        let graph_manager = GraphManager::new(db.pool().clone());
        let repo = TicketRepo::new(db.pool().clone(), graph_manager);
        let workflow_repo = WorkflowRepo::new(db.pool().clone());
        let worker_repo = WorkerRepo::new(db.pool().clone());
        (tmp, repo, workflow_repo, worker_repo)
    }

    fn dummy_builderd_client() -> ur_rpc::proto::builder::BuilderdClient {
        let channel =
            tonic::transport::Endpoint::from_static("http://localhost:50051").connect_lazy();
        ur_rpc::proto::builder::BuilderdClient::new(channel)
    }

    fn dummy_config() -> Arc<ur_config::Config> {
        Arc::new(ur_config::Config {
            config_dir: std::path::PathBuf::from("/tmp/test"),
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

    fn dummy_transition_tx() -> tokio::sync::mpsc::Sender<crate::workflow::TransitionRequest> {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        tx
    }

    fn dummy_worker_manager(worker_repo: WorkerRepo) -> crate::WorkerManager {
        let builderd_client = dummy_builderd_client();
        let config = dummy_config();
        let local_repo = local_repo::GitBackend {
            client: builderd_client.clone(),
        };
        let pool = crate::RepoPoolManager::new(
            &config,
            std::path::PathBuf::from("/tmp/test/workspace"),
            std::path::PathBuf::from("/tmp/test/workspace"),
            builderd_client,
            local_repo,
            worker_repo.clone(),
        );
        let network_manager = container::NetworkManager::new("docker".into(), "ur-workers".into());
        crate::WorkerManager::new(
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

    struct CountingHandler {
        call_count: Arc<AtomicU32>,
        should_fail: bool,
    }

    impl WorkflowHandler for CountingHandler {
        fn handle(
            &self,
            _ctx: &WorkflowContext,
            _ticket_id: &str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), anyhow::Error>> + Send + '_>,
        > {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let result = if self.should_fail {
                Err(anyhow::anyhow!("intentional test failure"))
            } else {
                Ok(())
            };
            Box::pin(std::future::ready(result))
        }
    }

    #[tokio::test]
    async fn engine_processes_event_and_deletes_on_success() {
        let (_tmp, repo, workflow_repo, worker_repo) = setup_test_db().await;

        let ticket = NewTicket {
            id: "ur-test1".to_string(),
            project: "ur".to_string(),
            type_: "task".to_string(),
            priority: 2,
            title: "Test ticket".to_string(),
            body: String::new(),
            lifecycle_status: Some(LifecycleStatus::Open),
            ..Default::default()
        };
        repo.create_ticket(&ticket).await.unwrap();

        let update = ur_db::model::TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::Implementing),
            lifecycle_managed: Some(true),
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        repo.update_ticket("ur-test1", &update).await.unwrap();

        // Verify event exists
        let event = workflow_repo.poll_workflow_event().await.unwrap();
        assert!(event.is_some(), "workflow event should exist");

        let call_count = Arc::new(AtomicU32::new(0));
        let engine = WorkflowEngine::new(
            repo.clone(),
            workflow_repo.clone(),
            worker_repo.clone(),
            "ur-worker-".to_string(),
            dummy_builderd_client(),
            dummy_config(),
            vec![(
                LifecycleStatus::Implementing,
                Arc::new(CountingHandler {
                    call_count: call_count.clone(),
                    should_fail: false,
                }) as Arc<dyn WorkflowHandler>,
            )],
            dummy_transition_tx(),
            dummy_worker_manager(worker_repo.clone()),
        );

        engine.poll_once().await;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "handler should be called once"
        );

        let event = workflow_repo.poll_workflow_event().await.unwrap();
        assert!(event.is_none(), "event should be deleted after success");
    }

    #[tokio::test]
    async fn engine_stalls_workflow_on_failure() {
        let (_tmp, repo, workflow_repo, worker_repo) = setup_test_db().await;

        let ticket = NewTicket {
            id: "ur-test2".to_string(),
            project: "ur".to_string(),
            type_: "task".to_string(),
            priority: 2,
            title: "Test ticket".to_string(),
            body: String::new(),
            lifecycle_status: Some(LifecycleStatus::Open),
            ..Default::default()
        };
        repo.create_ticket(&ticket).await.unwrap();

        // Create a workflow row so set_workflow_stalled has something to update.
        workflow_repo
            .create_workflow("ur-test2", LifecycleStatus::Implementing)
            .await
            .unwrap();

        let update = ur_db::model::TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::Implementing),
            lifecycle_managed: Some(true),
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        repo.update_ticket("ur-test2", &update).await.unwrap();

        let call_count = Arc::new(AtomicU32::new(0));
        let engine = WorkflowEngine::new(
            repo.clone(),
            workflow_repo.clone(),
            worker_repo.clone(),
            "ur-worker-".to_string(),
            dummy_builderd_client(),
            dummy_config(),
            vec![(
                LifecycleStatus::Implementing,
                Arc::new(CountingHandler {
                    call_count: call_count.clone(),
                    should_fail: true,
                }) as Arc<dyn WorkflowHandler>,
            )],
            dummy_transition_tx(),
            dummy_worker_manager(worker_repo.clone()),
        );

        engine.poll_once().await;
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Workflow should be stalled with the error message.
        let wf = workflow_repo
            .get_workflow_by_ticket("ur-test2")
            .await
            .unwrap()
            .unwrap();
        assert!(wf.stalled, "workflow should be stalled after failure");
        assert_eq!(wf.stall_reason, "intentional test failure");

        // Ticket stays in its current lifecycle state (not reverted to open).
        let t = repo.get_ticket("ur-test2").await.unwrap().unwrap();
        assert_eq!(t.lifecycle_status, LifecycleStatus::Implementing);

        // Workflow event is deleted (engine won't retry).
        let event = workflow_repo.poll_workflow_event().await.unwrap();
        assert!(event.is_none(), "event should be deleted after stalling");
    }

    #[tokio::test]
    async fn engine_open_to_awaiting_dispatch_noop_processes_and_deletes() {
        let (_tmp, repo, workflow_repo, worker_repo) = setup_test_db().await;

        let ticket = NewTicket {
            id: "ur-ad01".to_string(),
            project: "ur".to_string(),
            type_: "task".to_string(),
            priority: 2,
            title: "Awaiting dispatch test".to_string(),
            body: String::new(),
            lifecycle_status: Some(LifecycleStatus::Open),
            ..Default::default()
        };
        repo.create_ticket(&ticket).await.unwrap();

        // Enable lifecycle management and transition to AwaitingDispatch.
        let update = ur_db::model::TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::AwaitingDispatch),
            lifecycle_managed: Some(true),
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        repo.update_ticket("ur-ad01", &update).await.unwrap();

        // Verify event exists.
        let event = workflow_repo.poll_workflow_event().await.unwrap();
        assert!(event.is_some(), "workflow event should exist");

        let call_count = Arc::new(AtomicU32::new(0));
        let engine = WorkflowEngine::new(
            repo.clone(),
            workflow_repo.clone(),
            worker_repo.clone(),
            "ur-worker-".to_string(),
            dummy_builderd_client(),
            dummy_config(),
            vec![(
                LifecycleStatus::AwaitingDispatch,
                Arc::new(CountingHandler {
                    call_count: call_count.clone(),
                    should_fail: false,
                }) as Arc<dyn WorkflowHandler>,
            )],
            dummy_transition_tx(),
            dummy_worker_manager(worker_repo.clone()),
        );

        engine.poll_once().await;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "no-op handler should be called once"
        );

        // Event should be deleted after successful processing.
        let event = workflow_repo.poll_workflow_event().await.unwrap();
        assert!(
            event.is_none(),
            "event should be deleted after no-op handler succeeds"
        );

        // Ticket should still be in AwaitingDispatch.
        let t = repo.get_ticket("ur-ad01").await.unwrap().unwrap();
        assert_eq!(t.lifecycle_status, LifecycleStatus::AwaitingDispatch);
    }

    #[tokio::test]
    async fn engine_awaiting_dispatch_to_implementing_fires_handler() {
        let (_tmp, repo, workflow_repo, worker_repo) = setup_test_db().await;

        let ticket = NewTicket {
            id: "ur-ad02".to_string(),
            project: "ur".to_string(),
            type_: "task".to_string(),
            priority: 2,
            title: "Dispatch implement test".to_string(),
            body: String::new(),
            lifecycle_status: Some(LifecycleStatus::Open),
            ..Default::default()
        };
        repo.create_ticket(&ticket).await.unwrap();

        // First transition: Open → AwaitingDispatch (enable lifecycle management).
        let update = ur_db::model::TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::AwaitingDispatch),
            lifecycle_managed: Some(true),
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        repo.update_ticket("ur-ad02", &update).await.unwrap();

        // Drain the Open→AwaitingDispatch event (not relevant to this test).
        workflow_repo.poll_workflow_event().await.unwrap();
        // Delete it manually since we have no handler for it in this engine.
        // Use a small engine with the no-op handler.
        let noop_engine = WorkflowEngine::new(
            repo.clone(),
            workflow_repo.clone(),
            worker_repo.clone(),
            "ur-worker-".to_string(),
            dummy_builderd_client(),
            dummy_config(),
            vec![(
                LifecycleStatus::AwaitingDispatch,
                Arc::new(CountingHandler {
                    call_count: Arc::new(AtomicU32::new(0)),
                    should_fail: false,
                }) as Arc<dyn WorkflowHandler>,
            )],
            dummy_transition_tx(),
            dummy_worker_manager(worker_repo.clone()),
        );
        noop_engine.poll_once().await;

        // Second transition: AwaitingDispatch → Implementing.
        let update = ur_db::model::TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::Implementing),
            lifecycle_managed: None,
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        repo.update_ticket("ur-ad02", &update).await.unwrap();

        // Verify event exists.
        let event = workflow_repo.poll_workflow_event().await.unwrap();
        assert!(
            event.is_some(),
            "AwaitingDispatch→Implementing event should exist"
        );

        let call_count = Arc::new(AtomicU32::new(0));
        let engine = WorkflowEngine::new(
            repo.clone(),
            workflow_repo.clone(),
            worker_repo.clone(),
            "ur-worker-".to_string(),
            dummy_builderd_client(),
            dummy_config(),
            vec![(
                LifecycleStatus::Implementing,
                Arc::new(CountingHandler {
                    call_count: call_count.clone(),
                    should_fail: false,
                }) as Arc<dyn WorkflowHandler>,
            )],
            dummy_transition_tx(),
            dummy_worker_manager(worker_repo.clone()),
        );

        engine.poll_once().await;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "DispatchImplementHandler stand-in should be called once"
        );

        // Event should be deleted after successful processing.
        let event = workflow_repo.poll_workflow_event().await.unwrap();
        assert!(
            event.is_none(),
            "event should be deleted after handler succeeds"
        );
    }

    #[tokio::test]
    async fn engine_deletes_event_with_no_handler() {
        let (_tmp, repo, workflow_repo, worker_repo) = setup_test_db().await;

        let ticket = NewTicket {
            id: "ur-test4".to_string(),
            project: "ur".to_string(),
            type_: "task".to_string(),
            priority: 2,
            title: "Test ticket".to_string(),
            body: String::new(),
            lifecycle_status: Some(LifecycleStatus::Open),
            ..Default::default()
        };
        repo.create_ticket(&ticket).await.unwrap();

        let update = ur_db::model::TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::Implementing),
            lifecycle_managed: Some(true),
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        repo.update_ticket("ur-test4", &update).await.unwrap();

        let engine = WorkflowEngine::new(
            repo.clone(),
            workflow_repo.clone(),
            worker_repo.clone(),
            "ur-worker-".to_string(),
            dummy_builderd_client(),
            dummy_config(),
            vec![],
            dummy_transition_tx(),
            dummy_worker_manager(worker_repo.clone()),
        );

        engine.poll_once().await;

        let event = workflow_repo.poll_workflow_event().await.unwrap();
        assert!(
            event.is_none(),
            "event should be deleted when no handler is registered"
        );
    }
}
