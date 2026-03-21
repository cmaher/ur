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
