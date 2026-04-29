//! String constants for ticket metadata keys.
//!
//! These keys are used in `SetMetaRequest` and `get_meta` calls to identify
//! workflow-owned metadata on tickets.

/// Metadata key indicating the ticket should be auto-approved (skip human review).
pub const AUTOAPPROVE: &str = "autoapprove";

/// Metadata key controlling whether pre-push verification is skipped.
pub const NOVERIFY: &str = "noverify";

/// Metadata key controlling how feedback is delivered (now vs later).
pub const FEEDBACK_MODE: &str = "feedback_mode";

/// Metadata key for a reference label (e.g. a Jira ticket or external ID) to prepend to PR titles.
pub const REF: &str = "ref";

/// Metadata key storing the GitHub PR number associated with the ticket.
pub const PR_NUMBER: &str = "pr_number";
