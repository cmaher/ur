use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use ur_db::WorkflowRepo;
use ur_db::model::LifecycleStatus;

use super::{HandlerEntry, WorkflowContext, WorkflowHandler};

/// A request to transition a ticket to a target lifecycle status.
#[derive(Debug, Clone)]
pub struct TransitionRequest {
    pub ticket_id: String,
    pub target_status: LifecycleStatus,
}

/// Tracks in-flight and pending transitions for a single ticket.
struct TicketSlot {
    /// The currently running handler task.
    /// Used for cancellation when a ticket is closed.
    handle: JoinHandle<()>,
    /// The most recent pending request (latest wins, intermediates dropped).
    pending: Option<TransitionRequest>,
}

/// Channel-driven workflow coordinator with per-ticket task serialization.
///
/// Receives `TransitionRequest`s via an mpsc channel, writes intents to the
/// database, and spawns per-ticket handler tasks. At most one handler runs
/// per ticket at a time; if a new request arrives while a handler is running,
/// it replaces any pending request (latest wins).
///
/// On startup, recovers incomplete intents from the `workflow_intent` table
/// and re-spawns their handlers.
pub struct WorkflowCoordinator {
    rx: mpsc::Receiver<TransitionRequest>,
    cancel_rx: mpsc::Receiver<String>,
    completion_rx: mpsc::Receiver<String>,
    completion_tx: mpsc::Sender<String>,
    ctx: WorkflowContext,
    handlers: HashMap<LifecycleStatus, Arc<dyn WorkflowHandler>>,
    in_flight: HashMap<String, TicketSlot>,
}

impl WorkflowCoordinator {
    pub fn new(
        rx: mpsc::Receiver<TransitionRequest>,
        cancel_rx: mpsc::Receiver<String>,
        ctx: WorkflowContext,
        handler_entries: &[HandlerEntry],
    ) -> Self {
        let mut handlers = HashMap::new();
        for (target, handler) in handler_entries {
            handlers.insert(*target, handler.clone());
        }
        let (completion_tx, completion_rx) = mpsc::channel(64);
        Self {
            rx,
            cancel_rx,
            completion_rx,
            completion_tx,
            ctx,
            handlers,
            in_flight: HashMap::new(),
        }
    }

