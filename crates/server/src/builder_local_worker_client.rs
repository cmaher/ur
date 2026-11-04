use tonic::Status;

use ur_rpc::proto::builder_local_worker::builder_local_worker_service_client::BuilderLocalWorkerServiceClient;
use ur_rpc::proto::builder_local_worker::{LaunchLocalWorkerRequest, StopLocalWorkerRequest};

/// Clone-able wrapper around the generated `BuilderLocalWorkerServiceClient`,
/// sharing builderd's retry channel with the other builderd clients.
/// builderd's error text is preserved; callers wrap it into a `String`, so
/// the CLI sees the message but not the original `Status` code.
#[derive(Clone)]
pub struct BuilderLocalWorkerClient {
    inner: BuilderLocalWorkerServiceClient<tonic::transport::Channel>,
}

impl BuilderLocalWorkerClient {
    pub fn new(channel: tonic::transport::Channel) -> Self {
        Self {
            inner: BuilderLocalWorkerServiceClient::new(channel),
        }
    }

    /// Build the drive, run startup hooks and start the agent. builderd tears
    /// everything down before returning an error.
    pub async fn launch_local_worker(
        &self,
        request: LaunchLocalWorkerRequest,
    ) -> Result<(), Status> {
        self.inner.clone().launch_local_worker(request).await?;
        Ok(())
    }

    /// Tear a local worker down. Unknown ids succeed.
    pub async fn stop_local_worker(&self, worker_id: &str) -> Result<(), Status> {
        self.inner
            .clone()
            .stop_local_worker(StopLocalWorkerRequest {
                worker_id: worker_id.to_owned(),
            })
            .await?;
        Ok(())
    }
}
