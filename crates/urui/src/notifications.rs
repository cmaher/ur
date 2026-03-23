use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use tracing::{debug, warn};
use ur_config::NotificationConfig;
use ur_rpc::lifecycle;
use ur_rpc::proto::ticket::WorkflowInfo;

/// Snapshot of a workflow's notification-relevant state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedFlowState {
    stalled: bool,
    status: String,
    pr_url: String,
    stall_reason: String,
}

impl TrackedFlowState {
    fn from_workflow(flow: &WorkflowInfo) -> Self {
        Self {
            stalled: flow.stalled,
            status: flow.status.clone(),
            pr_url: flow.pr_url.clone(),
            stall_reason: flow.stall_reason.clone(),
        }
    }
}

/// Manages desktop notifications for workflow state transitions.
///
/// Tracks previous workflow states and fires macOS `terminal-notifier`
/// notifications when meaningful transitions occur (stall, in-review).
#[derive(Clone)]
pub struct NotificationManager {
    config: NotificationConfig,
    available: bool,
    previous_states: HashMap<String, TrackedFlowState>,
    icon_path: Option<PathBuf>,
}

impl NotificationManager {
    /// Create a new `NotificationManager`. Checks whether `terminal-notifier`
    /// is available on PATH and sets the `available` flag accordingly.
    pub fn new(config: NotificationConfig) -> Self {
        let available = check_terminal_notifier_available();
        if !available {
            debug!("terminal-notifier not found on PATH; notifications disabled");
        }

        let icon_path = resolve_icon_path();

        Self {
            config,
            available,
            previous_states: HashMap::new(),
            icon_path,
        }
    }

    /// Returns `true` if `terminal-notifier` was found on PATH.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Bulk-seed known workflow states without firing any notifications.
    ///
    /// Call this on initial data load so that the first incremental update
    /// doesn't fire notifications for pre-existing states.
    pub fn seed_flows(&mut self, flows: &[WorkflowInfo]) {
        for flow in flows {
            self.previous_states.insert(
                flow.ticket_id.clone(),
                TrackedFlowState::from_workflow(flow),
            );
        }
    }

    /// Check a single workflow update for notification-worthy transitions.
    ///
    /// If the workflow was not previously tracked, it is seeded (no notification).
    /// Otherwise, transitions are detected and notifications fired as appropriate.
    pub fn check_flow_update(&mut self, flow: &WorkflowInfo) {
        let new_state = TrackedFlowState::from_workflow(flow);
        let ticket_id = &flow.ticket_id;

        match self.previous_states.get(ticket_id) {
            Some(prev) => {
                self.detect_and_notify(ticket_id, prev, &new_state);
            }
            None => {
                debug!(%ticket_id, "first-seen workflow, seeding without notification");
            }
        }

        self.previous_states.insert(ticket_id.clone(), new_state);
    }

    /// Detect transitions between previous and new state, firing notifications
    /// for enabled event types.
    fn detect_and_notify(&self, ticket_id: &str, prev: &TrackedFlowState, new: &TrackedFlowState) {
        // Stalled: false -> true
        if self.config.flow_stalled && !prev.stalled && new.stalled {
            let message = if new.stall_reason.is_empty() {
                "Workflow stalled".to_owned()
            } else {
                new.stall_reason.clone()
            };
            self.fire_notification(ticket_id, "Flow Stalled", &message, None);
        }

        // Status transition to in_review
        if self.config.flow_in_review
            && prev.status != lifecycle::IN_REVIEW
            && new.status == lifecycle::IN_REVIEW
        {
            let open_url = if new.pr_url.is_empty() {
                None
            } else {
                Some(new.pr_url.as_str())
            };
            self.fire_notification(
                ticket_id,
                "PR Ready for Review",
                &format!("{ticket_id} is ready for review"),
                open_url,
            );
        }
    }

    /// Fire a macOS notification via `terminal-notifier` (fire-and-forget).
    fn fire_notification(
        &self,
        ticket_id: &str,
        title: &str,
        message: &str,
        open_url: Option<&str>,
    ) {
        if !self.available {
            return;
        }

        let group = format!("ur-{ticket_id}");
        let mut cmd = Command::new("terminal-notifier");
        cmd.args(["-title", title])
            .args(["-message", message])
            .args(["-group", &group]);

        if let Some(url) = open_url {
            cmd.args(["-open", url]);
        }

        if let Some(ref icon) = self.icon_path {
            cmd.args(["-appIcon", &icon.to_string_lossy()]);
        }

        match cmd.spawn() {
            Ok(_) => {
                debug!(%ticket_id, %title, "notification fired");
            }
            Err(e) => {
                warn!(%ticket_id, error = %e, "failed to spawn terminal-notifier");
            }
        }
    }
}