    /// Spawn the coordinator as a background tokio task.
    ///
    /// Recovers incomplete intents on startup, then processes incoming
    /// transition requests until shutdown is signaled.
    pub fn spawn(mut self, shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.recover().await;
            self.run(shutdown_rx).await;
        })
    }

    /// Recover incomplete intents from a previous run by re-spawning handlers.
    async fn recover(&mut self) {
        let intents = match self.ctx.workflow_repo.list_intents().await {
            Ok(intents) => intents,
            Err(e) => {
                error!(error = %e, "failed to list intents for recovery");
                return;
            }
        };

        if intents.is_empty() {
            return;
        }

        info!(
            count = intents.len(),
            "recovering incomplete workflow intents"
        );

        for intent in intents {
            // Check if the workflow is already stalled.
            let stalled = match self
                .ctx
                .workflow_repo
                .get_workflow_by_ticket(&intent.ticket_id)
                .await
            {
                Ok(Some(wf)) => wf.stalled,
                _ => false,
            };

            if stalled {
                warn!(
                    intent_id = %intent.id,
                    ticket_id = %intent.ticket_id,
                    "skipping stalled workflow intent on recovery"
                );
                let _ = self.ctx.workflow_repo.delete_intent(&intent.id).await.map_err(
                    |e| error!(error = %e, intent_id = %intent.id, "failed to delete stalled intent"),
                );
                continue;
            }

            self.spawn_handler_task(intent.ticket_id, intent.target_status);
        }
    }

    /// Main run loop: receive transition requests and dispatch handler tasks.
    async fn run(&mut self, mut shutdown_rx: watch::Receiver<bool>) {
        info!("workflow coordinator started");
        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("workflow coordinator shutting down");
                        return;
                    }
                }
                cancel_msg = self.cancel_rx.recv() => {
                    match cancel_msg {
                        Some(ticket_id) => self.handle_cancel(&ticket_id),
                        None => {
                            info!("workflow coordinator cancel channel closed");
                        }
                    }
                }
                Some(ticket_id) = self.completion_rx.recv() => {
                    self.handle_completion(&ticket_id).await;
                }
                msg = self.rx.recv() => {
                    match msg {
                        Some(request) => self.handle_request(request).await,
                        None => {
                            info!("workflow coordinator channel closed, shutting down");
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Process a single transition request.
    async fn handle_request(&mut self, request: TransitionRequest) {
        let ticket_id = &request.ticket_id;

        // Write intent to DB for crash recovery.
        if let Err(e) = self
            .ctx
            .workflow_repo
            .create_intent(ticket_id, request.target_status)
            .await
        {
            error!(
                error = %e,
                ticket_id = %ticket_id,
                target = %request.target_status,
                "failed to write workflow intent"
            );
            return;
        }

        // Per-ticket serialization: if a handler is already running, store as pending.
        if let Some(slot) = self.in_flight.get_mut(ticket_id) {
            let dropped_prev = slot.pending.replace(request).is_some();
            if dropped_prev {
                let tid = slot.pending.as_ref().unwrap().ticket_id.clone();
                info!(
                    ticket_id = %tid,
                    "dropping intermediate pending request (latest wins)"
                );
            }
            return;
        }

        // No in-flight handler — spawn one.
        self.spawn_handler_task(request.ticket_id.clone(), request.target_status);
    }

    /// Handle a completed handler task: remove from in_flight and process pending.
    async fn handle_completion(&mut self, ticket_id: &str) {
        if let Some(slot) = self.in_flight.remove(ticket_id)
            && let Some(pending) = slot.pending
        {
            info!(
                ticket_id = %pending.ticket_id,
                target = %pending.target_status,
                "processing pending transition after handler completion"
            );
            self.spawn_handler_task(pending.ticket_id, pending.target_status);
        }
    }

    /// Cancel any in-flight or pending handler for a ticket.
    ///
    /// Aborts the running task and removes the ticket from `in_flight`.
    /// Database cleanup (deleting workflow and intents) is handled by the caller.
    fn handle_cancel(&mut self, ticket_id: &str) {
        if let Some(slot) = self.in_flight.remove(ticket_id) {
            slot.handle.abort();
            info!(
                ticket_id = %ticket_id,
                "cancelled in-flight workflow handler"
            );
        }
    }

    /// Spawn a handler task for a ticket and track it in `in_flight`.
    fn spawn_handler_task(&mut self, ticket_id: String, target_status: LifecycleStatus) {
        let ctx = self.ctx.clone();
        let handler = self.handlers.get(&target_status).cloned();
        let completion_ticket_id = ticket_id.clone();
        let completion_tx = self.completion_tx.clone();

        let handle = tokio::spawn(async move {
            run_handler(ctx, handler, &completion_ticket_id, target_status).await;
            // Notify coordinator that this ticket's handler is done.
            let _ = completion_tx.send(completion_ticket_id).await;
        });

        self.in_flight.insert(
            ticket_id,
            TicketSlot {
                handle,
                pending: None,
            },
        );
    }
}

/// Execute a handler for a ticket transition, managing intent lifecycle.
///
/// This runs inside a spawned task. On success, deletes the intent.
/// On failure, stalls the workflow.
async fn run_handler(
    ctx: WorkflowContext,
    handler: Option<Arc<dyn WorkflowHandler>>,
    ticket_id: &str,
    target_status: LifecycleStatus,
) {
    // Update workflow status in DB.
    if let Err(e) = ctx
        .workflow_repo
        .update_workflow_status(ticket_id, target_status)
        .await
    {
        // Workflow row might not exist yet — try creating it.
        if let Err(e2) = ctx
            .workflow_repo
            .create_workflow(ticket_id, target_status)
            .await
        {
            warn!(
                error = %e,
                create_error = %e2,
                ticket_id = %ticket_id,
                "failed to update or create workflow status"
            );
        }
    }

    // Sync ticket lifecycle to match workflow status. This fires a SQLite trigger
    // that creates a workflow_event, so we immediately delete it to prevent the
    // engine from re-dispatching a transition we're already handling.
    let update = ur_db::model::TicketUpdate {
        lifecycle_status: Some(target_status),
        ..Default::default()
    };
    if let Err(e) = ctx.ticket_repo.update_ticket(ticket_id, &update).await {
        warn!(
            error = %e,
            ticket_id = %ticket_id,
            "failed to sync ticket lifecycle to workflow status"
        );
    }
    let _ = ctx
        .workflow_repo
        .delete_workflow_events_for_ticket(ticket_id)
        .await;

    // Emit a workflow event for this status transition.
    emit_workflow_event(&ctx, ticket_id, target_status).await;

    let handler = match handler {
        Some(h) => h,
        None => {
            warn!(
                ticket_id = %ticket_id,
                target = %target_status,
                "no handler registered for target status — cleaning up intent"
            );
            cleanup_intent(&ctx.workflow_repo, ticket_id, target_status).await;
            return;
        }
    };

    match handler.handle(&ctx, ticket_id).await {
        Ok(()) => {
            info!(
                ticket_id = %ticket_id,
                target = %target_status,
                "workflow handler completed successfully"
            );
            cleanup_intent(&ctx.workflow_repo, ticket_id, target_status).await;
        }
        Err(handler_err) => {
            handle_failure(&ctx.workflow_repo, ticket_id, target_status, handler_err).await;
        }
    }
}

/// Emit a workflow event for a status transition.
///
/// Looks up the workflow for the ticket and inserts a workflow_events row
/// using the lifecycle constant matching the target status.
async fn emit_workflow_event(
    ctx: &WorkflowContext,
    ticket_id: &str,
    target_status: LifecycleStatus,
) {
    let workflow = match ctx.workflow_repo.get_workflow_by_ticket(ticket_id).await {
        Ok(Some(wf)) => wf,
        Ok(None) => {
            warn!(
                ticket_id = %ticket_id,
                "no workflow found when emitting workflow event"
            );
            return;
        }
        Err(e) => {
            error!(
                error = %e,
                ticket_id = %ticket_id,
                "failed to fetch workflow for event emission"
            );
            return;
        }
    };

    let event = lifecycle_status_to_event(target_status);

    if let Err(e) = ctx
        .workflow_repo
        .insert_workflow_event(&workflow.id, event)
        .await
    {
        error!(
            error = %e,
            ticket_id = %ticket_id,
            event = %event,
            "failed to insert workflow event"
        );
    }
}

/// Map a `LifecycleStatus` to its corresponding `WorkflowEvent` variant.
fn lifecycle_status_to_event(status: LifecycleStatus) -> ur_rpc::workflow_event::WorkflowEvent {
    use ur_rpc::workflow_event::WorkflowEvent;
    match status {
        LifecycleStatus::AwaitingDispatch => WorkflowEvent::AwaitingDispatch,
        LifecycleStatus::Implementing => WorkflowEvent::Implementing,
        LifecycleStatus::Verifying => WorkflowEvent::Verifying,
        LifecycleStatus::Pushing => WorkflowEvent::Pushing,
        LifecycleStatus::InReview => WorkflowEvent::InReview,
        LifecycleStatus::AddressingFeedback => WorkflowEvent::AddressingFeedback,
        LifecycleStatus::Merging => WorkflowEvent::Merging,
        LifecycleStatus::Done => WorkflowEvent::Done,
        LifecycleStatus::Cancelled => WorkflowEvent::Cancelled,
        // Open and Design are not workflow transition events — use awaiting_dispatch as fallback.
        LifecycleStatus::Open | LifecycleStatus::Design => WorkflowEvent::AwaitingDispatch,
    }
}

/// Delete the intent for a specific ticket and target status after processing.
///
/// Only deletes the intent matching both `ticket_id` and `target_status`,
/// preserving intents for subsequent transitions that may have been queued
/// while this handler was running (crash recovery depends on these).
async fn cleanup_intent(
    workflow_repo: &WorkflowRepo,
    ticket_id: &str,
    target_status: LifecycleStatus,
) {
    let intents = match workflow_repo.list_intents().await {
        Ok(i) => i,
        Err(e) => {
            error!(error = %e, "failed to list intents for cleanup");
            return;
        }
    };

    for intent in intents {
        if intent.ticket_id == ticket_id
            && intent.target_status == target_status
            && let Err(e) = workflow_repo.delete_intent(&intent.id).await
        {
            error!(error = %e, intent_id = %intent.id, "failed to delete intent");
        }
    }
}

/// Handle a failed handler execution: stall the workflow and clean up the intent.
async fn handle_failure(
    workflow_repo: &WorkflowRepo,
    ticket_id: &str,
    target_status: LifecycleStatus,
    handler_err: anyhow::Error,
) {
    error!(
        ticket_id = %ticket_id,
        target = %target_status,
        error = %handler_err,
        "workflow handler failed — stalling workflow"
    );

    if let Err(e) = workflow_repo
        .set_workflow_stalled(ticket_id, &format!("{handler_err}"))
        .await
    {
        error!(error = %e, "failed to set workflow stalled");
    }

    cleanup_intent(workflow_repo, ticket_id, target_status).await;
}

/// Create a clonable sender for submitting transition requests.
pub fn channel(
    buffer: usize,
) -> (
    mpsc::Sender<TransitionRequest>,
    mpsc::Receiver<TransitionRequest>,
) {
    mpsc::channel(buffer)
}

/// Create a channel for sending workflow cancellation requests.
///
/// The sender is cloned into gRPC handlers; the receiver goes to the coordinator.
pub fn cancel_channel(buffer: usize) -> (mpsc::Sender<String>, mpsc::Receiver<String>) {
    mpsc::channel(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use ur_db::model::{LifecycleStatus, NewTicket};
    use ur_db::{GraphManager, TicketRepo, WorkerRepo, WorkflowRepo};
    use ur_db_test::TestDb;

    async fn setup_test_db() -> (TestDb, TicketRepo, WorkflowRepo, WorkerRepo) {
        let test_db = TestDb::new().await;
        let pool = test_db.db().pool().clone();
        let graph_manager = GraphManager::new(pool.clone());
        let repo = TicketRepo::new(pool.clone(), graph_manager);
        let workflow_repo = WorkflowRepo::new(pool.clone());
        let worker_repo = WorkerRepo::new(pool);
        (test_db, repo, workflow_repo, worker_repo)
    }

    fn dummy_builderd_client() -> ur_rpc::proto::builder::BuilderdClient {
        let channel =
            tonic::transport::Endpoint::from_static("http://localhost:50051").connect_lazy();
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
        })
    }

    fn dummy_worker_manager(worker_repo: WorkerRepo) -> crate::WorkerManager {
        let builderd_client = dummy_builderd_client();
        let config = dummy_config();
        let local_repo = local_repo::GitBackend {
            client: builderd_client.clone(),
        };
        let project_registry = crate::ProjectRegistry::new(
            config.projects.clone(),
            crate::hostexec::HostExecConfigManager::empty(),
        );
        let pool = crate::RepoPoolManager::new(
            &config,
            std::path::PathBuf::from("/tmp/test/workspace"),
            std::path::PathBuf::from("/tmp/test/workspace"),
            builderd_client,
            local_repo,
            worker_repo.clone(),
            std::path::PathBuf::from("/tmp/test/config"),
            project_registry,
        );
        let network_manager = container::NetworkManager::new("docker".into(), "ur-workers".into());
        crate::WorkerManager::new(
            std::path::PathBuf::from("/tmp/test/workspace"),
            std::path::PathBuf::from("/tmp/test"),
            std::path::PathBuf::from("/tmp/test/logs"),
            std::path::PathBuf::from("/tmp/test/logs"),
            pool,
            network_manager,
            config.network.clone(),
            config.worker_port,
            Default::default(),
            worker_repo,
        )
    }

    fn make_ctx(
        ticket_repo: TicketRepo,
        workflow_repo: WorkflowRepo,
        worker_repo: WorkerRepo,
    ) -> WorkflowContext {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let worker_manager = dummy_worker_manager(worker_repo.clone());
        WorkflowContext {
            ticket_repo,
            workflow_repo,
            worker_repo,
            worker_prefix: "ur-worker-".to_string(),
            builderd_client: dummy_builderd_client(),
            config: dummy_config(),
            transition_tx: tx,
            worker_manager,
        }
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

    async fn create_test_ticket(repo: &TicketRepo, id: &str) {
        let ticket = NewTicket {
            id: Some(id.to_string()),
            project: "ur".to_string(),
            type_: "code".to_string(),
            priority: 2,
            title: "Test ticket".to_string(),
            body: String::new(),
            lifecycle_status: Some(LifecycleStatus::Open),
            ..Default::default()
        };
        repo.create_ticket(&ticket).await.unwrap();
    }

    /// Poll a sync condition until it returns true, with a timeout. Avoids flaky
    /// fixed-duration sleeps when waiting for spawned handler tasks to complete.
    async fn poll_until(mut f: impl FnMut() -> bool, timeout_ms: u64) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        while tokio::time::Instant::now() < deadline {
            if f() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("poll_until timed out after {timeout_ms}ms");
    }

    /// Poll until a workflow is marked as stalled.
    async fn poll_until_stalled(repo: &WorkflowRepo, ticket_id: &str, timeout_ms: u64) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        while tokio::time::Instant::now() < deadline {
            let stalled = repo
                .get_workflow_by_ticket(ticket_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|wf| wf.stalled);
            if stalled {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("workflow {ticket_id} not stalled after {timeout_ms}ms");
    }

    async fn poll_until_intents_empty(repo: &WorkflowRepo, timeout_ms: u64) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        while tokio::time::Instant::now() < deadline {
            let intents = repo.list_intents().await.unwrap();
            if intents.is_empty() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let intents = repo.list_intents().await.unwrap();
        assert!(
            intents.is_empty(),
            "intents should be cleaned up within {timeout_ms}ms"
        );
    }

    #[tokio::test]
    async fn coordinator_processes_request_and_cleans_intent() {
        let (_test_db, repo, workflow_repo, worker_repo) = setup_test_db().await;
        create_test_ticket(&repo, "ur-coord1").await;

        let call_count = Arc::new(AtomicU32::new(0));
        let (tx, rx) = channel(16);
        let ctx = make_ctx(repo.clone(), workflow_repo.clone(), worker_repo);

        let handlers: Vec<HandlerEntry> = vec![(
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: false,
            }) as Arc<dyn WorkflowHandler>,
        )];

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (_cancel_tx, cancel_rx) = cancel_channel(16);
        let coordinator = WorkflowCoordinator::new(rx, cancel_rx, ctx, &handlers);
        let join = coordinator.spawn(shutdown_rx);

        tx.send(TransitionRequest {
            ticket_id: "ur-coord1".to_string(),
            target_status: LifecycleStatus::Implementing,
        })
        .await
        .unwrap();

        // Wait for the handler to run.
        poll_until(|| call_count.load(Ordering::SeqCst) >= 1, 5000).await;

        // Intent should be cleaned up (poll to account for async cleanup delay).
        poll_until_intents_empty(&workflow_repo, 5000).await;

        shutdown_tx.send(true).unwrap();
        join.await.unwrap();
    }

    #[tokio::test]
    async fn coordinator_recovers_intents_on_startup() {
        let (_test_db, repo, workflow_repo, worker_repo) = setup_test_db().await;
        create_test_ticket(&repo, "ur-recov1").await;

        // Pre-create an intent to simulate crash recovery.
        workflow_repo
            .create_intent("ur-recov1", LifecycleStatus::Implementing)
            .await
            .unwrap();

        let call_count = Arc::new(AtomicU32::new(0));
        let (_tx, rx) = channel(16);
        let ctx = make_ctx(repo.clone(), workflow_repo.clone(), worker_repo);

        let handlers: Vec<HandlerEntry> = vec![(
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: false,
            }) as Arc<dyn WorkflowHandler>,
        )];

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (_cancel_tx, cancel_rx) = cancel_channel(16);
        let coordinator = WorkflowCoordinator::new(rx, cancel_rx, ctx, &handlers);
        let join = coordinator.spawn(shutdown_rx);

        // Wait for recovery to trigger the handler.
        poll_until(|| call_count.load(Ordering::SeqCst) >= 1, 5000).await;

        shutdown_tx.send(true).unwrap();
        join.await.unwrap();
    }

    #[tokio::test]
    async fn coordinator_skips_stalled_workflow_on_recovery() {
        let (_test_db, repo, workflow_repo, worker_repo) = setup_test_db().await;
        create_test_ticket(&repo, "ur-stall1").await;

        // Pre-create a workflow and mark it stalled.
        workflow_repo
            .create_workflow("ur-stall1", LifecycleStatus::Implementing)
            .await
            .unwrap();
        workflow_repo
            .set_workflow_stalled("ur-stall1", "previous failure")
            .await
            .unwrap();

        // Pre-create an intent to simulate crash recovery.
        workflow_repo
            .create_intent("ur-stall1", LifecycleStatus::Implementing)
            .await
            .unwrap();

        let call_count = Arc::new(AtomicU32::new(0));
        let (_tx, rx) = channel(16);
        let ctx = make_ctx(repo.clone(), workflow_repo.clone(), worker_repo);

        let handlers: Vec<HandlerEntry> = vec![(
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: false,
            }) as Arc<dyn WorkflowHandler>,
        )];

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (_cancel_tx, cancel_rx) = cancel_channel(16);
        let coordinator = WorkflowCoordinator::new(rx, cancel_rx, ctx, &handlers);
        let join = coordinator.spawn(shutdown_rx);

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Handler should NOT be called — workflow is stalled.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "stalled workflow intent should not trigger handler"
        );

        // Intent should be deleted.
        let intents = workflow_repo.list_intents().await.unwrap();
        assert!(intents.is_empty(), "stalled intent should be deleted");

        // Workflow should still be stalled.
        let wf = workflow_repo
            .get_workflow_by_ticket("ur-stall1")
            .await
            .unwrap()
            .unwrap();
        assert!(wf.stalled, "workflow should remain stalled");
        assert_eq!(wf.stall_reason, "previous failure");

        shutdown_tx.send(true).unwrap();
        join.await.unwrap();
    }

    #[tokio::test]
    async fn coordinator_processes_pending_after_first_handler_completes() {
        let (_test_db, repo, workflow_repo, worker_repo) = setup_test_db().await;
        create_test_ticket(&repo, "ur-pend1").await;

        let dispatch_count = Arc::new(AtomicU32::new(0));
        let implement_count = Arc::new(AtomicU32::new(0));

        let (tx, rx) = channel(16);
        let ctx = make_ctx(repo.clone(), workflow_repo, worker_repo);

        let handlers: Vec<HandlerEntry> = vec![
            (
                LifecycleStatus::AwaitingDispatch,
                Arc::new(CountingHandler {
                    call_count: dispatch_count.clone(),
                    should_fail: false,
                }) as Arc<dyn WorkflowHandler>,
            ),
            (
                LifecycleStatus::Implementing,
                Arc::new(CountingHandler {
                    call_count: implement_count.clone(),
                    should_fail: false,
                }) as Arc<dyn WorkflowHandler>,
            ),
        ];

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (_cancel_tx, cancel_rx) = cancel_channel(16);
        let coordinator = WorkflowCoordinator::new(rx, cancel_rx, ctx, &handlers);
        let join = coordinator.spawn(shutdown_rx);

        // Send both transitions back-to-back: the second should be queued
        // as pending and processed after the first completes.
        tx.send(TransitionRequest {
            ticket_id: "ur-pend1".to_string(),
            target_status: LifecycleStatus::AwaitingDispatch,
        })
        .await
        .unwrap();

        tx.send(TransitionRequest {
            ticket_id: "ur-pend1".to_string(),
            target_status: LifecycleStatus::Implementing,
        })
        .await
        .unwrap();

        // Wait for both handlers to complete (first finishes, pending dequeued).
        poll_until(
            || {
                dispatch_count.load(Ordering::SeqCst) >= 1
                    && implement_count.load(Ordering::SeqCst) >= 1
            },
            5000,
        )
        .await;

        shutdown_tx.send(true).unwrap();
        join.await.unwrap();
    }

    #[tokio::test]
    async fn coordinator_stalls_workflow_on_handler_failure() {
        let (_test_db, repo, workflow_repo, worker_repo) = setup_test_db().await;
        create_test_ticket(&repo, "ur-fail1").await;

        // Create a workflow row so set_workflow_stalled has something to update.
        workflow_repo
            .create_workflow("ur-fail1", LifecycleStatus::Implementing)
            .await
            .unwrap();

        let call_count = Arc::new(AtomicU32::new(0));
        let (tx, rx) = channel(16);
        let ctx = make_ctx(repo.clone(), workflow_repo.clone(), worker_repo);

        let handlers: Vec<HandlerEntry> = vec![(
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: true,
            }) as Arc<dyn WorkflowHandler>,
        )];

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (_cancel_tx, cancel_rx) = cancel_channel(16);
        let coordinator = WorkflowCoordinator::new(rx, cancel_rx, ctx, &handlers);
        let join = coordinator.spawn(shutdown_rx);

        tx.send(TransitionRequest {
            ticket_id: "ur-fail1".to_string(),
            target_status: LifecycleStatus::Implementing,
        })
        .await
        .unwrap();

        // Wait for the handler to run and the workflow to be stalled.
        poll_until(|| call_count.load(Ordering::SeqCst) >= 1, 5000).await;

        poll_until_stalled(&workflow_repo, "ur-fail1", 5000).await;

        // Workflow should be stalled with the error message.
        let wf = workflow_repo
            .get_workflow_by_ticket("ur-fail1")
            .await
            .unwrap()
            .unwrap();
        assert!(wf.stalled, "workflow should be stalled after failure");
        assert_eq!(wf.stall_reason, "intentional test failure");

        // Intent should be cleaned up (poll to account for async cleanup delay).
        poll_until_intents_empty(&workflow_repo, 5000).await;

        shutdown_tx.send(true).unwrap();
        join.await.unwrap();
    }

    #[tokio::test]
    async fn cleanup_intent_preserves_intents_for_other_statuses() {
        let (_test_db, repo, workflow_repo, _worker_repo) = setup_test_db().await;
        create_test_ticket(&repo, "ur-race1").await;

        // Simulate the race: two intents exist for the same ticket but different statuses.
        // This happens when a handler (e.g., verifying) sends a follow-up transition
        // (e.g., pushing) before its own cleanup runs.
        workflow_repo
            .create_intent("ur-race1", LifecycleStatus::Verifying)
            .await
            .unwrap();
        workflow_repo
            .create_intent("ur-race1", LifecycleStatus::Pushing)
            .await
            .unwrap();

        // Cleaning up the verifying intent should NOT delete the pushing intent.
        cleanup_intent(&workflow_repo, "ur-race1", LifecycleStatus::Verifying).await;

        let remaining = workflow_repo.list_intents().await.unwrap();
        assert_eq!(remaining.len(), 1, "pushing intent should survive cleanup");
        assert_eq!(remaining[0].target_status, LifecycleStatus::Pushing);
    }
}
