mod coordinator;
mod engine;
pub mod github_poller;
pub mod handlers;
mod step_router;

pub use coordinator::{TransitionRequest, WorkflowCoordinator, channel as coordinator_channel};
pub use engine::WorkflowEngine;
pub use github_poller::GithubPollerManager;
pub use step_router::{LifecycleStepRouter, StepAction};

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;

use ur_config::Config;
use ur_db::TicketRepo;
use ur_db::WorkerRepo;
use ur_db::model::LifecycleStatus;
use ur_rpc::proto::builder::BuilderdClient;

/// Result future returned by `WorkflowHandler::handle()`.
pub type HandlerFuture<'a> = Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + 'a>>;

/// Context passed to every workflow handler, providing access to shared
/// managers and repositories needed to execute transitions.
#[derive(Clone)]
pub struct WorkflowContext {
    pub ticket_repo: TicketRepo,
    pub worker_repo: WorkerRepo,
    /// Docker container name prefix for workers (e.g., `ur-worker-`).
    /// Used to derive workerd gRPC addresses from process IDs.
    pub worker_prefix: String,
    /// Pre-connected builderd gRPC client for delegating operations
    /// (e.g., `gh` commands) that require host-side credentials.
    pub builderd_client: BuilderdClient,
    /// Resolved server configuration, providing access to per-project
    /// settings (workflow hooks, fix attempt limits, etc.).
    pub config: Arc<Config>,
    /// Channel sender for submitting transition requests to the
    /// WorkflowCoordinator. Handlers use this instead of directly
    /// updating lifecycle_status in the database.
    pub transition_tx: mpsc::Sender<TransitionRequest>,
}

/// Trait for handling a lifecycle state entry.
///
/// Each handler is keyed by the target `LifecycleStatus` it handles.
/// Implementations perform side effects (e.g., launching a worker, creating a
/// PR) and return `Ok(())` on success. The engine deletes the event on success
/// and increments attempts on failure.
pub trait WorkflowHandler: Send + Sync {
    fn handle(&self, ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_>;
}

/// A handler registration entry: `(target_status, handler)`.
pub type HandlerEntry = (LifecycleStatus, Arc<dyn WorkflowHandler>);

/// Shared dispatcher that can trigger lifecycle handlers directly (without events).
/// Used by the redrive endpoint to re-execute a handler for a given target status.
#[derive(Clone)]
pub struct WorkflowDispatcher {
    ctx: WorkflowContext,
    handlers: Arc<HashMap<LifecycleStatus, Arc<dyn WorkflowHandler>>>,
}

impl WorkflowDispatcher {
    pub fn new(ctx: WorkflowContext, handler_entries: &[HandlerEntry]) -> Self {
        let mut handlers = HashMap::new();
        for (target, handler) in handler_entries {
            handlers.insert(*target, handler.clone());
        }
        Self {
            ctx,
            handlers: Arc::new(handlers),
        }
    }

    /// Execute the handler registered for `target_status`.
    /// "Redrive to verifying" runs the VerifyHandler, not the PushHandler.
    pub async fn trigger(
        &self,
        ticket_id: &str,
        target_status: LifecycleStatus,
    ) -> Result<LifecycleStatus, anyhow::Error> {
        let handler = self
            .handlers
            .get(&target_status)
            .ok_or_else(|| anyhow::anyhow!("no handler registered for '{target_status}'"))?;
        handler.handle(&self.ctx, ticket_id).await?;
        Ok(target_status)
    }
}
