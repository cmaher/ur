// Handler registry for workflow transitions.

mod dispatch_implement;
mod dispatch_push;
mod feedback_create;
mod feedback_resolve;
mod review_start;
mod stall;

pub use dispatch_implement::DispatchImplementHandler;
pub use dispatch_push::DispatchPushHandler;
pub use feedback_create::FeedbackCreateHandler;
pub use feedback_resolve::FeedbackResolveHandler;
pub use review_start::ReviewStartHandler;
pub use stall::StallHandler;

use std::sync::Arc;

use ur_db::model::LifecycleStatus;

use super::HandlerEntry;

/// Build the list of all workflow handler registrations.
///
/// Returns a `Vec<HandlerEntry>` to be passed to `WorkflowEngine::new()`.
pub fn build_handlers() -> Vec<HandlerEntry> {
    let mut entries: Vec<HandlerEntry> = vec![
        // Open → Implementing: dispatch worker with implement RPC
        (
            LifecycleStatus::Open,
            LifecycleStatus::Implementing,
            Arc::new(DispatchImplementHandler),
        ),
        // Implementing → Pushing: dispatch push RPC to worker
        (
            LifecycleStatus::Implementing,
            LifecycleStatus::Pushing,
            Arc::new(DispatchPushHandler),
        ),
        // InReview → FeedbackCreating: dispatch feedback create RPC to worker
        (
            LifecycleStatus::InReview,
            LifecycleStatus::FeedbackCreating,
            Arc::new(FeedbackCreateHandler),
        ),
        // FeedbackCreating → FeedbackResolving: resolve feedback (merge or re-implement)
        (
            LifecycleStatus::FeedbackCreating,
            LifecycleStatus::FeedbackResolving,
            Arc::new(FeedbackResolveHandler),
        ),
        // Pushing → InReview: no-op signal handler
        (
            LifecycleStatus::Pushing,
            LifecycleStatus::InReview,
            Arc::new(ReviewStartHandler),
        ),
    ];

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
        entries.push((from, LifecycleStatus::Stalled, stall_handler.clone()));
    }

    entries
}
