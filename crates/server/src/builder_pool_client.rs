use std::path::PathBuf;

use tonic::{Code, Status};

use ur_rpc::proto::builder_pool::builder_pool_service_client::BuilderPoolServiceClient;
use ur_rpc::proto::builder_pool::{
    CheckoutBranchRequest, CleanSlotRequest, PrepareNewSlotRequest, PrepareSharedSlotRequest,
    RecycleSlotRequest, ScanSlotsRequest,
};

/// Thin clone-able wrapper around the generated `BuilderPoolServiceClient`.
///
/// Constructed from the same tonic channel used by the other builderd clients —
/// one TCP connection, multiple service clients. Exposes typed async methods
/// matching the six RPCs defined in `builder_pool.proto`.
#[derive(Clone)]
pub struct BuilderPoolClient {
    inner: BuilderPoolServiceClient<tonic::transport::Channel>,
}

impl BuilderPoolClient {
    /// Create a new client from an already-connected tonic transport channel.
    pub fn new(channel: tonic::transport::Channel) -> Self {
        Self {
            inner: BuilderPoolServiceClient::new(channel),
        }
    }

    /// Scan the existing slots for a project and return their indices.
    ///
    /// Returns `Internal` on builderd-side failures.
    pub async fn scan_slots(&self, project_key: String) -> Result<Vec<u32>, String> {
        let mut client = self.inner.clone();
        let response = client
            .scan_slots(ScanSlotsRequest { project_key })
            .await
            .map_err(|status| preserve_status_message(status, "scan_slots"))?;
        Ok(response.into_inner().slot_indices)
    }

    /// Prepare a new slot by cloning the repo into a fresh slot directory.
    ///
    /// Returns the host path to the slot directory.
    /// Returns `Internal` on clone failure.
    pub async fn prepare_new_slot(
        &self,
        project_key: String,
        slot_name: String,
        repo_url: String,
    ) -> Result<PathBuf, String> {
        let mut client = self.inner.clone();
        let response = client
            .prepare_new_slot(PrepareNewSlotRequest {
                project_key,
                slot_name,
                repo_url,
            })
            .await
            .map_err(|status| preserve_status_message(status, "prepare_new_slot"))?;
        Ok(PathBuf::from(response.into_inner().host_path))
    }

    /// Recycle an existing slot: fetch latest and reset to the default branch.
    ///
    /// Returns the host path to the slot directory.
    /// Returns `NotFound` if the slot does not exist.
    /// Returns `Internal` on git fetch/reset failure.
    pub async fn recycle_slot(
        &self,
        project_key: String,
        slot_name: String,
        repo_url: String,
    ) -> Result<PathBuf, String> {
        let mut client = self.inner.clone();
        let response = client
            .recycle_slot(RecycleSlotRequest {
                project_key,
                slot_name,
                repo_url,
            })
            .await
            .map_err(|status| preserve_status_message(status, "recycle_slot"))?;
        Ok(PathBuf::from(response.into_inner().host_path))
    }

    /// Prepare the shared (read-only) slot for the project, creating it if needed.
    ///
    /// Returns the host path to the shared slot directory.
    /// Returns `Internal` on clone or setup failure.
    pub async fn prepare_shared_slot(
        &self,
        project_key: String,
        repo_url: String,
    ) -> Result<PathBuf, String> {
        let mut client = self.inner.clone();
        let response = client
            .prepare_shared_slot(PrepareSharedSlotRequest {
                project_key,
                repo_url,
            })
            .await
            .map_err(|status| preserve_status_message(status, "prepare_shared_slot"))?;
        Ok(PathBuf::from(response.into_inner().host_path))
    }

    /// Checkout a branch in the given slot from the shared slot base.
    ///
    /// Returns `NotFound` if the slot does not exist.
    /// Returns `Internal` on git checkout failure.
    pub async fn checkout_branch(
        &self,
        project_key: String,
        slot_name: String,
        branch_prefix: String,
        branch_name: String,
    ) -> Result<(), String> {
        let mut client = self.inner.clone();
        client
            .checkout_branch(CheckoutBranchRequest {
                project_key,
                slot_name,
                branch_prefix,
                branch_name,
            })
            .await
            .map_err(|status| preserve_status_message(status, "checkout_branch"))?;
        Ok(())
    }

    /// Remove a slot directory and free its resources.
    ///
    /// Returns `NotFound` if the slot does not exist.
    /// Returns `Internal` on removal failure.
    pub async fn clean_slot(&self, project_key: String, slot_name: String) -> Result<(), String> {
        let mut client = self.inner.clone();
        client
            .clean_slot(CleanSlotRequest {
                project_key,
                slot_name,
            })
            .await
            .map_err(|status| preserve_status_message(status, "clean_slot"))?;
        Ok(())
    }
}

/// Produce a clear error message from a gRPC status, preserving the RPC name.
///
/// The original status code and message are included so callers can distinguish
/// transport-level failures from server-side errors.
fn preserve_status_message(status: Status, rpc: &str) -> String {
    let code = status.code();
    match code {
        // These codes originate from the server handler; pass through as-is.
        Code::NotFound
        | Code::FailedPrecondition
        | Code::Internal
        | Code::Unavailable
        | Code::InvalidArgument => {
            format!("builder_pool {rpc}: {} ({})", status.message(), code)
        }
        // Any other code (e.g. Unknown, Cancelled) is preserved with context.
        _ => format!("builder_pool {rpc}: {} ({})", status.message(), code),
    }
}
