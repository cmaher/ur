use anyhow::{Result, bail};
use clap::Subcommand;
use tracing::info;
use ur_rpc::error::StatusResultExt;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::*;

use crate::connection;
use crate::output::OutputManager;

/// Map a lifecycle status to its natural next state (what --continue does).
fn next_lifecycle_status(current: &str) -> Option<&'static str> {
    match current {
        "awaiting_dispatch" => Some("implementing"),
        "implementing" => Some("verifying"),
        "fixing" => Some("verifying"),
        "verifying" => Some("pushing"),
        "pushing" => Some("in_review"),
        _ => None,
    }
}

/// Admin subcommands — privileged operations blocked from workers via hostexec.
#[derive(Debug, Subcommand)]
pub enum AdminCommands {
    /// Set noverify meta on a ticket (skip pre-push verification)
    Noverify {
        /// Ticket ID
        ticket_id: String,
    },

    /// Move a ticket to a specified lifecycle status, clearing stall_reason metadata
    Redrive {
        /// Ticket ID
        id: String,

        /// Target lifecycle status to move to
        #[arg(long, required_unless_present = "advance")]
        to: Option<String>,

        /// Advance to the natural next lifecycle state
        #[arg(long = "continue", id = "advance", conflicts_with = "to")]
        advance: bool,
    },

    /// Pre-set feedback_mode meta so GithubPoller auto-advances from in_review
    Autoapprove {
        /// Ticket ID
        ticket_id: String,

        /// Create feedback tickets immediately after approval
        #[arg(long, group = "feedback_timing")]
        feedback_now: bool,

        /// Defer feedback ticket creation to later
        #[arg(long, group = "feedback_timing")]
        feedback_later: bool,
    },
}

pub async fn handle(port: u16, command: AdminCommands, output: &OutputManager) -> Result<()> {
    let channel = connection::connect(port).await?;
    let mut client = TicketServiceClient::new(channel);

    match command {
        AdminCommands::Noverify { ticket_id } => {
            info!(ticket_id = %ticket_id, "setting noverify meta");
            client
                .set_meta(SetMetaRequest {
                    ticket_id: ticket_id.clone(),
                    key: "noverify".to_owned(),
                    value: "true".to_owned(),
                })
                .await
                .with_status_context("set noverify metadata")?;

            if output.is_json() {
                output.print_success(&serde_json::json!({
                    "kind": "noverify_set",
                    "id": ticket_id,
                }));
            } else {
                println!("Set noverify on {ticket_id}");
            }
            Ok(())
        }

        AdminCommands::Redrive { id, to, advance } => {
            handle_redrive(&mut client, output, id, to, advance).await
        }

        AdminCommands::Autoapprove {
            ticket_id,
            feedback_now,
            feedback_later,
        } => {
            if !feedback_now && !feedback_later {
                bail!("one of --feedback-now or --feedback-later is required");
            }

            let feedback_mode = if feedback_now { "now" } else { "later" }.to_owned();

            info!(ticket_id = %ticket_id, feedback_mode = %feedback_mode, "setting autoapprove");

            client
                .set_meta(SetMetaRequest {
                    ticket_id: ticket_id.clone(),
                    key: "feedback_mode".to_owned(),
                    value: feedback_mode.clone(),
                })
                .await
                .with_status_context("set feedback_mode metadata")?;

            if output.is_json() {
                output.print_success(&serde_json::json!({
                    "kind": "autoapprove_set",
                    "id": ticket_id,
                    "feedback_mode": feedback_mode,
                }));
            } else {
                println!("Set autoapprove on {ticket_id} (feedback_mode={feedback_mode})");
            }
            Ok(())
        }
    }
}

async fn handle_redrive(
    client: &mut TicketServiceClient<tonic::transport::Channel>,
    output: &OutputManager,
    id: String,
    to: Option<String>,
    advance: bool,
) -> Result<()> {
    let to = if advance {
        let resp = client
            .get_ticket(GetTicketRequest { id: id.clone() })
            .await
            .with_status_context("get ticket for --continue")?;
        let ticket = resp
            .into_inner()
            .ticket
            .ok_or_else(|| anyhow::anyhow!("ticket {id} not found"))?;
        let current = &ticket.lifecycle_status;
        next_lifecycle_status(current)
            .ok_or_else(|| anyhow::anyhow!("no natural next state from '{current}' — use --to"))?
            .to_string()
    } else {
        to.unwrap()
    };

    info!(id = %id, to = %to, "redriving ticket");

    client
        .delete_meta(DeleteMetaRequest {
            ticket_id: id.clone(),
            key: "stall_reason".to_owned(),
        })
        .await
        .with_status_context("delete stall_reason metadata")?;

    client
        .update_ticket(UpdateTicketRequest {
            id: id.clone(),
            status: None,
            priority: None,
            title: None,
            body: None,
            force: false,
            ticket_type: None,
            parent_id: None,
            lifecycle_status: Some(to.clone()),
            branch: None,
            project: None,
            lifecycle_managed: None,
        })
        .await
        .with_status_context("update lifecycle status")?;

    if output.is_json() {
        output.print_success(&serde_json::json!({
            "kind": "redriven",
            "id": id,
            "lifecycle_status": to,
        }));
    } else {
        println!("Redrove {id} to {to}");
    }
    Ok(())
}
