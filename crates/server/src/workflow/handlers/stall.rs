use std::future::Future;
use std::pin::Pin;

use tracing::{info, warn};

use crate::workflow::{TransitionKey, WorkflowContext, WorkflowHandler};

/// Wildcard handler for any transition into the Stalled state.
///
/// Sets `stall_reason` metadata on the ticket and logs the event.
/// No automated recovery — a human uses `ur admin redrive` to unstall.
pub struct StallHandler;

impl WorkflowHandler for StallHandler {
    fn handle(
        &self,
        ctx: &WorkflowContext,
        ticket_id: &str,
        transition: &TransitionKey,
    ) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + '_>> {
        let ctx = ctx.clone();
        let ticket_id = ticket_id.to_owned();
        let reason = format!(
            "Stalled during transition {} → {}",
            transition.from, transition.to
        );
        Box::pin(async move {
            info!(
                ticket_id = %ticket_id,
                from = %reason,
                "ticket stalled — manual redrive required"
            );

            if let Err(e) = ctx
                .ticket_repo
                .set_meta(&ticket_id, "ticket", "stall_reason", &reason)
                .await
            {
                warn!(
                    ticket_id = %ticket_id,
                    error = %e,
                    "failed to set stall_reason metadata"
                );
                return Err(e.into());
            }

            Ok(())
        })
    }
}
