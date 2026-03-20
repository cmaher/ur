use ur_db::model::LifecycleStatus;

/// Pure-function router for workerd-driven lifecycle states.
///
/// Called when a worker signals step completion via `WorkflowStepComplete`.
/// Maps the current workflow status to the next lifecycle status.
#[derive(Clone, Default)]
pub struct WorkerdNextStepRouter;

/// Result of routing a step-complete signal through the `WorkerdNextStepRouter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextStepResult {
    /// Advance to the given lifecycle status.
    Advance { to: LifecycleStatus },
    /// Advance based on `feedback_mode` metadata on the ticket.
    /// The caller looks up `feedback_mode` and routes accordingly:
    /// - `now` -> Implementing
    /// - `later` -> Merging
    AdvanceByFeedbackMode,
    /// No routing defined for this status — ignore.
    Ignore,
}

impl WorkerdNextStepRouter {
    /// Determine the next lifecycle status after a worker completes its current step.
    pub fn route(&self, current_status: LifecycleStatus) -> NextStepResult {
        match current_status {
            LifecycleStatus::Implementing => NextStepResult::Advance {
                to: LifecycleStatus::Verifying,
            },
            LifecycleStatus::FeedbackCreating => NextStepResult::AdvanceByFeedbackMode,
            _ => NextStepResult::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router() -> WorkerdNextStepRouter {
        WorkerdNextStepRouter
    }

    #[test]
    fn implementing_advances_to_verifying() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing),
            NextStepResult::Advance {
                to: LifecycleStatus::Verifying
            },
        );
    }

    #[test]
    fn feedback_creating_advances_by_feedback_mode() {
        assert_eq!(
            router().route(LifecycleStatus::FeedbackCreating),
            NextStepResult::AdvanceByFeedbackMode,
        );
    }

    #[test]
    fn open_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Open),
            NextStepResult::Ignore,
        );
    }

    #[test]
    fn awaiting_dispatch_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::AwaitingDispatch),
            NextStepResult::Ignore,
        );
    }

    #[test]
    fn verifying_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Verifying),
            NextStepResult::Ignore,
        );
    }

    #[test]
    fn pushing_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing),
            NextStepResult::Ignore,
        );
    }

    #[test]
    fn in_review_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::InReview),
            NextStepResult::Ignore,
        );
    }

    #[test]
    fn merging_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Merging),
            NextStepResult::Ignore,
        );
    }

    #[test]
    fn design_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Design),
            NextStepResult::Ignore,
        );
    }

    #[test]
    fn done_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Done),
            NextStepResult::Ignore,
        );
    }
}
