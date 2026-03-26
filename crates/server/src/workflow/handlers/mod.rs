// Handler registry for workflow states.

mod awaiting_dispatch;
mod dispatch_implement;
mod feedback_address;
mod merge;
mod push;
mod review_start;
mod verify;

pub use awaiting_dispatch::AwaitingDispatchHandler;
pub use dispatch_implement::ImplementHandler;
pub use feedback_address::FeedbackAddressHandler;
pub use merge::MergeHandler;
pub use push::PushHandler;
pub use review_start::ReviewStartHandler;
pub use verify::VerifyHandler;

use std::sync::Arc;

use ur_db::model::LifecycleStatus;

use super::HandlerEntry;
use super::ticket_client::TicketClient;

/// Build the list of all workflow handler registrations.
///
/// Returns a `Vec<HandlerEntry>` keyed by target `LifecycleStatus`.
pub fn build_handlers(ticket_client: TicketClient) -> Vec<HandlerEntry> {
    vec![
        // AwaitingDispatch: no-op (acknowledge and delete event)
        (
            LifecycleStatus::AwaitingDispatch,
            Arc::new(AwaitingDispatchHandler) as Arc<dyn super::WorkflowHandler>,
        ),
        // Implementing: dispatch worker with implement RPC
        (LifecycleStatus::Implementing, Arc::new(ImplementHandler)),
        // Verifying: run pre-push verification hook
        (LifecycleStatus::Verifying, Arc::new(VerifyHandler)),
        // Pushing: workflow-driven push handler
        (LifecycleStatus::Pushing, Arc::new(PushHandler)),
        // AddressingFeedback: dispatch address feedback RPC to worker
        (
            LifecycleStatus::AddressingFeedback,
            Arc::new(FeedbackAddressHandler),
        ),
        // Merging: merge PR (squash), kill worker, close epic, dispatch children
        (
            LifecycleStatus::Merging,
            Arc::new(MergeHandler { ticket_client }),
        ),
        // InReview: no-op signal handler
        (LifecycleStatus::InReview, Arc::new(ReviewStartHandler)),
    ]
}
