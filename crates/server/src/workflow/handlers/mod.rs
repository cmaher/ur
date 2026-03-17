// Handler registry for workflow transitions.

mod review_start;
mod stall;

pub use review_start::ReviewStartHandler;
pub use stall::StallHandler;

use std::sync::Arc;

use ur_db::model::LifecycleStatus;

use super::WorkflowEngine;

/// Register all workflow handlers with the engine.
///
/// Called once during engine setup to wire transitions to their handlers.
pub fn register_all(engine: &mut WorkflowEngine) {
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
