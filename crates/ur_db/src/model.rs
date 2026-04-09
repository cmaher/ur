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
    AddressingFeedback,
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
            Self::AddressingFeedback => "addressing_feedback",
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
            "addressing_feedback" => Ok(Self::AddressingFeedback),
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

/// Valid ticket types.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TicketType {
    #[default]
    Code,
    Design,
}

impl TicketType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Design => "design",
        }
    }

    /// All valid ticket type strings (including aliases).
    pub const VALID: &[&str] = &["code", "design", "task", "epic", "c", "d"];

    /// Normalize a ticket type string: maps aliases to their canonical form.
    ///
    /// Maps "task" → "code", "epic" → "code", "c" → "code", "d" → "design".
    /// All other values pass through unchanged.
    pub fn normalize(s: &str) -> String {
        match s {
            "task" | "epic" | "c" => "code".to_owned(),
            "d" => "design".to_owned(),
            other => other.to_owned(),
        }
    }
}

impl FromStr for TicketType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "code" | "task" | "epic" | "c" => Ok(Self::Code),
            "design" | "d" => Ok(Self::Design),
            _ => Err(format!(
                "invalid ticket type '{s}': valid types are {}",
                Self::VALID.join(", ")
            )),
        }
    }
}

impl fmt::Display for TicketType {
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
    pub children_completed: i32,
    pub children_total: i32,
}

#[derive(Default)]
pub struct NewTicket {
    pub id: Option<String>,
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
    /// When non-empty, filter to tickets whose status is in this list.
    pub statuses: Vec<String>,
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
}

pub struct WorkflowEvent {
    pub id: String,
    pub ticket_id: String,
    pub old_lifecycle_status: LifecycleStatus,
    pub new_lifecycle_status: LifecycleStatus,
    pub attempts: i32,
    pub created_at: String,
}

/// A row from the workflow_events table (lifecycle/condition events for a workflow).
pub struct WorkflowEventRow {
    pub event: String,
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
    pub ci_status: String,
    pub mergeable: String,
    pub review_status: String,
    pub node_id: String,
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
    pub node_id: String,
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
    pub node_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub idle_redispatch_count: i32,
}

pub struct WorkerSlot {
    pub worker_id: String,
    pub slot_id: String,
    pub created_at: String,
}

pub struct TicketComment {
    pub comment_id: String,
    pub ticket_id: String,
    pub pr_number: i64,
    pub gh_repo: String,
    pub reply_posted: bool,
    pub created_at: String,
}

pub struct UiEventRow {
    pub id: i64,
    pub entity_type: String,
    pub entity_id: String,
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
            LifecycleStatus::AddressingFeedback,
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

    #[test]
    fn ticket_type_roundtrip() {
        for tt in [TicketType::Code, TicketType::Design] {
            let s = tt.as_str();
            let parsed: TicketType = s.parse().unwrap();
            assert_eq!(parsed, tt);
            assert_eq!(tt.to_string(), s);
        }
    }

    #[test]
    fn ticket_type_aliases() {
        for alias in ["task", "epic", "c"] {
            let result = alias.parse::<TicketType>();
            assert!(
                result.is_ok(),
                "'{alias}' should be accepted as alias for code"
            );
            assert_eq!(result.unwrap(), TicketType::Code);
        }
        let result = "d".parse::<TicketType>();
        assert!(result.is_ok(), "'d' should be accepted as alias for design");
        assert_eq!(result.unwrap(), TicketType::Design);
    }

    #[test]
    fn ticket_type_normalize() {
        assert_eq!(TicketType::normalize("task"), "code");
        assert_eq!(TicketType::normalize("epic"), "code");
        assert_eq!(TicketType::normalize("c"), "code");
        assert_eq!(TicketType::normalize("code"), "code");
        assert_eq!(TicketType::normalize("d"), "design");
        assert_eq!(TicketType::normalize("design"), "design");
        assert_eq!(TicketType::normalize("other"), "other");
    }

    #[test]
    fn ticket_type_rejects_removed_types() {
        for invalid in ["bug", "feature", "chore"] {
            let result = invalid.parse::<TicketType>();
            assert!(result.is_err(), "should reject '{invalid}'");
            assert!(
                result.unwrap_err().contains("invalid ticket type"),
                "error message for '{invalid}'"
            );
        }
    }

    #[test]
    fn ticket_type_valid_list() {
        assert_eq!(
            TicketType::VALID,
            &["code", "design", "task", "epic", "c", "d"]
        );
    }
}
