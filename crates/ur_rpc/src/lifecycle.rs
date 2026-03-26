/// String constants for lifecycle statuses.
///
/// Use these instead of string literals when constructing gRPC requests or
/// comparing lifecycle_status fields on proto types.
pub const DESIGN: &str = "design";
pub const OPEN: &str = "open";
pub const IMPLEMENTING: &str = "implementing";
pub const PUSHING: &str = "pushing";
pub const IN_REVIEW: &str = "in_review";
pub const ADDRESSING_FEEDBACK: &str = "addressing_feedback";
pub const MERGING: &str = "merging";
pub const VERIFYING: &str = "verifying";
pub const AWAITING_DISPATCH: &str = "awaiting_dispatch";
pub const DONE: &str = "done";
pub const CANCELLED: &str = "cancelled";
