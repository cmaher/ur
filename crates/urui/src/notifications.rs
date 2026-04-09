use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use tracing::{debug, warn};
use ur_config::NotificationConfig;
use ur_rpc::lifecycle;
use ur_rpc::proto::ticket::WorkflowInfo;

use super::cmd::Cmd;
use super::components::banner::BannerVariant;
use super::msg::Msg;

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

/// Desktop notification to be fired as a side effect.
#[derive(Debug, Clone)]
pub struct DesktopNotification {
    /// Ticket ID for grouping.
    pub ticket_id: String,
    /// Notification title.
    pub title: String,
    /// Notification body message.
    pub message: String,
    /// Optional URL to open when the notification is clicked.
    pub open_url: Option<String>,
}

/// Manages notification state tracking for workflow transitions in the v2 TEA loop.
///
/// Tracks previous workflow states and produces `Msg::BannerShow` messages and
/// `Cmd::FireDesktopNotification` commands when meaningful transitions occur
/// (stall, in-review).
#[derive(Debug, Clone)]
pub struct NotificationModel {
    config: NotificationConfig,
    desktop_available: bool,
    previous_states: HashMap<String, TrackedFlowState>,
}

impl NotificationModel {
    /// Create a new `NotificationModel`. Checks whether `terminal-notifier`
    /// is available on PATH and sets the `desktop_available` flag accordingly.
    pub fn new(config: NotificationConfig) -> Self {
        let desktop_available = check_terminal_notifier_available();
        if !desktop_available {
            debug!("terminal-notifier not found on PATH; desktop notifications disabled");
        }

        Self {
            config,
            desktop_available,
            previous_states: HashMap::new(),
        }
    }

