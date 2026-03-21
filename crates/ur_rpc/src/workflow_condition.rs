//! String constants for workflow condition values.
//!
//! Use these instead of string literals when reading or updating condition
//! columns in the workflow tables.

/// Identifies which workflow condition to update.
///
/// Use this enum instead of raw column-name strings when calling
/// `TicketRepo::update_workflow_condition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowCondition {
    CiStatus,
    Mergeable,
    ReviewStatus,
}

impl WorkflowCondition {
    /// Returns the database column name for this condition.
    pub fn column_name(&self) -> &'static str {
        match self {
            Self::CiStatus => "ci_status",
            Self::Mergeable => "mergeable",
            Self::ReviewStatus => "review_status",
        }
    }
}

/// Values for the `ci_status` condition.
pub mod ci_status {
    pub const PENDING: &str = "pending";
    pub const SUCCEEDED: &str = "succeeded";
    pub const FAILED: &str = "failed";
}

/// Values for the `mergeable` condition.
pub mod mergeable {
    pub const UNKNOWN: &str = "unknown";
    pub const MERGEABLE: &str = "mergeable";
    pub const CONFLICT: &str = "conflict";
}

/// Values for the `review_status` condition.
pub mod review_status {
    pub const PENDING: &str = "pending";
    pub const APPROVED: &str = "approved";
    pub const CHANGES_REQUESTED: &str = "changes_requested";
}
