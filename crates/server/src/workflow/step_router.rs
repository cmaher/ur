use ur_db::model::LifecycleStatus;

/// Action to take when a worker reports its agent status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepAction {
    /// Advance the ticket's lifecycle to the given status.
    Advance { to: LifecycleStatus },
    /// Re-dispatch the current phase's RPC to the worker.
    /// When `reminder` is true, the worker is still working and this is
    /// a nudge rather than a cold re-dispatch.
    Redispatch { reminder: bool },
    /// No action needed.
    Ignore,
}

/// Pure-function router that maps `(lifecycle_status, agent_status)` pairs
/// to a `StepAction`.
///
/// This centralises the decision logic that was previously inlined in the
/// STOP / idle-redispatch handler in `grpc.rs`.
#[derive(Clone, Default)]
pub struct LifecycleStepRouter;

impl LifecycleStepRouter {
    pub fn new() -> Self {
        Self
    }

    /// Determine the step action for a worker that just reported the given
    /// `agent_status` while its assigned ticket is in `lifecycle_status`.
    ///
    /// `has_ticket` should be `false` when the worker has no assigned ticket
    /// (cold-start / unassigned worker).
    pub fn route(
        &self,
        lifecycle_status: LifecycleStatus,
        agent_status: &str,
        has_ticket: bool,
    ) -> StepAction {
        // No ticket assigned — nothing to do regardless of agent status.
        if !has_ticket {
            return StepAction::Ignore;
        }

        // Open tickets are not actively dispatched — ignore all statuses.
        if lifecycle_status == LifecycleStatus::Open {
            return StepAction::Ignore;
        }

        // AwaitingDispatch tickets are waiting for worker assignment — the
        // grpc handler triggers the transition to Implementing when the
        // worker reports idle, so the step router ignores all statuses here.
        if lifecycle_status == LifecycleStatus::AwaitingDispatch {
            return StepAction::Ignore;
        }

        match agent_status {
            "stalled" => StepAction::Ignore,

            "working" => StepAction::Redispatch { reminder: true },

            "idle" => match lifecycle_status {
                LifecycleStatus::Implementing => StepAction::Advance {
                    to: LifecycleStatus::Verifying,
                },
                LifecycleStatus::Fixing => StepAction::Advance {
                    to: LifecycleStatus::Verifying,
                },
                LifecycleStatus::Pushing => StepAction::Redispatch { reminder: false },
                LifecycleStatus::FeedbackCreating => StepAction::Redispatch { reminder: false },
                // All other lifecycle statuses with idle — no action.
                _ => StepAction::Ignore,
            },

            // Unknown agent status — treat as ignore.
            _ => StepAction::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router() -> LifecycleStepRouter {
        LifecycleStepRouter::new()
    }

    // ---------------------------------------------------------------
    // No ticket (cold start) — always Ignore regardless of agent status
    // ---------------------------------------------------------------

    #[test]
    fn no_ticket_idle() {
        assert_eq!(
            router().route(LifecycleStatus::Open, "idle", false),
            StepAction::Ignore,
        );
    }

    #[test]
    fn no_ticket_working() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing, "working", false),
            StepAction::Ignore,
        );
    }

    #[test]
    fn no_ticket_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing, "stalled", false),
            StepAction::Ignore,
        );
    }

    // ---------------------------------------------------------------
    // Open lifecycle — always Ignore regardless of agent status
    // ---------------------------------------------------------------

    #[test]
    fn open_idle() {
        assert_eq!(
            router().route(LifecycleStatus::Open, "idle", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn open_working() {
        assert_eq!(
            router().route(LifecycleStatus::Open, "working", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn open_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Open, "stalled", true),
            StepAction::Ignore,
        );
    }

    // ---------------------------------------------------------------
    // AwaitingDispatch lifecycle — always Ignore regardless of agent status
    // ---------------------------------------------------------------

    #[test]
    fn awaiting_dispatch_idle() {
        assert_eq!(
            router().route(LifecycleStatus::AwaitingDispatch, "idle", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn awaiting_dispatch_working() {
        assert_eq!(
            router().route(LifecycleStatus::AwaitingDispatch, "working", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn awaiting_dispatch_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::AwaitingDispatch, "stalled", true),
            StepAction::Ignore,
        );
    }

    // ---------------------------------------------------------------
    // Stalled agent — always Ignore for any lifecycle status
    // ---------------------------------------------------------------

    #[test]
    fn implementing_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing, "stalled", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn fixing_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Fixing, "stalled", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn pushing_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing, "stalled", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn feedback_creating_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::FeedbackCreating, "stalled", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn verifying_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Verifying, "stalled", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn in_review_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::InReview, "stalled", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn feedback_resolving_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::FeedbackResolving, "stalled", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn design_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Design, "stalled", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn done_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Done, "stalled", true),
            StepAction::Ignore,
        );
    }

    // ---------------------------------------------------------------
    // Working agent — Redispatch with reminder for active lifecycle statuses
    // ---------------------------------------------------------------

    #[test]
    fn implementing_working() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing, "working", true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn fixing_working() {
        assert_eq!(
            router().route(LifecycleStatus::Fixing, "working", true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn pushing_working() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing, "working", true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn feedback_creating_working() {
        assert_eq!(
            router().route(LifecycleStatus::FeedbackCreating, "working", true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn verifying_working() {
        assert_eq!(
            router().route(LifecycleStatus::Verifying, "working", true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn in_review_working() {
        assert_eq!(
            router().route(LifecycleStatus::InReview, "working", true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn feedback_resolving_working() {
        assert_eq!(
            router().route(LifecycleStatus::FeedbackResolving, "working", true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn design_working() {
        assert_eq!(
            router().route(LifecycleStatus::Design, "working", true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn done_working() {
        assert_eq!(
            router().route(LifecycleStatus::Done, "working", true),
            StepAction::Redispatch { reminder: true },
        );
    }

    // ---------------------------------------------------------------
    // Idle agent — lifecycle-specific routing
    // ---------------------------------------------------------------

    #[test]
    fn implementing_idle_advances_to_verifying() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing, "idle", true),
            StepAction::Advance {
                to: LifecycleStatus::Verifying
            },
        );
    }

    #[test]
    fn fixing_idle_advances_to_verifying() {
        assert_eq!(
            router().route(LifecycleStatus::Fixing, "idle", true),
            StepAction::Advance {
                to: LifecycleStatus::Verifying
            },
        );
    }

    #[test]
    fn pushing_idle_redispatches() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing, "idle", true),
            StepAction::Redispatch { reminder: false },
        );
    }

    #[test]
    fn feedback_creating_idle_redispatches() {
        assert_eq!(
            router().route(LifecycleStatus::FeedbackCreating, "idle", true),
            StepAction::Redispatch { reminder: false },
        );
    }

    #[test]
    fn verifying_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Verifying, "idle", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn in_review_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::InReview, "idle", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn feedback_resolving_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::FeedbackResolving, "idle", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn design_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Design, "idle", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn done_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Done, "idle", true),
            StepAction::Ignore,
        );
    }

    // ---------------------------------------------------------------
    // Unknown agent status — always Ignore
    // ---------------------------------------------------------------

    #[test]
    fn implementing_unknown_status() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing, "banana", true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn pushing_empty_status() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing, "", true),
            StepAction::Ignore,
        );
    }
}
