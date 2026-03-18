// Handler registry for workflow transitions.

#[cfg(feature = "workerd")]
mod dispatch_implement;
#[cfg(feature = "workerd")]
mod dispatch_push;
mod review_start;
mod stall;

#[cfg(feature = "workerd")]
pub use dispatch_implement::DispatchImplementHandler;
#[cfg(feature = "workerd")]
pub use dispatch_push::DispatchPushHandler;
pub use review_start::ReviewStartHandler;
pub use stall::StallHandler;

use std::sync::Arc;

use ur_db::model::LifecycleStatus;

use super::WorkflowEngine;

/// Register all workflow handlers with the engine.
///
/// Called once during engine setup to wire transitions to their handlers.
pub fn register_all(engine: &mut WorkflowEngine) {
    // Open → Implementing: dispatch worker with implement RPC
    #[cfg(feature = "workerd")]
    engine.register_handler(
        LifecycleStatus::Open,
        LifecycleStatus::Implementing,
        Arc::new(DispatchImplementHandler),
    );

    // Implementing → Pushing: dispatch push RPC to worker
    #[cfg(feature = "workerd")]
    engine.register_handler(
        LifecycleStatus::Implementing,
        LifecycleStatus::Pushing,
        Arc::new(DispatchPushHandler),
    );

    // Pushing → InReview: no-op signal handler
    engine.register_handler(
        LifecycleStatus::Pushing,
        LifecycleStatus::InReview,
        Arc::new(ReviewStartHandler),
    );

    // * → Stalled: wildcard handler for all possible source states
    let stall_handler = Arc::new(StallHandler);
    let source_states = [
        LifecycleStatus::Design,
        LifecycleStatus::Open,
        LifecycleStatus::Implementing,
        LifecycleStatus::Pushing,
        LifecycleStatus::InReview,
        LifecycleStatus::FeedbackCreating,
        LifecycleStatus::FeedbackResolving,
        // Done → Stalled is unlikely but registered for completeness
        LifecycleStatus::Done,
    ];
    for from in source_states {
        engine.register_handler(from, LifecycleStatus::Stalled, stall_handler.clone());
    }
}
