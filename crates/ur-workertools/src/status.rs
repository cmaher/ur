use clap::Subcommand;
use tonic::transport::Channel;

use ur_rpc::proto::core::UpdateAgentStatusRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::workerd::{PauseNudgeRequest, StepCompleteRequest};

use crate::inject_auth;
use crate::workerd_client::{connect_workerd, workerd_request};

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
            eprintln!("[ur-workertools] status: failed to connect to ur server: {e}");
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
    eprintln!("[ur-workertools] step-complete: signaling step completion to workerd");
    let mut client = match connect_workerd("status").await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .step_complete(workerd_request(StepCompleteRequest {}))
        .await
    {
        Ok(_) => {
            eprintln!("[ur-workertools] step-complete: success");
            0
        }
        Err(e) => {
            eprintln!("[ur-workertools] step-complete: RPC failed: {e}");
            1
        }
    }
}

async fn run_pause_nudge() -> i32 {
    eprintln!("[ur-workertools] pause-nudge: suppressing nudges via workerd");
    let mut client = match connect_workerd("status").await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .pause_nudge(workerd_request(PauseNudgeRequest {}))
        .await
    {
        Ok(_) => {
            eprintln!("[ur-workertools] pause-nudge: success");
            0
        }
        Err(e) => {
            eprintln!("[ur-workertools] pause-nudge: RPC failed: {e}");
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
