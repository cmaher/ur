use tokio::runtime::Handle;
use tonic::{Request, Status};
use ur_config::{WORKER_ID_HEADER, WORKER_SECRET_HEADER};
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
