use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use ur_db::TicketRepo;
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
    /// Kept to allow future cancellation or join-on-shutdown.
    _handle: JoinHandle<()>,
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
    ctx: WorkflowContext,
    handlers: HashMap<LifecycleStatus, Arc<dyn WorkflowHandler>>,
    in_flight: HashMap<String, TicketSlot>,
    max_attempts: i32,
}

impl WorkflowCoordinator {
    pub fn new(
        rx: mpsc::Receiver<TransitionRequest>,
        ctx: WorkflowContext,
        handler_entries: &[HandlerEntry],
        max_attempts: i32,
    ) -> Self {
        let mut handlers = HashMap::new();
        for (target, handler) in handler_entries {
            handlers.insert(*target, handler.clone());
        }
        Self {
            rx,
            ctx,
            handlers,
            in_flight: HashMap::new(),
            max_attempts,
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
        let intents = match self.ctx.ticket_repo.list_intents().await {
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
            if intent.attempts >= self.max_attempts {
                warn!(
                    intent_id = %intent.id,
                    ticket_id = %intent.ticket_id,
                    attempts = intent.attempts,
                    "skipping stalled intent (max attempts reached)"
                );
                self.stall_and_delete(
                    &intent.ticket_id,
                    &intent.id,
                    "max attempts reached on recovery",
                )
                .await;
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
            .ticket_repo
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

    /// Spawn a handler task for a ticket and track it in `in_flight`.
    fn spawn_handler_task(&mut self, ticket_id: String, target_status: LifecycleStatus) {
        let ctx = self.ctx.clone();
        let handler = self.handlers.get(&target_status).cloned();
        let max_attempts = self.max_attempts;
        let completion_ticket_id = ticket_id.clone();

        let handle = tokio::spawn(async move {
            run_handler(
                ctx,
                handler,
                &completion_ticket_id,
                target_status,
                max_attempts,
            )
            .await;
        });

        self.in_flight.insert(
            ticket_id,
            TicketSlot {
                _handle: handle,
                pending: None,
            },
        );
    }

    /// Stall a ticket and delete its intent.
    async fn stall_and_delete(&self, ticket_id: &str, intent_id: &str, reason: &str) {
        if let Err(e) = self
            .ctx
            .ticket_repo
            .set_meta(ticket_id, "ticket", "stall_reason", reason)
            .await
        {
            error!(error = %e, "failed to set stall_reason metadata");
        }
        if let Err(e) = self.ctx.ticket_repo.delete_intent(intent_id).await {
            error!(error = %e, intent_id = %intent_id, "failed to delete stalled intent");
        }
    }
}

/// Execute a handler for a ticket transition, managing intent lifecycle.
///
/// This runs inside a spawned task. On success, deletes the intent.
/// On failure, increments attempts and stalls if max is reached.
async fn run_handler(
    ctx: WorkflowContext,
    handler: Option<Arc<dyn WorkflowHandler>>,
    ticket_id: &str,
    target_status: LifecycleStatus,
    max_attempts: i32,
) {
    // Update workflow status in DB.
    if let Err(e) = ctx
        .ticket_repo
        .update_workflow_status(ticket_id, target_status)
        .await
    {
        // Workflow row might not exist yet — try creating it.
        if let Err(e2) = ctx
            .ticket_repo
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

    let handler = match handler {
        Some(h) => h,
        None => {
            warn!(
                ticket_id = %ticket_id,
                target = %target_status,
                "no handler registered for target status — cleaning up intent"
            );
            cleanup_intent(&ctx.ticket_repo, ticket_id).await;
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
            cleanup_intent(&ctx.ticket_repo, ticket_id).await;
        }
        Err(handler_err) => {
            handle_failure(
                &ctx.ticket_repo,
                ticket_id,
                target_status,
                handler_err,
                max_attempts,
            )
            .await;
        }
    }
}

/// Delete all intents for a ticket after successful processing.
async fn cleanup_intent(ticket_repo: &TicketRepo, ticket_id: &str) {
    let intents = match ticket_repo.list_intents().await {
        Ok(i) => i,
        Err(e) => {
            error!(error = %e, "failed to list intents for cleanup");
            return;
        }
    };

    for intent in intents {
        if intent.ticket_id == ticket_id
            && let Err(e) = ticket_repo.delete_intent(&intent.id).await
        {
            error!(error = %e, intent_id = %intent.id, "failed to delete intent");
        }
    }
}

/// Handle a failed handler execution: increment attempts, stall if over max.
async fn handle_failure(
    ticket_repo: &TicketRepo,
    ticket_id: &str,
    target_status: LifecycleStatus,
    handler_err: anyhow::Error,
    max_attempts: i32,
) {
    let intents = match ticket_repo.list_intents().await {
        Ok(i) => i,
        Err(e) => {
            error!(error = %e, "failed to list intents for failure handling");
            return;
        }
    };

    let intent = intents.into_iter().find(|i| i.ticket_id == ticket_id);
    let intent = match intent {
        Some(i) => i,
        None => {
            error!(ticket_id = %ticket_id, "no intent found for failed handler");
            return;
        }
    };

    let new_attempts = intent.attempts + 1;

    if new_attempts >= max_attempts {
        error!(
            ticket_id = %ticket_id,
            target = %target_status,
            attempts = new_attempts,
            error = %handler_err,
            "workflow handler failed after max attempts — stalling"
        );
        if let Err(e) = ticket_repo
            .set_meta(
                ticket_id,
                "ticket",
                "stall_reason",
                &format!("{handler_err}"),
            )
            .await
        {
            error!(error = %e, "failed to set stall_reason metadata");
        }
        if let Err(e) = ticket_repo.delete_intent(&intent.id).await {
            error!(error = %e, "failed to delete stalled intent");
        }
    } else {
        warn!(
            ticket_id = %ticket_id,
            target = %target_status,
            attempts = new_attempts,
            error = %handler_err,
            "workflow handler failed — will retry"
        );
        if let Err(e) = ticket_repo.increment_intent_attempts(&intent.id).await {
            error!(error = %e, "failed to increment intent attempts");
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tempfile::TempDir;
    use ur_db::model::{LifecycleStatus, NewTicket};
    use ur_db::{DatabaseManager, GraphManager, TicketRepo, WorkerRepo};

    async fn setup_test_db() -> (TempDir, TicketRepo, WorkerRepo) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = DatabaseManager::open(&db_path.to_string_lossy())
            .await
            .expect("open test db");
        let graph_manager = GraphManager::new(db.pool().clone());
        let repo = TicketRepo::new(db.pool().clone(), graph_manager);
        let worker_repo = WorkerRepo::new(db.pool().clone());
        (tmp, repo, worker_repo)
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
                max_transition_attempts: 3,
                poll_interval_ms: 500,
                github_scan_interval_secs: 30,
            },
            projects: std::collections::HashMap::new(),
        })
    }

    fn make_ctx(ticket_repo: TicketRepo, worker_repo: WorkerRepo) -> WorkflowContext {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        WorkflowContext {
            ticket_repo,
            worker_repo,
            worker_prefix: "ur-worker-".to_string(),
            builderd_client: dummy_builderd_client(),
            config: dummy_config(),
            transition_tx: tx,
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
            id: id.to_string(),
            project: "ur".to_string(),
            type_: "task".to_string(),
            priority: 2,
            title: "Test ticket".to_string(),
            body: String::new(),
            lifecycle_status: Some(LifecycleStatus::Open),
            ..Default::default()
        };
        repo.create_ticket(&ticket).await.unwrap();
    }

    #[tokio::test]
    async fn coordinator_processes_request_and_cleans_intent() {
        let (_tmp, repo, worker_repo) = setup_test_db().await;
        create_test_ticket(&repo, "ur-coord1").await;

        let call_count = Arc::new(AtomicU32::new(0));
        let (tx, rx) = channel(16);
        let ctx = make_ctx(repo.clone(), worker_repo);

        let handlers: Vec<HandlerEntry> = vec![(
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: false,
            }) as Arc<dyn WorkflowHandler>,
        )];

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let coordinator = WorkflowCoordinator::new(rx, ctx, &handlers, 3);
        let join = coordinator.spawn(shutdown_rx);

        tx.send(TransitionRequest {
            ticket_id: "ur-coord1".to_string(),
            target_status: LifecycleStatus::Implementing,
        })
        .await
        .unwrap();

        // Give the handler time to run.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Intent should be cleaned up.
        let intents = repo.list_intents().await.unwrap();
        assert!(
            intents.is_empty(),
            "intents should be cleaned up after success"
        );

        shutdown_tx.send(true).unwrap();
        join.await.unwrap();
    }

    #[tokio::test]
    async fn coordinator_recovers_intents_on_startup() {
        let (_tmp, repo, worker_repo) = setup_test_db().await;
        create_test_ticket(&repo, "ur-recov1").await;

        // Pre-create an intent to simulate crash recovery.
        repo.create_intent("ur-recov1", LifecycleStatus::Implementing)
            .await
            .unwrap();

        let call_count = Arc::new(AtomicU32::new(0));
        let (_tx, rx) = channel(16);
        let ctx = make_ctx(repo.clone(), worker_repo);

        let handlers: Vec<HandlerEntry> = vec![(
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: false,
            }) as Arc<dyn WorkflowHandler>,
        )];

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let coordinator = WorkflowCoordinator::new(rx, ctx, &handlers, 3);
        let join = coordinator.spawn(shutdown_rx);

