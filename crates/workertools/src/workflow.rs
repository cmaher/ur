use clap::Subcommand;

use ur_rpc::proto::workerd::worker_daemon_service_client::WorkerDaemonServiceClient;
use ur_rpc::proto::workerd::{DispatchTicketRequest, SetTicketRequest};

use crate::WORKERD_PORT;

#[derive(Subcommand)]
pub enum WorkflowCommands {
    /// Set the ticket ID for the current workflow session
    SetTicket {
        /// The ticket ID to set (e.g. ur-abc12)
        ticket_id: String,
    },
    /// Dispatch the previously set ticket to the server for workflow creation
    Dispatch,
}

async fn connect_workerd() -> Result<WorkerDaemonServiceClient<tonic::transport::Channel>, i32> {
    let addr = format!("http://127.0.0.1:{WORKERD_PORT}");
    let channel = match tonic::transport::Endpoint::try_from(addr)
        .unwrap()
        .connect()
        .await
    {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("[workertools] workflow: failed to connect to workerd: {e}");
            return Err(1);
        }
    };
    Ok(WorkerDaemonServiceClient::new(channel))
}

pub async fn run(command: WorkflowCommands) -> i32 {
    match command {
        WorkflowCommands::SetTicket { ticket_id } => run_set_ticket(ticket_id).await,
        WorkflowCommands::Dispatch => run_dispatch().await,
    }
}

async fn run_set_ticket(ticket_id: String) -> i32 {
    let mut client = match connect_workerd().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .set_ticket(SetTicketRequest {
            ticket_id: ticket_id.clone(),
        })
        .await
    {
        Ok(_) => {
            eprintln!("[workertools] set-ticket: ticket set to {ticket_id}");
            0
        }
        Err(e) => {
            eprintln!("[workertools] set-ticket: RPC failed: {e}");
            1
        }
    }
}

async fn run_dispatch() -> i32 {
    let mut client = match connect_workerd().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.dispatch_ticket(DispatchTicketRequest {}).await {
        Ok(resp) => {
            let error = resp.into_inner().error;
            if error.is_empty() {
                eprintln!("[workertools] dispatch: success");
                0
            } else {
                eprintln!("[workertools] dispatch: {error}");
                1
            }
        }
        Err(e) => {
            eprintln!("[workertools] dispatch: RPC failed: {e}");
            1
        }
    }
}
