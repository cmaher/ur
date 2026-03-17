use clap::{Parser, Subcommand};

/// Ticket management subcommands matching all TicketService RPCs.
#[derive(Debug, Subcommand)]
pub enum TicketArgs {
    /// Create a new ticket
    Create {
        /// Ticket title
        title: String,

        /// Project key
        #[arg(short, long)]
        project: Option<String>,

        /// Ticket type (epic or task)
        #[arg(long = "type", default_value = "task")]
        ticket_type: String,

        /// Parent ticket ID
        #[arg(long)]
        parent: Option<String>,

        /// Priority (lower is higher priority)
        #[arg(long, default_value_t = 0)]
        priority: i64,

        /// Ticket body text
        #[arg(long, default_value = "")]
        body: String,

        /// Create as work-in-progress (sets lifecycle_status to design)
        #[arg(long)]
        wip: bool,
    },

    /// List tickets with optional filters
    List {
        /// Project key
        #[arg(short, long)]
        project: Option<String>,

        /// Show tickets from all projects (omit project filter)
        #[arg(long)]
        all: bool,

        /// Filter by parent epic ID
        #[arg(long)]
        epic: Option<String>,

        /// Filter by ticket type
        #[arg(long = "type")]
        ticket_type: Option<String>,

        /// Filter by status
        #[arg(long)]
        status: Option<String>,

        /// Filter by lifecycle status (design, open, implementing, pushing, in_review, etc.)
        #[arg(long)]
        lifecycle: Option<String>,
    },

    /// Show a ticket's full detail
    Show {
        /// Ticket ID
        id: String,
    },

    /// Update a ticket's fields
    #[command(alias = "edit")]
    Update {
        /// Ticket ID
        id: String,

        /// New title
        #[arg(long)]
        title: Option<String>,

        /// New body text
        #[arg(long)]
        body: Option<String>,

        /// New status
        #[arg(long)]
        status: Option<String>,

        /// New priority
        #[arg(long)]
        priority: Option<i64>,

        /// New ticket type
        #[arg(long = "type")]
        ticket_type: Option<String>,

        /// New parent ticket ID
        #[arg(long, conflicts_with = "no_parent")]
        parent: Option<String>,

        /// Clear the parent (remove from epic)
        #[arg(long, conflicts_with = "parent")]
        no_parent: bool,

        /// Force the update (e.g. close an epic with open children)
        #[arg(long)]
        force: bool,

        /// New lifecycle status (design, open, implementing, pushing, in_review, etc.)
        #[arg(long)]
        lifecycle: Option<String>,

        /// New branch name (use --no-branch to clear)
        #[arg(long, conflicts_with = "no_branch")]
        branch: Option<String>,

        /// Clear the branch
        #[arg(long, conflicts_with = "branch")]
        no_branch: bool,
    },

    /// Set a metadata key-value pair on a ticket
    SetMeta {
        /// Ticket ID
        id: String,

        /// Metadata key
        key: String,

        /// Metadata value
        value: String,
    },

    /// Delete a metadata key from a ticket
    DeleteMeta {
        /// Ticket ID
        id: String,

        /// Metadata key to delete
        key: String,
    },

    /// Add an activity note to a ticket
    AddActivity {
        /// Ticket ID
        id: String,

        /// Activity message
        message: String,

        /// Metadata key=value pairs
        #[arg(long = "meta", value_parser = parse_key_value)]
        meta: Vec<KeyValue>,
    },

    /// List activities on a ticket
    ListActivities {
        /// Ticket ID
        id: String,
    },

    /// Add a blocking dependency (blocked-by-id blocks id)
    AddBlock {
        /// Ticket ID that is blocked
        id: String,

        /// Ticket ID that is the blocker
        blocked_by_id: String,
    },

    /// Remove a blocking dependency
    RemoveBlock {
        /// Ticket ID that is blocked
        id: String,

        /// Ticket ID that is the blocker
        blocked_by_id: String,
    },

    /// Add a bidirectional link between tickets
    AddLink {
        /// First ticket ID
        id: String,

        /// Second ticket ID
        linked_id: String,

        /// Edge kind: relates_to (default), follow_up
        #[arg(long, default_value = "relates_to")]
        edge: String,
    },

    /// Remove a bidirectional link between tickets
    RemoveLink {
        /// First ticket ID
        id: String,

        /// Second ticket ID
        linked_id: String,
    },

    /// List dispatchable tickets for an epic
    Dispatchable {
        /// Epic ticket ID
        epic_id: String,

        /// Project key
        #[arg(short, long)]
        project: Option<String>,
    },

    /// Approve a ticket (transition lifecycle from in_review to feedback_creating)
    Approve {
        /// Ticket ID
        id: String,

        /// Create feedback tickets immediately after approval
        #[arg(long, group = "feedback_timing")]
        feedback_now: bool,

        /// Defer feedback ticket creation to later
        #[arg(long, group = "feedback_timing")]
        feedback_later: bool,
    },

    /// Close a ticket (sugar for `update <id> --status closed`)
    Close {
        /// Ticket ID
        id: String,

        /// Force close (e.g. close an epic with open children)
        #[arg(long)]
        force: bool,
    },

    /// Open a ticket (sugar for `update <id> --status open`)
    Open {
        /// Ticket ID
        id: String,
    },

    /// Print project status report (epic tree with open/closed counts)
    Status {
        /// Project key to filter tickets by ID prefix (e.g. "ur" shows ur-* tickets)
        #[arg(short, long)]
        project: Option<String>,
    },
}

/// A parsed key=value pair for activity metadata.
#[derive(Debug, Clone)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

fn parse_key_value(s: &str) -> Result<KeyValue, String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid key=value pair: no '=' found in '{s}'"))?;
    Ok(KeyValue {
        key: s[..pos].to_owned(),
        value: s[pos + 1..].to_owned(),
    })
}

/// Wrapper struct for use as a clap subcommand group.
#[derive(Debug, Parser)]
pub struct TicketCommand {
    #[command(subcommand)]
    pub command: TicketArgs,
}