    /// Returns `true` if `terminal-notifier` was found on PATH.
    pub fn is_desktop_available(&self) -> bool {
        self.desktop_available
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

    /// Process a batch of workflow updates and return any resulting messages
    /// and commands for detected transitions.
    ///
    /// For each workflow, if it was not previously tracked it is seeded without
    /// notification. Otherwise transitions are detected and banner messages +
    /// desktop notification commands are produced.
    pub fn process_flow_updates(&mut self, flows: &[WorkflowInfo]) -> (Vec<Msg>, Vec<Cmd>) {
        let mut msgs = Vec::new();
        let mut cmds = Vec::new();

        for flow in flows {
            let new_state = TrackedFlowState::from_workflow(flow);
            let ticket_id = &flow.ticket_id;

            if let Some(prev) = self.previous_states.get(ticket_id) {
                let (new_msgs, new_cmds) = self.detect_transitions(ticket_id, prev, &new_state);
                msgs.extend(new_msgs);
                cmds.extend(new_cmds);
            } else {
                debug!(%ticket_id, "first-seen workflow, seeding without notification");
            }

            self.previous_states.insert(ticket_id.clone(), new_state);
        }

        (msgs, cmds)
    }

    /// Detect transitions between previous and new state, producing banner
    /// messages and desktop notification commands for enabled event types.
    fn detect_transitions(
        &self,
        ticket_id: &str,
        prev: &TrackedFlowState,
        new: &TrackedFlowState,
    ) -> (Vec<Msg>, Vec<Cmd>) {
        let mut msgs = Vec::new();
        let mut cmds = Vec::new();

        // Stalled: false -> true
        if self.config.flow_stalled && !prev.stalled && new.stalled {
            let message = if new.stall_reason.is_empty() {
                format!("{ticket_id}: Workflow stalled")
            } else {
                format!("{ticket_id}: {}", new.stall_reason)
            };
            msgs.push(Msg::BannerShow {
                message: message.clone(),
                variant: BannerVariant::Error,
            });
            if self.desktop_available {
                cmds.push(Cmd::FireDesktopNotification(DesktopNotification {
                    ticket_id: ticket_id.to_owned(),
                    title: "Flow Stalled".to_owned(),
                    message,
                    open_url: None,
                }));
            }
        }

        // Status transition to in_review
        if self.config.flow_in_review
            && prev.status != lifecycle::IN_REVIEW
            && new.status == lifecycle::IN_REVIEW
        {
            let banner_msg = format!("{ticket_id} is ready for review");
            let open_url = if new.pr_url.is_empty() {
                None
            } else {
                Some(new.pr_url.clone())
            };
            msgs.push(Msg::BannerShow {
                message: banner_msg.clone(),
                variant: BannerVariant::Success,
            });
            if self.desktop_available {
                cmds.push(Cmd::FireDesktopNotification(DesktopNotification {
                    ticket_id: ticket_id.to_owned(),
                    title: "PR Ready for Review".to_owned(),
                    message: banner_msg,
                    open_url,
                }));
            }
        }

        (msgs, cmds)
    }
}

/// Fire a macOS desktop notification via `terminal-notifier` (fire-and-forget).
///
/// Called by the command runner when processing `Cmd::FireDesktopNotification`.
pub fn fire_desktop_notification(notification: &DesktopNotification, icon_path: Option<&PathBuf>) {
    let group = format!("ur-{}", notification.ticket_id);
    let mut cmd = Command::new("terminal-notifier");
    cmd.args(["-title", &notification.title])
        .args(["-message", &notification.message])
        .args(["-group", &group])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    if let Some(ref url) = notification.open_url {
        cmd.args(["-open", url]);
    }

    if let Some(icon) = icon_path {
        cmd.args(["-appIcon", &icon.to_string_lossy()]);
    }

    match cmd.spawn() {
        Ok(_) => {
            debug!(
                ticket_id = %notification.ticket_id,
                title = %notification.title,
                "desktop notification fired"
            );
        }
        Err(e) => {
            warn!(
                ticket_id = %notification.ticket_id,
                error = %e,
                "failed to spawn terminal-notifier"
            );
        }
    }
}

/// Check if `terminal-notifier` is available on PATH.
fn check_terminal_notifier_available() -> bool {
    Command::new("terminal-notifier")
        .arg("-help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
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

    /// Helper that creates a model with `desktop_available` forced to false
    /// so tests never actually spawn terminal-notifier.
    fn test_model(config: NotificationConfig) -> NotificationModel {
        NotificationModel {
            config,
            desktop_available: false,
            previous_states: HashMap::new(),
        }
    }

    #[test]
    fn seed_flows_populates_state_without_notification() {
        let mut model = test_model(make_config(true, true));
        let flows = vec![
            make_flow("ur-a1", lifecycle::IMPLEMENTING, false),
            make_flow("ur-b2", lifecycle::IN_REVIEW, false),
        ];

        model.seed_flows(&flows);

        assert_eq!(model.previous_states.len(), 2);
        assert!(model.previous_states.contains_key("ur-a1"));
        assert!(model.previous_states.contains_key("ur-b2"));
    }

    #[test]
    fn first_seen_workflow_is_seeded_without_notification() {
        let mut model = test_model(make_config(true, true));
        let flows = vec![make_flow("ur-new", lifecycle::IMPLEMENTING, false)];

        let (msgs, cmds) = model.process_flow_updates(&flows);

        assert!(msgs.is_empty());
        assert!(cmds.is_empty());
        assert!(model.previous_states.contains_key("ur-new"));
    }

    #[test]
    fn stalled_transition_produces_banner() {
        let mut model = test_model(make_config(true, true));
        let flow_v1 = make_flow("ur-s1", lifecycle::IMPLEMENTING, false);
        model.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-s1", lifecycle::IMPLEMENTING, true);
        let (msgs, cmds) = model.process_flow_updates(&[flow_v2]);

        // Should produce a banner (desktop_available=false so no cmds)
        assert_eq!(msgs.len(), 1);
        assert!(cmds.is_empty());
        match &msgs[0] {
            Msg::BannerShow { variant, message } => {
                assert!(matches!(variant, BannerVariant::Error));
                assert!(message.contains("ur-s1"));
            }
            other => panic!("expected BannerShow, got {other:?}"),
        }
    }

    #[test]
    fn stalled_transition_not_detected_when_disabled() {
        let mut model = test_model(make_config(false, true));
        let flow_v1 = make_flow("ur-s2", lifecycle::IMPLEMENTING, false);
        model.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-s2", lifecycle::IMPLEMENTING, true);
        let (msgs, cmds) = model.process_flow_updates(&[flow_v2]);

        assert!(msgs.is_empty());
        assert!(cmds.is_empty());
    }

    #[test]
    fn in_review_transition_produces_banner() {
        let mut model = test_model(make_config(true, true));
        let flow_v1 = make_flow("ur-r1", lifecycle::IMPLEMENTING, false);
        model.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-r1", lifecycle::IN_REVIEW, false);
        let (msgs, cmds) = model.process_flow_updates(&[flow_v2]);

        assert_eq!(msgs.len(), 1);
        assert!(cmds.is_empty());
        match &msgs[0] {
            Msg::BannerShow { variant, message } => {
                assert!(matches!(variant, BannerVariant::Success));
                assert!(message.contains("ur-r1"));
                assert!(message.contains("review"));
            }
            other => panic!("expected BannerShow, got {other:?}"),
        }
    }

    #[test]
    fn in_review_transition_not_detected_when_disabled() {
        let mut model = test_model(make_config(true, false));
        let flow_v1 = make_flow("ur-r2", lifecycle::IMPLEMENTING, false);
        model.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-r2", lifecycle::IN_REVIEW, false);
        let (msgs, cmds) = model.process_flow_updates(&[flow_v2]);

        assert!(msgs.is_empty());
        assert!(cmds.is_empty());
    }

    #[test]
    fn no_notification_when_already_stalled() {
        let mut model = test_model(make_config(true, true));
        let flow_v1 = make_flow("ur-s3", lifecycle::IMPLEMENTING, true);
        model.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-s3", lifecycle::IMPLEMENTING, true);
        let (msgs, cmds) = model.process_flow_updates(&[flow_v2]);

        assert!(msgs.is_empty());
        assert!(cmds.is_empty());
    }

    #[test]
    fn no_notification_when_already_in_review() {
        let mut model = test_model(make_config(true, true));
        let flow_v1 = make_flow("ur-r3", lifecycle::IN_REVIEW, false);
        model.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-r3", lifecycle::IN_REVIEW, false);
        let (msgs, cmds) = model.process_flow_updates(&[flow_v2]);

        assert!(msgs.is_empty());
        assert!(cmds.is_empty());
    }

    #[test]
    fn desktop_available_produces_cmd() {
        let mut model = NotificationModel {
            config: make_config(true, true),
            desktop_available: true,
            previous_states: HashMap::new(),
        };
        let flow_v1 = make_flow("ur-d1", lifecycle::IMPLEMENTING, false);
        model.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-d1", lifecycle::IMPLEMENTING, true);
        let (msgs, cmds) = model.process_flow_updates(&[flow_v2]);

        assert_eq!(msgs.len(), 1);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(&cmds[0], Cmd::FireDesktopNotification(_)));
    }

    #[test]
    fn in_review_with_pr_url_produces_cmd_with_url() {
        let mut model = NotificationModel {
            config: make_config(true, true),
            desktop_available: true,
            previous_states: HashMap::new(),
        };
        let flow_v1 = make_flow("ur-d2", lifecycle::IMPLEMENTING, false);
        model.seed_flows(&[flow_v1]);

        let flow_v2 = make_flow("ur-d2", lifecycle::IN_REVIEW, false);
        let (msgs, cmds) = model.process_flow_updates(&[flow_v2]);

        assert_eq!(msgs.len(), 1);
        assert_eq!(cmds.len(), 1);
        if let Cmd::FireDesktopNotification(notif) = &cmds[0] {
            assert!(notif.open_url.is_some());
            assert!(notif.open_url.as_ref().unwrap().contains("github"));
        } else {
            panic!("expected FireDesktopNotification cmd");
        }
    }

    #[test]
    fn model_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<NotificationModel>();
    }

