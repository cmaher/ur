//! String constants for workflow event types.
//!
//! Use these instead of string literals when emitting or matching workflow
//! events in the coordinator, poller, and handlers.

// Lifecycle events — mirror lifecycle statuses as event names.
pub const AWAITING_DISPATCH: &str = "awaiting_dispatch";
pub const IMPLEMENTING: &str = "implementing";
pub const VERIFYING: &str = "verifying";
pub const PUSHING: &str = "pushing";
pub const IN_REVIEW: &str = "in_review";
pub const FEEDBACK_CREATING: &str = "feedback_creating";
pub const MERGING: &str = "merging";
pub const DONE: &str = "done";
pub const CANCELLED: &str = "cancelled";

// Condition events — fired when external state changes are detected.
pub const PR_CREATED: &str = "pr_created";
pub const CI_SUCCEEDED: &str = "ci_succeeded";
pub const CI_FAILED: &str = "ci_failed";
pub const REVIEW_APPROVED: &str = "review_approved";
pub const REVIEW_CHANGES_REQUESTED: &str = "review_changes_requested";
pub const MERGE_CONFLICT_DETECTED: &str = "merge_conflict_detected";
pub const STALLED: &str = "stalled";
