use subtle::ConstantTimeEq;
use tokio::runtime::Handle;
use tonic::{Request, Status};
use ur_config::{HOST_TOKEN_HEADER, UR_HOST_TOKEN_ENV, WORKER_ID_HEADER, WORKER_SECRET_HEADER};
use workflow_db::WorkerRepo;

/// Creates a tonic interceptor that validates worker requests by checking
/// `ur-worker-id` and `ur-worker-secret` metadata headers against the
/// `WorkerRepo`'s registered workers.
///
/// Returns `Status::unauthenticated` if either header is missing or the
/// worker_id/secret pair doesn't match a registered worker.
#[allow(clippy::result_large_err)]
pub fn worker_auth_interceptor(
    worker_repo: WorkerRepo,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone + Send + Sync + 'static {
    move |req: Request<()>| {
        let metadata = req.metadata();

        let worker_id = metadata
            .get(WORKER_ID_HEADER)
            .ok_or_else(|| Status::unauthenticated("missing ur-worker-id header"))?
            .to_str()
            .map_err(|_| Status::unauthenticated("invalid ur-worker-id header value"))?;

        let secret = metadata
            .get(WORKER_SECRET_HEADER)
            .ok_or_else(|| Status::unauthenticated("missing ur-worker-secret header"))?
            .to_str()
            .map_err(|_| Status::unauthenticated("invalid ur-worker-secret header value"))?;

        // Bridge async verify_worker into the sync interceptor context.
        let repo = worker_repo.clone();
        let worker_id = worker_id.to_owned();
        let secret = secret.to_owned();
        let verified = tokio::task::block_in_place(|| {
            Handle::current()
                .block_on(repo.verify_worker(&worker_id, &secret))
                .unwrap_or(false)
        });

        if !verified {
            return Err(Status::unauthenticated(
                "worker authentication failed: invalid worker-id or secret",
            ));
        }

        Ok(req)
    }
}

/// Read the host token ur-server requires on its host listener from
/// [`UR_HOST_TOKEN_ENV`] (set by `ur start` through the compose environment).
///
/// Fails closed: an unset, empty or malformed token is an error, so the
/// server refuses to start rather than serving the host listener
/// unauthenticated.
pub fn host_token_from_env() -> anyhow::Result<String> {
    let token = std::env::var(UR_HOST_TOKEN_ENV).unwrap_or_default();
    if token.is_empty() {
        anyhow::bail!(
            "{UR_HOST_TOKEN_ENV} is not set; refusing to serve the host gRPC listener \
             unauthenticated (start the server with 'ur server start')"
        );
    }
    ur_config::validate_host_token(&token)
        .map_err(|e| anyhow::anyhow!("{UR_HOST_TOKEN_ENV} is invalid: {e}"))?;
    Ok(token)
}

/// Creates a tonic interceptor for the host listener: every request must
/// carry `expected` in the [`HOST_TOKEN_HEADER`] metadata header.
///
/// The comparison is constant-time. A missing or wrong token is
/// `Status::unauthenticated`.
#[allow(clippy::result_large_err)]
pub fn host_auth_interceptor(
    expected: String,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone + Send + Sync + 'static {
    move |req: Request<()>| {
        let presented = req
            .metadata()
            .get(HOST_TOKEN_HEADER)
            .ok_or_else(|| Status::unauthenticated("missing ur-host-token header"))?;
        if !bool::from(presented.as_bytes().ct_eq(expected.as_bytes())) {
            return Err(Status::unauthenticated(
                "host authentication failed: invalid ur-host-token",
            ));
        }
        Ok(req)
    }
}
