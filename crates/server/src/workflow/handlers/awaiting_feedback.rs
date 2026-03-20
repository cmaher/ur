use tracing::info;

use crate::workflow::{HandlerFuture, WorkflowContext, WorkflowHandler};

/// No-op handler for the Pushing → AwaitingFeedback transition.
///
/// The push handler has already pushed the branch and created the PR.
/// This state exists to distinguish "push completed, waiting for CI"
/// from the earlier "actively pushing" phase. The GithubPollerManager
/// monitors tickets in this state and advances them to InReview (CI
/// green) or back to Implementing (CI failure).
pub struct AwaitingFeedbackHandler;

impl WorkflowHandler for AwaitingFeedbackHandler {
    fn handle(&self, _ctx: &WorkflowContext, ticket_id: &str) -> HandlerFuture<'_> {
        let ticket_id = ticket_id.to_owned();
        Box::pin(async move {
            info!(
                ticket_id = %ticket_id,
                "awaiting feedback — PR pushed, waiting for CI checks"
            );
            Ok(())
        })
    }
}
