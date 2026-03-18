/// String constants for lifecycle statuses.
///
/// Use these instead of string literals when constructing gRPC requests or
/// comparing lifecycle_status fields on proto types.
pub const DESIGN: &str = "design";
pub const OPEN: &str = "open";
pub const IMPLEMENTING: &str = "implementing";
pub const PUSHING: &str = "pushing";
pub const IN_REVIEW: &str = "in_review";
pub const FEEDBACK_CREATING: &str = "feedback_creating";
pub const FEEDBACK_RESOLVING: &str = "feedback_resolving";
pub const VERIFYING: &str = "verifying";
pub const FIXING: &str = "fixing";
pub const DONE: &str = "done";
