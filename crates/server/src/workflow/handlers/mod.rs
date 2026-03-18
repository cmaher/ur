// Handler registry for workflow transitions.

mod dispatch_implement;
mod dispatch_push;
mod feedback_create;
mod feedback_resolve;
mod review_start;
mod verify;

pub use dispatch_implement::DispatchImplementHandler;
pub use dispatch_push::DispatchPushHandler;
pub use feedback_create::FeedbackCreateHandler;
pub use feedback_resolve::FeedbackResolveHandler;
pub use review_start::ReviewStartHandler;
pub use verify::VerifyHandler;

use std::sync::Arc;

use ur_db::model::LifecycleStatus;

use super::HandlerEntry;

/// Build the list of all workflow handler registrations.
///
/// Returns a `Vec<HandlerEntry>` to be passed to `WorkflowEngine::new()`.
pub fn build_handlers() -> Vec<HandlerEntry> {
    vec![
        // Open → Implementing: dispatch worker with implement RPC
        (
            LifecycleStatus::Open,
            LifecycleStatus::Implementing,
            Arc::new(DispatchImplementHandler),
        ),
        // Implementing → Verifying: run pre-push verification hook
        (
            LifecycleStatus::Implementing,
            LifecycleStatus::Verifying,
            Arc::new(VerifyHandler),
        ),
        // Fixing → Verifying: re-run pre-push verification hook after fix
        (
            LifecycleStatus::Fixing,
            LifecycleStatus::Verifying,
            Arc::new(VerifyHandler),
        ),
        // Verifying → Pushing: dispatch push RPC to worker
        (
            LifecycleStatus::Verifying,
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
    ]
}
