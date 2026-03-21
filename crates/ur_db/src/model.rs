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
    Merging,
    Verifying,
    AwaitingDispatch,
    Done,
    Cancelled,
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
            Self::Merging => "merging",
            Self::Verifying => "verifying",
            Self::AwaitingDispatch => "awaiting_dispatch",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    /// Returns `true` if this status represents a terminal state (workflow complete).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
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
            "merging" => Ok(Self::Merging),
            "verifying" => Ok(Self::Verifying),
            "awaiting_dispatch" => Ok(Self::AwaitingDispatch),
            "done" => Ok(Self::Done),
            "cancelled" => Ok(Self::Cancelled),
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
    pub lifecycle_managed: bool,
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

#[derive(Default)]
pub struct TicketUpdate {
    pub status: Option<String>,
    pub lifecycle_status: Option<LifecycleStatus>,
    pub lifecycle_managed: Option<bool>,
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

/// Ticket status enum for workflow-driven tickets.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TicketStatus {
    #[default]
    Open,
    InProgress,
    Closed,
}

impl TicketStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Closed => "closed",
        }
    }
}

impl FromStr for TicketStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(Self::Open),
            "in_progress" => Ok(Self::InProgress),
            "closed" => Ok(Self::Closed),
            _ => Err(format!("unknown ticket status: {s}")),
        }
    }
}

impl fmt::Display for TicketStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A workflow tracks the lifecycle state machine for a single ticket.
pub struct Workflow {
    pub id: String,
    pub ticket_id: String,
    pub status: LifecycleStatus,
    pub stalled: bool,
    pub stall_reason: String,
    pub implement_cycles: i32,
    pub worker_id: String,
    pub noverify: bool,
    pub feedback_mode: String,
    pub created_at: String,
}

/// A workflow intent represents a desired state transition for a ticket.
pub struct WorkflowIntent {
    pub id: String,
    pub ticket_id: String,
    pub target_status: LifecycleStatus,
    pub created_at: String,
}

/// Agent status for workers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentStatus {
    #[default]
    Starting,
    Idle,
    Working,
    Stalled,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Stalled => "stalled",
        }
    }
}

impl FromStr for AgentStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "starting" => Ok(Self::Starting),
            "idle" => Ok(Self::Idle),
            "working" => Ok(Self::Working),
            "stalled" => Ok(Self::Stalled),
            _ => Err(format!("unknown agent status: {s}")),
        }
    }
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_status_roundtrip() {
        for status in [
            AgentStatus::Starting,
            AgentStatus::Idle,
            AgentStatus::Working,
            AgentStatus::Stalled,
        ] {
            let s = status.as_str();
            let parsed: AgentStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
            assert_eq!(status.to_string(), s);
        }
    }

    #[test]
    fn agent_status_rejects_unknown() {
        let result = "unknown_value".parse::<AgentStatus>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown agent status"));
    }

    #[test]
    fn lifecycle_status_roundtrip() {
        for status in [
            LifecycleStatus::Design,
            LifecycleStatus::Open,
            LifecycleStatus::Implementing,
            LifecycleStatus::Pushing,
            LifecycleStatus::InReview,
            LifecycleStatus::FeedbackCreating,
            LifecycleStatus::Merging,
            LifecycleStatus::Verifying,
            LifecycleStatus::AwaitingDispatch,
            LifecycleStatus::Done,
            LifecycleStatus::Cancelled,
        ] {
            let s = status.as_str();
            let parsed: LifecycleStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
            assert_eq!(status.to_string(), s);
        }
    }

    #[test]
    fn lifecycle_status_is_terminal() {
        assert!(LifecycleStatus::Done.is_terminal());
        assert!(LifecycleStatus::Cancelled.is_terminal());
        assert!(!LifecycleStatus::Open.is_terminal());
        assert!(!LifecycleStatus::Implementing.is_terminal());
        assert!(!LifecycleStatus::InReview.is_terminal());
    }

    #[test]
    fn lifecycle_status_rejects_unknown() {
        let result = "bogus_status".parse::<LifecycleStatus>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown lifecycle status"));
    }

    #[test]
    fn ticket_status_roundtrip() {
        for status in [
            TicketStatus::Open,
            TicketStatus::InProgress,
            TicketStatus::Closed,
        ] {
            let s = status.as_str();
            let parsed: TicketStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
            assert_eq!(status.to_string(), s);
        }
    }

    #[test]
    fn ticket_status_rejects_unknown() {
        let result = "bogus".parse::<TicketStatus>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown ticket status"));
    }
}
