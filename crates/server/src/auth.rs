use tonic::{Request, Status};
use ur_config::{AGENT_ID_HEADER, AGENT_SECRET_HEADER};

use crate::ProcessManager;

/// Creates a tonic interceptor that validates worker requests by checking
/// `ur-agent-id` and `ur-agent-secret` metadata headers against the
/// `ProcessManager`'s registered agents.
///
/// Returns `Status::unauthenticated` if either header is missing or the
/// agent_id/secret pair doesn't match a registered agent.
#[allow(clippy::result_large_err)]
pub fn worker_auth_interceptor(
    process_manager: ProcessManager,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone + Send + Sync + 'static {
    move |req: Request<()>| {
        let metadata = req.metadata();

        let agent_id = metadata
            .get(AGENT_ID_HEADER)
            .ok_or_else(|| Status::unauthenticated("missing ur-agent-id header"))?
            .to_str()
            .map_err(|_| Status::unauthenticated("invalid ur-agent-id header value"))?;

        let secret = metadata
            .get(AGENT_SECRET_HEADER)
            .ok_or_else(|| Status::unauthenticated("missing ur-agent-secret header"))?
            .to_str()
            .map_err(|_| Status::unauthenticated("invalid ur-agent-secret header value"))?;

        if !process_manager.verify_agent(agent_id, secret) {
            return Err(Status::unauthenticated(
                "agent authentication failed: invalid agent-id or secret",
            ));
        }

        Ok(req)
    }
}
