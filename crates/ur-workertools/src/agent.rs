use clap::Subcommand;

use ur_rpc::proto::core::UpdateAgentStatusRequest;

use crate::{connect_core, inject_auth};

#[derive(Subcommand)]
pub enum AgentCommands {
    /// Request human attention with a message
    RequestHuman {
        /// Message describing why human attention is needed
        message: String,
    },
}

pub async fn run(command: AgentCommands) -> i32 {
    let mut client = match connect_core().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("agent: {e}");
            return 1;
        }
    };

    let worker_id = std::env::var(ur_config::UR_WORKER_ID_ENV).unwrap_or_default();

    match command {
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
