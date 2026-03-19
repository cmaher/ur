use ur_db::model::LifecycleStatus;
use ur_rpc::agent_status;

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
            agent_status::STALLED => StepAction::Ignore,

            agent_status::WORKING => StepAction::Redispatch { reminder: true },

            agent_status::IDLE => match lifecycle_status {
                LifecycleStatus::Implementing => StepAction::Advance {
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
            router().route(LifecycleStatus::Open, agent_status::IDLE, false),
            StepAction::Ignore,
        );
    }

    #[test]
    fn no_ticket_working() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing, agent_status::WORKING, false),
            StepAction::Ignore,
        );
    }

    #[test]
    fn no_ticket_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing, agent_status::STALLED, false),
            StepAction::Ignore,
        );
    }

    // ---------------------------------------------------------------
    // Open lifecycle — always Ignore regardless of agent status
    // ---------------------------------------------------------------

    #[test]
    fn open_idle() {
        assert_eq!(
            router().route(LifecycleStatus::Open, agent_status::IDLE, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn open_working() {
        assert_eq!(
            router().route(LifecycleStatus::Open, agent_status::WORKING, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn open_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Open, agent_status::STALLED, true),
            StepAction::Ignore,
        );
    }

    // ---------------------------------------------------------------
    // AwaitingDispatch lifecycle — always Ignore regardless of agent status
    // ---------------------------------------------------------------

    #[test]
    fn awaiting_dispatch_idle() {
        assert_eq!(
            router().route(LifecycleStatus::AwaitingDispatch, agent_status::IDLE, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn awaiting_dispatch_working() {
        assert_eq!(
            router().route(
                LifecycleStatus::AwaitingDispatch,
                agent_status::WORKING,
                true
            ),
            StepAction::Ignore,
        );
    }

    #[test]
    fn awaiting_dispatch_stalled() {
        assert_eq!(
            router().route(
                LifecycleStatus::AwaitingDispatch,
                agent_status::STALLED,
                true
            ),
            StepAction::Ignore,
        );
    }

    // ---------------------------------------------------------------
    // Stalled agent — always Ignore for any lifecycle status
    // ---------------------------------------------------------------

    #[test]
    fn implementing_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing, agent_status::STALLED, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn pushing_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing, agent_status::STALLED, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn feedback_creating_stalled() {
        assert_eq!(
            router().route(
                LifecycleStatus::FeedbackCreating,
                agent_status::STALLED,
                true
            ),
            StepAction::Ignore,
        );
    }

    #[test]
    fn verifying_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Verifying, agent_status::STALLED, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn in_review_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::InReview, agent_status::STALLED, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn merging_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Merging, agent_status::STALLED, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn design_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Design, agent_status::STALLED, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn done_stalled() {
        assert_eq!(
            router().route(LifecycleStatus::Done, agent_status::STALLED, true),
            StepAction::Ignore,
        );
    }

    // ---------------------------------------------------------------
    // Working agent — Redispatch with reminder for active lifecycle statuses
    // ---------------------------------------------------------------

    #[test]
    fn implementing_working() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing, agent_status::WORKING, true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn pushing_working() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing, agent_status::WORKING, true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn feedback_creating_working() {
        assert_eq!(
            router().route(
                LifecycleStatus::FeedbackCreating,
                agent_status::WORKING,
                true
            ),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn verifying_working() {
        assert_eq!(
            router().route(LifecycleStatus::Verifying, agent_status::WORKING, true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn in_review_working() {
        assert_eq!(
            router().route(LifecycleStatus::InReview, agent_status::WORKING, true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn merging_working() {
        assert_eq!(
            router().route(LifecycleStatus::Merging, agent_status::WORKING, true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn design_working() {
        assert_eq!(
            router().route(LifecycleStatus::Design, agent_status::WORKING, true),
            StepAction::Redispatch { reminder: true },
        );
    }

    #[test]
    fn done_working() {
        assert_eq!(
            router().route(LifecycleStatus::Done, agent_status::WORKING, true),
            StepAction::Redispatch { reminder: true },
        );
    }

    // ---------------------------------------------------------------
    // Idle agent — lifecycle-specific routing
    // ---------------------------------------------------------------

    #[test]
    fn implementing_idle_advances_to_verifying() {
        assert_eq!(
            router().route(LifecycleStatus::Implementing, agent_status::IDLE, true),
            StepAction::Advance {
                to: LifecycleStatus::Verifying
            },
        );
    }

    #[test]
    fn pushing_idle_redispatches() {
        assert_eq!(
            router().route(LifecycleStatus::Pushing, agent_status::IDLE, true),
            StepAction::Redispatch { reminder: false },
        );
    }

    #[test]
    fn feedback_creating_idle_redispatches() {
        assert_eq!(
            router().route(LifecycleStatus::FeedbackCreating, agent_status::IDLE, true),
            StepAction::Redispatch { reminder: false },
        );
    }

    #[test]
    fn verifying_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Verifying, agent_status::IDLE, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn in_review_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::InReview, agent_status::IDLE, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn merging_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Merging, agent_status::IDLE, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn design_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Design, agent_status::IDLE, true),
            StepAction::Ignore,
        );
    }

    #[test]
    fn done_idle_ignores() {
        assert_eq!(
            router().route(LifecycleStatus::Done, agent_status::IDLE, true),
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
