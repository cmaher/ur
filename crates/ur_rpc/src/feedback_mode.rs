/// String constants for feedback_mode metadata values.
///
/// Set by the github poller or `ur flow autoapprove`, read by the
/// FeedbackResolveHandler to decide the resolution path.
pub const NOW: &str = "now";
pub const LATER: &str = "later";
