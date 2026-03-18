/// String constants for lifecycle statuses.
///
/// Use these instead of string literals when working with lifecycle statuses
/// across crate boundaries (gRPC requests, database models, etc.).
pub const DESIGN: &str = "design";
pub const OPEN: &str = "open";
pub const IMPLEMENTING: &str = "implementing";
pub const PUSHING: &str = "pushing";
pub const IN_REVIEW: &str = "in_review";
pub const FEEDBACK_CREATING: &str = "feedback_creating";
pub const FEEDBACK_RESOLVING: &str = "feedback_resolving";
pub const STALLED: &str = "stalled";
pub const DONE: &str = "done";
