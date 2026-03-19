mod engine;
pub mod github_poller;
pub mod handlers;
mod step_router;

pub use engine::WorkflowEngine;
pub use github_poller::GithubPollerManager;
pub use step_router::{LifecycleStepRouter, StepAction};

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

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
}

/// Key identifying a specific lifecycle transition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransitionKey {
    pub from: LifecycleStatus,
    pub to: LifecycleStatus,
}

impl fmt::Display for TransitionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} -> {}", self.from, self.to)
    }
}

/// Trait for handling a specific lifecycle transition.
///
/// Implementations perform side effects (e.g., launching a worker, creating a
/// PR) and return `Ok(())` on success. The engine deletes the event on success
/// and increments attempts on failure.
pub trait WorkflowHandler: Send + Sync {
    fn handle(
        &self,
        ctx: &WorkflowContext,
        ticket_id: &str,
        transition: &TransitionKey,
    ) -> HandlerFuture<'_>;
}

/// A handler registration entry: `(from_status, to_status, handler)`.
pub type HandlerEntry = (LifecycleStatus, LifecycleStatus, Arc<dyn WorkflowHandler>);

/// The "happy path" forward transition for each lifecycle status.
/// Used by redrive to determine which handler to invoke.
fn natural_next(status: &LifecycleStatus) -> Option<LifecycleStatus> {
    match status {
        LifecycleStatus::Open => Some(LifecycleStatus::AwaitingDispatch),
        LifecycleStatus::AwaitingDispatch => Some(LifecycleStatus::Implementing),
        LifecycleStatus::Implementing => Some(LifecycleStatus::Verifying),
        LifecycleStatus::Fixing => Some(LifecycleStatus::Verifying),
        LifecycleStatus::Verifying => Some(LifecycleStatus::Pushing),
        LifecycleStatus::Pushing => Some(LifecycleStatus::InReview),
        LifecycleStatus::InReview => Some(LifecycleStatus::FeedbackCreating),
        LifecycleStatus::FeedbackCreating => Some(LifecycleStatus::FeedbackResolving),
        _ => None,
    }
}

/// Shared dispatcher that can trigger lifecycle handlers directly (without events).
/// Used by the redrive endpoint to re-execute a transition for a given status.
#[derive(Clone)]
pub struct WorkflowDispatcher {
    ctx: WorkflowContext,
    handlers: Arc<HashMap<TransitionKey, Arc<dyn WorkflowHandler>>>,
}

impl WorkflowDispatcher {
    pub fn new(ctx: WorkflowContext, handler_entries: &[HandlerEntry]) -> Self {
        let mut handlers = HashMap::new();
        for (from, to, handler) in handler_entries {
            let key = TransitionKey {
                from: *from,
                to: *to,
            };
            handlers.insert(key, handler.clone());
        }
        Self {
            ctx,
            handlers: Arc::new(handlers),
        }
    }

    /// Find the natural forward transition from `from_status` and execute its handler.
    /// Returns the target status on success, or an error if no handler or execution fails.
    pub async fn trigger(
        &self,
        ticket_id: &str,
        from_status: LifecycleStatus,
    ) -> Result<LifecycleStatus, anyhow::Error> {
        let to = natural_next(&from_status)
            .ok_or_else(|| anyhow::anyhow!("no natural next state from '{from_status}'"))?;
        let key = TransitionKey {
            from: from_status,
            to,
        };
        let handler = self
            .handlers
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("no handler registered for transition '{key}'"))?;
        handler.handle(&self.ctx, ticket_id, &key).await?;
        Ok(to)
    }
}