        // Give recovery time to run.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "recovered intent should trigger handler"
        );

        shutdown_tx.send(true).unwrap();
        join.await.unwrap();
    }

    #[tokio::test]
    async fn coordinator_stalls_after_max_attempts_on_recovery() {
        let (_tmp, repo, worker_repo) = setup_test_db().await;
        create_test_ticket(&repo, "ur-stall1").await;

        // Pre-create an intent already at max attempts.
        let intent = repo
            .create_intent("ur-stall1", LifecycleStatus::Implementing)
            .await
            .unwrap();
        for _ in 0..3 {
            repo.increment_intent_attempts(&intent.id).await.unwrap();
        }

        let call_count = Arc::new(AtomicU32::new(0));
        let (_tx, rx) = channel(16);
        let ctx = make_ctx(repo.clone(), worker_repo);

        let handlers: Vec<HandlerEntry> = vec![(
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: false,
            }) as Arc<dyn WorkflowHandler>,
        )];

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let coordinator = WorkflowCoordinator::new(rx, ctx, &handlers, 3);
        let join = coordinator.spawn(shutdown_rx);

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Handler should NOT be called — intent was already stalled.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "stalled intent should not trigger handler"
        );

        // Intent should be deleted and stall_reason set.
        let intents = repo.list_intents().await.unwrap();
        assert!(intents.is_empty(), "stalled intent should be deleted");

        let meta = repo.get_meta("ur-stall1", "ticket").await.unwrap();
        assert!(
            meta.contains_key("stall_reason"),
            "stall_reason should be set"
        );

        shutdown_tx.send(true).unwrap();
        join.await.unwrap();
    }
}