/// Check if `terminal-notifier` is available on PATH by attempting to run it
/// with `--help` (which exits quickly).
fn check_terminal_notifier_available() -> bool {
    Command::new("terminal-notifier")
        .arg("-help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Look for an icon file at a well-known location.
fn resolve_icon_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(home).join(".ur").join("icon.png");
    if path.exists() { Some(path) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(flow_stalled: bool, flow_in_review: bool) -> NotificationConfig {
        NotificationConfig {
            flow_stalled,
            flow_in_review,
        }
    }

    fn make_flow(ticket_id: &str, status: &str, stalled: bool) -> WorkflowInfo {
        WorkflowInfo {
            ticket_id: ticket_id.to_owned(),
            status: status.to_owned(),
            stalled,
            stall_reason: if stalled {
                "blocked on dependency".to_owned()
            } else {
                String::new()
            },
            pr_url: if status == lifecycle::IN_REVIEW {
                "https://github.com/org/repo/pull/42".to_owned()
            } else {
                String::new()
            },
            ..Default::default()
        }
    }

    /// Helper that creates a manager with `available` forced to false
    /// so tests never actually spawn terminal-notifier.
    fn test_manager(config: NotificationConfig) -> NotificationManager {
        NotificationManager {
            config,
            available: false,
            previous_states: HashMap::new(),
            icon_path: None,
        }
    }

    #[test]
    fn seed_flows_populates_state_without_notification() {
        let mut mgr = test_manager(make_config(true, true));
        let flows = vec![
            make_flow("ur-a1", lifecycle::IMPLEMENTING, false),
            make_flow("ur-b2", lifecycle::IN_REVIEW, false),
        ];

        mgr.seed_flows(&flows);

        assert_eq!(mgr.previous_states.len(), 2);
        assert!(mgr.previous_states.contains_key("ur-a1"));
        assert!(mgr.previous_states.contains_key("ur-b2"));
    }

    #[test]
    fn first_seen_workflow_is_seeded_without_notification() {
        let mut mgr = test_manager(make_config(true, true));
        let flow = make_flow("ur-new", lifecycle::IMPLEMENTING, false);

        // First call should seed, not crash or fire
        mgr.check_flow_update(&flow);

        assert!(mgr.previous_states.contains_key("ur-new"));
    }

    #[test]
    fn stalled_transition_detected() {
        let config = make_config(true, true);
        let mut mgr = test_manager(config);

        // Seed with non-stalled state
        let flow_v1 = make_flow("ur-s1", lifecycle::IMPLEMENTING, false);
        mgr.seed_flows(&[flow_v1]);

        // Update to stalled - detect_and_notify is called internally
        let flow_v2 = make_flow("ur-s1", lifecycle::IMPLEMENTING, true);
        mgr.check_flow_update(&flow_v2);

        // State should be updated
        let state = mgr.previous_states.get("ur-s1").unwrap();
        assert!(state.stalled);
    }

    #[test]
    fn stalled_transition_not_detected_when_disabled() {
        let config = make_config(false, true);
        let mut mgr = test_manager(config);

        let flow_v1 = make_flow("ur-s2", lifecycle::IMPLEMENTING, false);
        mgr.seed_flows(&[flow_v1]);

        // Even with stall transition, config says don't notify
        let flow_v2 = make_flow("ur-s2", lifecycle::IMPLEMENTING, true);
        mgr.check_flow_update(&flow_v2);

        // State still updated for tracking
        let state = mgr.previous_states.get("ur-s2").unwrap();
        assert!(state.stalled);
    }

    #[test]
    fn in_review_transition_detected() {
        let config = make_config(true, true);
        let mut mgr = test_manager(config);

        let flow_v1 = make_flow("ur-r1", lifecycle::IMPLEMENTING, false);
        mgr.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-r1", lifecycle::IN_REVIEW, false);
        mgr.check_flow_update(&flow_v2);

        let state = mgr.previous_states.get("ur-r1").unwrap();
        assert_eq!(state.status, lifecycle::IN_REVIEW);
    }

    #[test]
    fn in_review_transition_not_detected_when_disabled() {
        let config = make_config(true, false);
        let mut mgr = test_manager(config);

        let flow_v1 = make_flow("ur-r2", lifecycle::IMPLEMENTING, false);
        mgr.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-r2", lifecycle::IN_REVIEW, false);
        mgr.check_flow_update(&flow_v2);

        let state = mgr.previous_states.get("ur-r2").unwrap();
        assert_eq!(state.status, lifecycle::IN_REVIEW);
    }

    #[test]
    fn no_notification_when_already_stalled() {
        let config = make_config(true, true);
        let mut mgr = test_manager(config);

        // Seed as already stalled
        let flow_v1 = make_flow("ur-s3", lifecycle::IMPLEMENTING, true);
        mgr.seed_flows(&[flow_v1]);

        // Still stalled - no false->true transition
        let flow_v2 = make_flow("ur-s3", lifecycle::IMPLEMENTING, true);
        mgr.check_flow_update(&flow_v2);

        let state = mgr.previous_states.get("ur-s3").unwrap();
        assert!(state.stalled);
    }

    #[test]
    fn no_notification_when_already_in_review() {
        let config = make_config(true, true);
        let mut mgr = test_manager(config);

        // Seed as already in_review
        let flow_v1 = make_flow("ur-r3", lifecycle::IN_REVIEW, false);
        mgr.seed_flows(&[flow_v1]);

        // Still in_review - no transition
        let flow_v2 = make_flow("ur-r3", lifecycle::IN_REVIEW, false);
        mgr.check_flow_update(&flow_v2);

        let state = mgr.previous_states.get("ur-r3").unwrap();
        assert_eq!(state.status, lifecycle::IN_REVIEW);
    }

    #[test]
    fn detect_and_notify_stalled_with_reason() {
        let config = make_config(true, true);
        let mgr = test_manager(config);

        let prev = TrackedFlowState {
            stalled: false,
            status: lifecycle::IMPLEMENTING.to_owned(),
            pr_url: String::new(),
            stall_reason: String::new(),
        };
        let new = TrackedFlowState {
            stalled: true,
            status: lifecycle::IMPLEMENTING.to_owned(),
            pr_url: String::new(),
            stall_reason: "blocked on dependency".to_owned(),
        };

        // This calls fire_notification internally, but available=false so no spawn
        mgr.detect_and_notify("ur-test", &prev, &new);
    }

    #[test]
    fn detect_and_notify_in_review_with_pr_url() {
        let config = make_config(true, true);
        let mgr = test_manager(config);

        let prev = TrackedFlowState {
            stalled: false,
            status: lifecycle::IMPLEMENTING.to_owned(),
            pr_url: String::new(),
            stall_reason: String::new(),
        };
        let new = TrackedFlowState {
            stalled: false,
            status: lifecycle::IN_REVIEW.to_owned(),
            pr_url: "https://github.com/org/repo/pull/99".to_owned(),
            stall_reason: String::new(),
        };

        mgr.detect_and_notify("ur-test", &prev, &new);
    }

    #[test]
    fn fire_notification_noop_when_unavailable() {
        let mgr = test_manager(make_config(true, true));
        // Should not panic or error
        mgr.fire_notification("ur-test", "Test", "test message", None);
        mgr.fire_notification(
            "ur-test",
            "Test",
            "test message",
            Some("https://example.com"),
        );
    }

    #[test]
    fn tracked_flow_state_from_workflow() {
        let flow = WorkflowInfo {
            ticket_id: "ur-x1".to_owned(),
            status: lifecycle::IN_REVIEW.to_owned(),
            stalled: true,
            stall_reason: "waiting".to_owned(),
            pr_url: "https://github.com/pull/1".to_owned(),
            ..Default::default()
        };
        let state = TrackedFlowState::from_workflow(&flow);
        assert!(state.stalled);
        assert_eq!(state.status, lifecycle::IN_REVIEW);
        assert_eq!(state.pr_url, "https://github.com/pull/1");
        assert_eq!(state.stall_reason, "waiting");
    }

    #[test]
    fn manager_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<NotificationManager>();
    }
}
