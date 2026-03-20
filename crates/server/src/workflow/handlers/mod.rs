// Handler registry for workflow transitions.

mod awaiting_dispatch;
mod dispatch_implement;
mod feedback_create;
mod merge;
mod push;
mod review_start;
mod verify;

pub use awaiting_dispatch::AwaitingDispatchHandler;
pub use dispatch_implement::DispatchImplementHandler;
pub use feedback_create::FeedbackCreateHandler;
pub use merge::MergeHandler;
pub use push::PushHandler;
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
        // Open → AwaitingDispatch: no-op (acknowledge and delete event)
        (
            LifecycleStatus::Open,
            LifecycleStatus::AwaitingDispatch,
            Arc::new(AwaitingDispatchHandler),
        ),
        // AwaitingDispatch → Implementing: dispatch worker with implement RPC
        (
            LifecycleStatus::AwaitingDispatch,
            LifecycleStatus::Implementing,
            Arc::new(DispatchImplementHandler),
        ),
        // Open → Verifying: direct verification for shipped work (skips dispatch)
        (
            LifecycleStatus::Open,
            LifecycleStatus::Verifying,
            Arc::new(VerifyHandler),
        ),
        // Implementing → Verifying: run pre-push verification hook
        (
            LifecycleStatus::Implementing,
            LifecycleStatus::Verifying,
            Arc::new(VerifyHandler),
        ),
        // Verifying → Pushing: workflow-driven push handler
        (
            LifecycleStatus::Verifying,
            LifecycleStatus::Pushing,
            Arc::new(PushHandler),
        ),
        // InReview → FeedbackCreating: dispatch feedback create RPC to worker
        (
            LifecycleStatus::InReview,
            LifecycleStatus::FeedbackCreating,
            Arc::new(FeedbackCreateHandler),
        ),
        // FeedbackCreating → Merging: merge PR (squash), kill worker, close epic, dispatch children
        (
            LifecycleStatus::FeedbackCreating,
            LifecycleStatus::Merging,
            Arc::new(MergeHandler),
        ),
        // Pushing → Implementing: CI failure detected by GitHub poller
        (
            LifecycleStatus::Pushing,
            LifecycleStatus::Implementing,
            Arc::new(DispatchImplementHandler),
        ),
        // Merging → Implementing: merge conflict during PR merge
        (
            LifecycleStatus::Merging,
            LifecycleStatus::Implementing,
            Arc::new(DispatchImplementHandler),
        ),
        // Pushing → InReview: no-op signal handler
        (
            LifecycleStatus::Pushing,
            LifecycleStatus::InReview,
            Arc::new(ReviewStartHandler),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_to_verifying_transition_registered() {
        let handlers = build_handlers();
        let found = handlers.iter().any(|(from, to, _)| {
            *from == LifecycleStatus::Open && *to == LifecycleStatus::Verifying
        });
        assert!(
            found,
            "Open → Verifying transition should be registered in build_handlers()"
        );
    }
}
