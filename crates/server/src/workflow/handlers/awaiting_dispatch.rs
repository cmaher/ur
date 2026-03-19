use tracing::info;

use crate::workflow::{HandlerFuture, TransitionKey, WorkflowContext, WorkflowHandler};

/// No-op handler for the Open -> AwaitingDispatch transition.
///
/// The transition itself is the meaningful signal (the CLI has dispatched
/// the ticket and a worker is being assigned). The engine acknowledges
/// the event and deletes it — no further action is needed until the
/// worker reports idle and triggers AwaitingDispatch -> Implementing.
pub struct AwaitingDispatchHandler;

impl WorkflowHandler for AwaitingDispatchHandler {
    fn handle(
        &self,
        _ctx: &WorkflowContext,
        ticket_id: &str,
        _transition: &TransitionKey,
    ) -> HandlerFuture<'_> {
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move {
            info!(
                ticket_id = %ticket_id,
                "ticket now awaiting dispatch — worker assignment in progress"
            );
            Ok(())
        })
    }
}