    #[test]
    fn stalled_with_reason_includes_reason_in_message() {
        let mut model = test_model(make_config(true, true));
        let flow_v1 = make_flow("ur-sr", lifecycle::IMPLEMENTING, false);
        model.seed_flows(&[flow_v1]);

        let mut flow_v2 = make_flow("ur-sr", lifecycle::IMPLEMENTING, true);
        flow_v2.stall_reason = "CI failure".to_owned();
        let (msgs, _) = model.process_flow_updates(&[flow_v2]);

        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            Msg::BannerShow { message, .. } => {
                assert!(message.contains("CI failure"));
            }
            other => panic!("expected BannerShow, got {other:?}"),
        }
    }

    #[test]
    fn stalled_without_reason_uses_default_message() {
        let mut model = test_model(make_config(true, true));
        let flow_v1 = make_flow("ur-sn", lifecycle::IMPLEMENTING, false);
        model.seed_flows(&[flow_v1]);

        let flow_v2 = WorkflowInfo {
            ticket_id: "ur-sn".to_owned(),
            status: lifecycle::IMPLEMENTING.to_owned(),
            stalled: true,
            stall_reason: String::new(),
            pr_url: String::new(),
            ..Default::default()
        };
        let (msgs, _) = model.process_flow_updates(&[flow_v2]);

        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            Msg::BannerShow { message, .. } => {
                assert!(message.contains("Workflow stalled"));
            }
            other => panic!("expected BannerShow, got {other:?}"),
        }
    }
}
