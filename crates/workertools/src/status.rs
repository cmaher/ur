use clap::Subcommand;
use tonic::transport::Channel;

use ur_rpc::proto::core::UpdateAgentStatusRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::workerd::worker_daemon_service_client::WorkerDaemonServiceClient;
use ur_rpc::proto::workerd::{PauseNudgeRequest, StepCompleteRequest};

use crate::{WORKERD_PORT, inject_auth};

#[derive(Subcommand)]
pub enum StatusCommands {
    /// Signal workerd that the current lifecycle step completed successfully
    StepComplete,
    /// Suppress nudges for 5 minutes via workerd
    PauseNudge,
    /// Request human attention with a message
    RequestHuman {
        /// Message describing why human attention is needed
        message: String,
    },
}

async fn connect_workerd() -> Result<WorkerDaemonServiceClient<Channel>, i32> {
    let addr = format!("http://127.0.0.1:{WORKERD_PORT}");
    let channel = match tonic::transport::Endpoint::try_from(addr)
        .unwrap()
        .connect()
        .await
    {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("[workertools] status: failed to connect to workerd: {e}");
            return Err(1);
        }
    };
    Ok(WorkerDaemonServiceClient::new(channel))
}

async fn connect_server() -> Result<CoreServiceClient<Channel>, i32> {
    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).expect("UR_SERVER_ADDR must be set");
    let addr = format!("http://{server_addr}");

    let channel = match tonic::transport::Endpoint::try_from(addr)
        .unwrap()
        .connect()
        .await
    {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("[workertools] status: failed to connect to ur server: {e}");
            return Err(1);
        }
    };
    Ok(CoreServiceClient::new(channel))
}

pub async fn run(command: StatusCommands) -> i32 {
    match command {
        StatusCommands::StepComplete => run_step_complete().await,
        StatusCommands::PauseNudge => run_pause_nudge().await,
        StatusCommands::RequestHuman { message } => run_request_human(message).await,
    }
}

async fn run_step_complete() -> i32 {
    eprintln!("[workertools] step-complete: signaling step completion to workerd");
    let mut client = match connect_workerd().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.step_complete(StepCompleteRequest {}).await {
        Ok(_) => {
            eprintln!("[workertools] step-complete: success");
            0
        }
        Err(e) => {
            eprintln!("[workertools] step-complete: RPC failed: {e}");
            1
        }
    }
}

async fn run_pause_nudge() -> i32 {
    eprintln!("[workertools] pause-nudge: suppressing nudges via workerd");
    let mut client = match connect_workerd().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.pause_nudge(PauseNudgeRequest {}).await {
        Ok(_) => {
            eprintln!("[workertools] pause-nudge: success");
            0
        }
        Err(e) => {
            eprintln!("[workertools] pause-nudge: RPC failed: {e}");
            1
        }
    }
}

async fn run_request_human(message: String) -> i32 {
    let mut client = match connect_server().await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let worker_id = std::env::var(ur_config::UR_WORKER_ID_ENV).unwrap_or_default();

    let mut request = tonic::Request::new(UpdateAgentStatusRequest {
        worker_id,
        status: ur_rpc::agent_status::STALLED.to_string(),
        message,
    });
    inject_auth(&mut request);

    match client.update_agent_status(request).await {
        Ok(_) => 0,
        Err(status) => {
            eprintln!("status request-human: {}", status.message());
            1
        }
    }
}
