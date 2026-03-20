use clap::{Parser, Subcommand};

/// Workflow management subcommands.
#[derive(Debug, Subcommand)]
pub enum FlowArgs {
    /// Show a workflow by ticket ID
    Show {
        /// Ticket ID
        ticket_id: String,
    },

    /// List workflows with optional status filter
    List {
        /// Filter by workflow status
        #[arg(long)]
        status: Option<String>,
    },

    /// Cancel an active workflow for a ticket
    Cancel {
        /// Ticket ID
        ticket_id: String,
    },
}

/// Wrapper struct for use as a clap subcommand group.
#[derive(Debug, Parser)]
pub struct FlowCommand {
    #[command(subcommand)]
    pub command: FlowArgs,
}
