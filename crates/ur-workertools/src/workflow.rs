use clap::Subcommand;

use ur_rpc::proto::workerd::{DispatchTicketRequest, SetTicketRequest};

use crate::workerd_client::{connect_workerd, workerd_request};

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

pub async fn run(command: WorkflowCommands) -> i32 {
    match command {
        WorkflowCommands::SetTicket { ticket_id } => run_set_ticket(ticket_id).await,
        WorkflowCommands::Dispatch => run_dispatch().await,
    }
}

async fn run_set_ticket(ticket_id: String) -> i32 {
    let mut client = match connect_workerd("workflow").await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .set_ticket(workerd_request(SetTicketRequest {
            ticket_id: ticket_id.clone(),
        }))
        .await
    {
        Ok(_) => {
            eprintln!("[ur-workertools] set-ticket: ticket set to {ticket_id}");
            0
        }
        Err(e) => {
            eprintln!("[ur-workertools] set-ticket: RPC failed: {e}");
            1
        }
    }
}

async fn run_dispatch() -> i32 {
    let mut client = match connect_workerd("workflow").await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .dispatch_ticket(workerd_request(DispatchTicketRequest {}))
        .await
    {
        Ok(resp) => {
            let error = resp.into_inner().error;
            if error.is_empty() {
                eprintln!("[ur-workertools] dispatch: success");
                0
            } else {
                eprintln!("[ur-workertools] dispatch: {error}");
                1
            }
        }
        Err(e) => {
            eprintln!("[ur-workertools] dispatch: RPC failed: {e}");
            1
        }
    }
}
