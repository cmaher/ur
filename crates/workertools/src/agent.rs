use clap::Subcommand;
use tonic::transport::Channel;

use ur_rpc::proto::core::UpdateAgentStatusRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;

use crate::inject_auth;

#[derive(Subcommand)]
pub enum AgentCommands {
    /// Signal that the agent has finished its current task
    Done,
    /// Request human attention with a message
    RequestHuman {
        /// Message describing why human attention is needed
        message: String,
    },
}

async fn connect() -> Result<CoreServiceClient<Channel>, i32> {
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
            eprintln!("agent: failed to connect to ur server: {e}");
            return Err(1);
        }
    };

    Ok(CoreServiceClient::new(channel))
}

pub async fn run(command: AgentCommands) -> i32 {
    let mut client = match connect().await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let worker_id = std::env::var(ur_config::UR_WORKER_ID_ENV).unwrap_or_default();

    match command {
        AgentCommands::Done => {
            let mut request = tonic::Request::new(UpdateAgentStatusRequest {
                worker_id,
                status: ur_rpc::agent_status::IDLE.to_string(),
                message: String::new(),
            });
            inject_auth(&mut request);

            match client.update_agent_status(request).await {
                Ok(_) => 0,
                Err(status) => {
                    eprintln!("agent done: {}", status.message());
                    1
                }
            }
        }
        AgentCommands::RequestHuman { message } => {
            let mut request = tonic::Request::new(UpdateAgentStatusRequest {
                worker_id,
                status: ur_rpc::agent_status::STALLED.to_string(),
                message,
            });
            inject_auth(&mut request);

            match client.update_agent_status(request).await {
                Ok(_) => 0,
                Err(status) => {
                    eprintln!("agent request-human: {}", status.message());
                    1
                }
            }
        }
    }
}
