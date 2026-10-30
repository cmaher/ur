use std::time::Duration;

use anyhow::Result;
use tonic::transport::Endpoint;
use tracing::{error, info};

use ur_rpc::proto::core::WorkerStopRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;

use crate::exit_watcher::{AgentExitReason, AgentExitSink};
use crate::grpc_service::WorkerIdentity;

/// [`AgentExitSink`] for workerd running as a container's PID 1.
///
/// Sends `WorkerStop` to ur-server and then waits forever for the server to
/// kill the container. If the RPC fails, exits the process (code 0), which
/// stops the container directly. Never returns.
#[derive(Clone)]
pub struct ContainerExitSink {
    identity: WorkerIdentity,
}

impl ContainerExitSink {
    pub fn new(identity: WorkerIdentity) -> Self {
        ContainerExitSink { identity }
    }

    async fn self_stop_rpc(&self) -> Result<()> {
        let addr = format!("http://{}", self.identity.server_addr);
        let channel = Endpoint::try_from(addr)?.connect().await?;
        let mut client = CoreServiceClient::new(channel);

        let mut request = tonic::Request::new(WorkerStopRequest {
            worker_id: self.identity.worker_id.clone(),
        });
        self.identity.inject_auth_headers(request.metadata_mut());

        client.worker_stop(request).await?;
        Ok(())
    }
}

#[tonic::async_trait]
impl AgentExitSink for ContainerExitSink {
    async fn agent_exited(&self, reason: AgentExitReason) -> Result<()> {
        info!(
            ?reason,
            worker_id = self.identity.worker_id,
            server_addr = self.identity.server_addr,
            "agent exited, sending WorkerStop RPC"
        );
        match self.self_stop_rpc().await {
            Ok(()) => {
                info!("WorkerStop RPC succeeded, waiting for server to stop container");
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            }
            Err(e) => {
                error!(error = %e, "WorkerStop RPC failed, falling back to process::exit(0)");
                std::process::exit(0);
            }
        }
    }
}
