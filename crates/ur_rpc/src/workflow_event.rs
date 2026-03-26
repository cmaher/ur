//! Enum for workflow event types.
//!
//! Use this enum instead of string literals when emitting or matching workflow
//! events in the coordinator, poller, and handlers.

/// Identifies a workflow event type.
///
/// Use this enum instead of raw event strings when calling
/// `TicketRepo::insert_workflow_event` or `insert_workflow_event_at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowEvent {
    // Lifecycle events — mirror lifecycle statuses as event names.
    AwaitingDispatch,
    Implementing,
    Verifying,
    Pushing,
    InReview,
    AddressingFeedback,
    Merging,
    Done,
    Cancelled,

    // Condition events — fired when external state changes are detected.
    PrCreated,
    CiSucceeded,
    CiFailed,
    ReviewApproved,
    ReviewChangesRequested,
    MergeConflictDetected,
    Stalled,
}

impl WorkflowEvent {
    /// Returns the database string value for this event.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AwaitingDispatch => "awaiting_dispatch",
            Self::Implementing => "implementing",
            Self::Verifying => "verifying",
            Self::Pushing => "pushing",
            Self::InReview => "in_review",
            Self::AddressingFeedback => "addressing_feedback",
            Self::Merging => "merging",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::PrCreated => "pr_created",
            Self::CiSucceeded => "ci_succeeded",
            Self::CiFailed => "ci_failed",
            Self::ReviewApproved => "review_approved",
            Self::ReviewChangesRequested => "review_changes_requested",
            Self::MergeConflictDetected => "merge_conflict_detected",
            Self::Stalled => "stalled",
        }
    }
}

impl std::fmt::Display for WorkflowEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
