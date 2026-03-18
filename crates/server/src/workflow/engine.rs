use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{error, info, warn};

use ur_db::TicketRepo;
use ur_db::WorkerRepo;
use ur_db::model::LifecycleStatus;

use super::{TransitionKey, WorkflowContext, WorkflowHandler};

/// Maximum number of processing attempts before an event is stalled.
const MAX_ATTEMPTS: i32 = 3;

/// Polling interval for the workflow event table.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Drives workflow transitions by polling the `workflow_event` table and
/// dispatching to registered handlers.
///
/// Implements `Clone` and follows the manager pattern: holds references to
/// dependencies injected via the constructor.
#[derive(Clone)]
pub struct WorkflowEngine {
    ctx: WorkflowContext,
    handlers: HashMap<TransitionKey, Arc<dyn WorkflowHandler>>,
}

impl WorkflowEngine {
    pub fn new(ticket_repo: TicketRepo, worker_repo: WorkerRepo, worker_prefix: String) -> Self {
        let ctx = WorkflowContext {
            ticket_repo,
            worker_repo,
            worker_prefix,
        };
        Self {
            ctx,
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a specific lifecycle transition.
    pub fn register_handler(
        &mut self,
        from: LifecycleStatus,
        to: LifecycleStatus,
        handler: Arc<dyn WorkflowHandler>,
    ) {
        let key = TransitionKey { from, to };
        self.handlers.insert(key, handler);
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
                _ = tokio::time::sleep(POLL_INTERVAL) => {
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
        let event = match self.ctx.ticket_repo.poll_workflow_event().await {
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
            if let Err(e) = self.ctx.ticket_repo.delete_workflow_event(&event.id).await {
                error!(error = %e, "failed to delete stale workflow event");
            }
            return;
        }

        let key = TransitionKey {
            from: event.old_lifecycle_status,
            to: event.new_lifecycle_status,
        };

        let handler = match self.handlers.get(&key) {
            Some(h) => h,
            None => {
                warn!(
                    event_id = %event.id,
                    transition = %key,
                    "no handler registered for transition — deleting event"
                );
                self.delete_event(&event.id).await;
                return;
            }
        };

        match handler.handle(&self.ctx, &event.ticket_id, &key).await {
            Ok(()) => self.handle_success(&event.id, &event.ticket_id, &key).await,
            Err(handler_err) => self.handle_failure(&event, &key, handler_err).await,
        }
    }

    /// Clean up after a successful handler execution.
    async fn handle_success(&self, event_id: &str, ticket_id: &str, transition: &TransitionKey) {
        info!(
            event_id = %event_id,
            ticket_id = %ticket_id,
            transition = %transition,
            "workflow transition completed successfully"
        );
        self.delete_event(event_id).await;
    }

    /// Handle a failed transition: increment attempts, stall if threshold reached.
    async fn handle_failure(
        &self,
        event: &ur_db::model::WorkflowEvent,
        transition: &TransitionKey,
        handler_err: anyhow::Error,
    ) {
        let new_attempts = event.attempts + 1;
        if new_attempts >= MAX_ATTEMPTS {
            error!(
                event_id = %event.id,
                ticket_id = %event.ticket_id,
                transition = %transition,
                attempts = new_attempts,
                error = %handler_err,
                "workflow transition stalled after max attempts"
            );
            self.mark_ticket_stalled(&event.ticket_id).await;
        } else {
            warn!(
                event_id = %event.id,
                ticket_id = %event.ticket_id,
                transition = %transition,
                attempts = new_attempts,
                error = %handler_err,
                "workflow transition failed — will retry"
            );
        }
        if let Err(e) = self
            .ctx
            .ticket_repo
            .increment_workflow_event_attempts(&event.id)
            .await
        {
            error!(error = %e, "failed to increment workflow event attempts");
        }
    }

    /// Delete a workflow event, logging any errors.
    async fn delete_event(&self, event_id: &str) {
        if let Err(e) = self.ctx.ticket_repo.delete_workflow_event(event_id).await {
            error!(error = %e, event_id = %event_id, "failed to delete workflow event");
        }
    }

    /// Set a ticket's lifecycle status to Stalled.
    async fn mark_ticket_stalled(&self, ticket_id: &str) {
        let update = ur_db::model::TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::Stalled),
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        if let Err(e) = self.ctx.ticket_repo.update_ticket(ticket_id, &update).await {
            error!(error = %e, "failed to set ticket to stalled");
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

    struct CountingHandler {
        call_count: Arc<AtomicU32>,
        should_fail: bool,
    }

    impl WorkflowHandler for CountingHandler {
        fn handle(
            &self,
            _ctx: &WorkflowContext,
            _ticket_id: &str,
            _transition: &TransitionKey,
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
        let (_tmp, repo, worker_repo) = setup_test_db().await;

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
        let event = repo.poll_workflow_event().await.unwrap();
        assert!(event.is_some(), "workflow event should exist");

        let call_count = Arc::new(AtomicU32::new(0));
        let mut engine =
            WorkflowEngine::new(repo.clone(), worker_repo.clone(), "ur-worker-".to_string());
        engine.register_handler(
            LifecycleStatus::Open,
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: false,
            }),
        );

        engine.poll_once().await;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "handler should be called once"
        );

        let event = repo.poll_workflow_event().await.unwrap();
        assert!(event.is_none(), "event should be deleted after success");
    }

    #[tokio::test]
    async fn engine_increments_attempts_on_failure() {
        let (_tmp, repo, worker_repo) = setup_test_db().await;

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

        let update = ur_db::model::TicketUpdate {
            lifecycle_status: Some(LifecycleStatus::Implementing),
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
        let mut engine =
            WorkflowEngine::new(repo.clone(), worker_repo.clone(), "ur-worker-".to_string());
        engine.register_handler(
            LifecycleStatus::Open,
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: true,
            }),
        );

        engine.poll_once().await;
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        let event = repo.poll_workflow_event().await.unwrap();
        assert!(event.is_some(), "event should still exist after failure");
        assert_eq!(event.unwrap().attempts, 1);
    }

    #[tokio::test]
    async fn engine_stalls_after_max_attempts() {
        let (_tmp, repo, worker_repo) = setup_test_db().await;

        let ticket = NewTicket {
            id: "ur-test3".to_string(),
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
            status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        repo.update_ticket("ur-test3", &update).await.unwrap();

        let call_count = Arc::new(AtomicU32::new(0));
        let mut engine =
            WorkflowEngine::new(repo.clone(), worker_repo.clone(), "ur-worker-".to_string());
        engine.register_handler(
            LifecycleStatus::Open,
            LifecycleStatus::Implementing,
            Arc::new(CountingHandler {
                call_count: call_count.clone(),
                should_fail: true,
            }),
        );

        for _ in 0..MAX_ATTEMPTS {
            engine.poll_once().await;
        }

        assert_eq!(call_count.load(Ordering::SeqCst), MAX_ATTEMPTS as u32);

        let t = repo.get_ticket("ur-test3").await.unwrap().unwrap();
        assert_eq!(t.lifecycle_status, LifecycleStatus::Stalled);
    }

    #[tokio::test]
    async fn engine_deletes_event_with_no_handler() {
        let (_tmp, repo, worker_repo) = setup_test_db().await;

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

        let engine =
            WorkflowEngine::new(repo.clone(), worker_repo.clone(), "ur-worker-".to_string());

        engine.poll_once().await;

        let event = repo.poll_workflow_event().await.unwrap();
        assert!(
            event.is_none(),
            "event should be deleted when no handler is registered"
        );
    }
}
