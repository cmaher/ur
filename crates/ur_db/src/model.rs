// Shared data types for ur_db (workflow and worker domain).

use std::fmt;
use std::str::FromStr;

pub use ticket_db::LifecycleStatus;

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
}
