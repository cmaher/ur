mod engine;
pub mod github_poller;
pub mod handlers;
mod step_router;

pub use engine::WorkflowEngine;
pub use github_poller::GithubPollerManager;
pub use step_router::{LifecycleStepRouter, StepAction};

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
