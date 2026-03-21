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

    /// Set noverify on a ticket (skip pre-push verification and push with --no-verify)
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

/// Wrapper struct for use as a clap subcommand group.
#[derive(Debug, Parser)]
pub struct FlowCommand {
    #[command(subcommand)]
    pub command: FlowArgs,
}
