// Shared data types for ur_db.

use std::fmt;
use std::str::FromStr;

/// Lifecycle status for workflow-driven tickets.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleStatus {
    Design,
    #[default]
    Open,
    Implementing,
    Pushing,
    InReview,
    FeedbackCreating,
    FeedbackResolving,
    Stalled,
    Done,
}

impl LifecycleStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Design => "design",
            Self::Open => "open",
            Self::Implementing => "implementing",
            Self::Pushing => "pushing",
            Self::InReview => "in_review",
            Self::FeedbackCreating => "feedback_creating",
            Self::FeedbackResolving => "feedback_resolving",
            Self::Stalled => "stalled",
            Self::Done => "done",
        }
    }
}

impl FromStr for LifecycleStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "design" => Ok(Self::Design),
            "open" => Ok(Self::Open),
            "implementing" => Ok(Self::Implementing),
            "pushing" => Ok(Self::Pushing),
            "in_review" => Ok(Self::InReview),
            "feedback_creating" => Ok(Self::FeedbackCreating),
            "feedback_resolving" => Ok(Self::FeedbackResolving),
            "stalled" => Ok(Self::Stalled),
            "done" => Ok(Self::Done),
            _ => Err(format!("unknown lifecycle status: {s}")),
        }
    }
}

impl fmt::Display for LifecycleStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub struct Ticket {
    pub id: String,
    pub project: String,
    pub type_: String,
    pub status: String,
    pub lifecycle_status: LifecycleStatus,
    pub priority: i32,
    pub parent_id: Option<String>,
    pub title: String,
    pub body: String,
    pub branch: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Default)]
pub struct NewTicket {
    pub id: String,
    pub project: String,
    pub type_: String,
    pub priority: i32,
    pub parent_id: Option<String>,
    pub title: String,
    pub body: String,
    /// If set, use this status instead of the default "open".
    pub status: Option<String>,
    /// If set, use this lifecycle status instead of the default.
    pub lifecycle_status: Option<LifecycleStatus>,
    /// Branch associated with this ticket.
    pub branch: Option<String>,
    /// If set, use this timestamp instead of now.
    pub created_at: Option<String>,
}

pub struct TicketUpdate {
    pub status: Option<String>,
    pub lifecycle_status: Option<LifecycleStatus>,
    pub type_: Option<String>,
    pub priority: Option<i32>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub branch: Option<Option<String>>,    // Some(None) to clear
    pub parent_id: Option<Option<String>>, // Some(None) to clear
    pub project: Option<String>,
}

pub struct TicketFilter {
    pub project: Option<String>,
    pub status: Option<String>,
    pub type_: Option<String>,
    pub parent_id: Option<String>,
    pub lifecycle_status: Option<LifecycleStatus>,
}

pub struct Activity {
    pub id: String,
    pub ticket_id: String,
    pub timestamp: String,
    pub author: String,
    pub message: String,
}

pub struct DispatchableTicket {
    pub id: String,
    pub title: String,
    pub priority: i32,
    pub type_: String,
}

pub struct MetadataMatchTicket {
    pub id: String,
    pub title: String,
    pub type_: String,
    pub status: String,
    pub key: String,
    pub value: String,
}

pub struct Edge {
    pub source_id: String,
    pub target_id: String,
    pub kind: EdgeKind,
}

pub enum EdgeKind {
    Blocks,
    RelatesTo,
    FollowUp,
}

pub struct WorkflowEvent {
    pub id: String,
    pub ticket_id: String,
    pub old_lifecycle_status: LifecycleStatus,
    pub new_lifecycle_status: LifecycleStatus,
    pub attempts: i32,
    pub created_at: String,
}

pub struct Slot {
    pub id: String,
    pub project_key: String,
    pub slot_name: String,
    pub host_path: String,
    pub created_at: String,
    pub updated_at: String,
}

pub struct Worker {
    pub worker_id: String,
    pub process_id: String,
    pub project_key: String,
    pub container_id: String,
    pub worker_secret: String,
    pub strategy: String,
    pub container_status: String,
    pub agent_status: String,
    pub workspace_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub idle_redispatch_count: i32,
}

pub struct WorkerSlot {
    pub worker_id: String,
    pub slot_id: String,
    pub created_at: String,
}
