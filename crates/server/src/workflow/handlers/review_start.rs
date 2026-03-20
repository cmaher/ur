use tracing::info;

use crate::workflow::{HandlerFuture, WorkflowContext, WorkflowHandler};

/// No-op handler for the Pushing → InReview transition.
///
/// The transition itself is the meaningful signal (e.g., a PR was created).
/// Future iterations may add TUI/dashboard notifications here.
pub struct ReviewStartHandler;

impl WorkflowHandler for ReviewStartHandler {
    fn handle(&self, _ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move {
            info!(
                ticket_id = %ticket_id,
                "review started — transition is the signal, no further action required"
            );
            Ok(())
        })
    }
}
