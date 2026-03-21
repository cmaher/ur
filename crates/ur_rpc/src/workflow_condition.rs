//! String constants for workflow condition values.
//!
//! Use these instead of string literals when reading or updating condition
//! columns in the workflow tables.

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
