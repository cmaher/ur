use tonic::{Code, Status};

use ur_rpc::proto::builder_container::builder_container_service_client::BuilderContainerServiceClient;
use ur_rpc::proto::builder_container::{
    ExecContainerRequest, ExecContainerResponse, InspectNetworkRequest, InspectNetworkResponse,
    LaunchWorkerRequest, LaunchWorkerResponse, ListWorkerContainersRequest, StopWorkerRequest,
    StopWorkerResponse, WorkerContainerStatus,
};

/// Thin clone-able wrapper around the generated `BuilderContainerServiceClient`.
///
/// Constructed from the same retry channel used by `BuilderdClient` — one TCP
/// connection, two service clients. Exposes typed async methods matching the
/// RPCs defined in `builder_container.proto`.
#[derive(Clone)]
pub struct BuilderContainerClient {
    inner: BuilderContainerServiceClient<tonic::transport::Channel>,
}

impl BuilderContainerClient {
    /// Create a new client from an already-connected tonic transport channel.
    pub fn new(channel: tonic::transport::Channel) -> Self {
        Self {
            inner: BuilderContainerServiceClient::new(channel),
        }
    }

    /// Launch a worker container with the given spec.
    ///
    /// Returns `FailedPrecondition` if a volume source is missing or the worker exits
    /// before becoming healthy. Returns `Internal` on Docker launch or inspection failure.
    pub async fn launch_worker(
        &self,
        request: LaunchWorkerRequest,
    ) -> Result<LaunchWorkerResponse, Status> {
        let mut client = self.inner.clone();
        let response = client
            .launch_worker(request)
            .await
            .map_err(|status| preserve_status_code(status, "launch_worker"))?;
        Ok(response.into_inner())
    }

    /// Stop and remove every container of a worker id (builderd's Stop
    /// barrier). Ok when none is left; `Internal` on docker failures.
    pub async fn stop_worker(
        &self,
        request: StopWorkerRequest,
    ) -> Result<StopWorkerResponse, Status> {
        let mut client = self.inner.clone();
        let response = client
            .stop_worker(request)
            .await
            .map_err(|status| preserve_status_code(status, "stop_worker"))?;
        Ok(response.into_inner())
    }

    /// Execute a command inside a running container and return its output.
    ///
    /// Used for squid reconfigure and similar one-shot exec operations.
    /// Returns `Internal` on docker exec failure.
    pub async fn exec_container(
        &self,
        request: ExecContainerRequest,
    ) -> Result<ExecContainerResponse, Status> {
        let mut client = self.inner.clone();
        let response = client
            .exec_container(request)
            .await
            .map_err(|status| preserve_status_code(status, "exec_container"))?;
        Ok(response.into_inner())
    }

    /// Inspect whether a Docker network exists.
    ///
    /// Returns `Internal` if the inspect command itself fails to run.
    /// A missing network is not an error — it is reflected as `exists: false`.
    pub async fn inspect_network(
        &self,
        request: InspectNetworkRequest,
    ) -> Result<InspectNetworkResponse, Status> {
        let mut client = self.inner.clone();
        let response = client
            .inspect_network(request)
            .await
            .map_err(|status| preserve_status_code(status, "inspect_network"))?;
        Ok(response.into_inner())
    }

    /// Every worker container this ur instance runs, by worker id.
    pub async fn list_worker_containers(&self) -> Result<Vec<WorkerContainerStatus>, Status> {
        let mut client = self.inner.clone();
        let response = client
            .list_worker_containers(ListWorkerContainersRequest {})
            .await
            .map_err(|status| preserve_status_code(status, "list_worker_containers"))?;
        Ok(response.into_inner().containers)
    }
}

/// Preserve the original gRPC status code from the server.
///
/// When the server returns `NotFound`, `FailedPrecondition`, `Internal`, or
/// `Unavailable`, this function ensures those codes are passed through unchanged.
/// For transport-level errors (where tonic synthesizes a code), the code is
/// preserved as-is — typically `Unavailable` for connection failures.
fn preserve_status_code(status: Status, rpc: &str) -> Status {
    let code = status.code();
    match code {
        // These codes originate from the server handler; pass through as-is.
        Code::NotFound
        | Code::FailedPrecondition
        | Code::Internal
        | Code::Unavailable
        | Code::InvalidArgument => status,
        // Any other code (e.g. Unknown, Cancelled) is preserved without remapping.
        _ => Status::new(
            code,
            format!("builder_container {rpc}: {}", status.message()),
        ),
    }
}
