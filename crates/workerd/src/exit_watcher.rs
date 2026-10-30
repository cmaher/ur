//! Agent exit detection.
//!
//! [`ExitWatcherManager`] polls the agent's tmux pane and reports through an
//! injected [`AgentExitSink`] when the agent has exited. It never decides what
//! "exited" means for the hosting process: the container binary's sink asks
//! the server to stop the container, while a host-side daemon can tear down
//! just its own worker.

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, info, warn};

/// Why the exit watcher concluded the agent is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExitReason {
    /// The agent's tmux pane is dead.
    PaneDead,
    /// Design worker: the agent ran and the foreground process returned to a shell.
    AgentReturnedToShell,
    /// The pane could not be queried; treated as exited.
    ProbeFailed,
}

/// Receives the exit watcher's single "agent exited" notification.
///
/// Implementations own the consequence (container shutdown, local teardown).
/// The watcher stops polling once `agent_exited` returns.
#[tonic::async_trait]
pub trait AgentExitSink: Send + Sync {
    async fn agent_exited(&self, reason: AgentExitReason);
}

/// The pane queries the exit watcher needs. Implemented for [`tmux::Session`];
/// abstracted so the watcher is testable without a tmux server.
#[tonic::async_trait]
pub trait AgentPane: Send + Sync {
    async fn is_pane_alive(&self) -> anyhow::Result<bool>;
    async fn pane_current_command(&self) -> anyhow::Result<String>;
}

#[tonic::async_trait]
impl AgentPane for tmux::Session {
    async fn is_pane_alive(&self) -> anyhow::Result<bool> {
        tmux::Session::is_pane_alive(self).await
    }

    async fn pane_current_command(&self) -> anyhow::Result<String> {
        tmux::Session::pane_current_command(self).await
    }
}

/// State machine for tracking whether the agent has started and then exited in a design worker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AgentWatchState {
    /// The agent hasn't started yet — the foreground process is still a shell.
    Waiting,
    /// The agent is running — a non-shell process is in the foreground.
    Running,
}

/// Returns `true` if `name` is a common shell process name.
fn is_shell_process(name: &str) -> bool {
    matches!(name, "bash" | "sh" | "zsh" | "dash" | "fish")
}

/// Advance the state machine given the current foreground process name.
///
/// Returns `(next_state, should_stop)`. `should_stop` is true when the state
/// transitions from `Running` back to a shell, indicating the agent has exited.
pub(crate) fn advance_agent_watch_state(
    state: AgentWatchState,
    foreground: &str,
) -> (AgentWatchState, bool) {
    match state {
        AgentWatchState::Waiting => {
            if is_shell_process(foreground) {
                (AgentWatchState::Waiting, false)
            } else {
                (AgentWatchState::Running, false)
            }
        }
        AgentWatchState::Running => (AgentWatchState::Running, is_shell_process(foreground)),
    }
}

/// Polls the agent pane and notifies an [`AgentExitSink`] once the agent exits.
#[derive(Clone)]
pub struct ExitWatcherManager {
    pane: Arc<dyn AgentPane>,
    sink: Arc<dyn AgentExitSink>,
    is_design_worker: bool,
    poll_interval: Duration,
}

impl ExitWatcherManager {
    /// `is_design_worker` enables the shell-return check: design workers exit
    /// when the agent process ends even though the pane (a shell) survives.
    pub fn new(
        pane: Arc<dyn AgentPane>,
        sink: Arc<dyn AgentExitSink>,
        is_design_worker: bool,
        poll_interval: Duration,
    ) -> Self {
        ExitWatcherManager {
            pane,
            sink,
            is_design_worker,
            poll_interval,
        }
    }

    /// Poll until the agent exits, invoke the sink once, then return.
    pub async fn run(&self) {
        info!(
            is_design_worker = self.is_design_worker,
            poll_interval = ?self.poll_interval,
            "exit watcher started"
        );

        let mut state = AgentWatchState::Waiting;
        loop {
            tokio::time::sleep(self.poll_interval).await;
            if let Some(reason) = self.check(&mut state).await {
                self.sink.agent_exited(reason).await;
                return;
            }
        }
    }

    /// One poll: `Some(reason)` when the agent has exited.
    async fn check(&self, state: &mut AgentWatchState) -> Option<AgentExitReason> {
        match self.pane.is_pane_alive().await {
            Ok(true) => {
                debug!("agent pane is alive");
                if self.is_design_worker {
                    self.check_design_worker_exit(state).await
                } else {
                    None
                }
            }
            Ok(false) => {
                info!("agent pane is dead");
                Some(AgentExitReason::PaneDead)
            }
            Err(e) => {
                warn!(error = %e, "failed to check pane status, treating as dead");
                Some(AgentExitReason::ProbeFailed)
            }
        }
    }

    /// Check the foreground process for a design worker and advance the state machine.
    async fn check_design_worker_exit(
        &self,
        state: &mut AgentWatchState,
    ) -> Option<AgentExitReason> {
        match self.pane.pane_current_command().await {
            Ok(foreground) => {
                let (next_state, stop) = advance_agent_watch_state(*state, &foreground);
                debug!(
                    foreground = %foreground,
                    state = ?state,
                    next_state = ?next_state,
                    "design worker foreground process check"
                );
                *state = next_state;
                if stop {
                    info!(foreground = %foreground, "agent process exited in design worker");
                    Some(AgentExitReason::AgentReturnedToShell)
                } else {
                    None
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to query pane_current_command, treating as stopped");
                Some(AgentExitReason::ProbeFailed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tokio::sync::Mutex;

    /// Scripted pane: each poll pops the next `(alive, foreground)` pair.
    struct ScriptedPane {
        alive: Mutex<VecDeque<anyhow::Result<bool>>>,
        foreground: Mutex<VecDeque<String>>,
    }

    impl ScriptedPane {
        fn new(alive: Vec<anyhow::Result<bool>>, foreground: Vec<&str>) -> Arc<Self> {
            Arc::new(ScriptedPane {
                alive: Mutex::new(alive.into()),
                foreground: Mutex::new(foreground.into_iter().map(String::from).collect()),
            })
        }
    }

    #[tonic::async_trait]
    impl AgentPane for ScriptedPane {
        async fn is_pane_alive(&self) -> anyhow::Result<bool> {
            self.alive.lock().await.pop_front().unwrap_or(Ok(true))
        }

        async fn pane_current_command(&self) -> anyhow::Result<String> {
            Ok(self
                .foreground
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| "node".into()))
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        reasons: Mutex<Vec<AgentExitReason>>,
    }

    #[tonic::async_trait]
    impl AgentExitSink for RecordingSink {
        async fn agent_exited(&self, reason: AgentExitReason) {
            self.reasons.lock().await.push(reason);
        }
    }

    fn watcher(
        pane: Arc<ScriptedPane>,
        sink: Arc<RecordingSink>,
        design: bool,
    ) -> ExitWatcherManager {
        ExitWatcherManager::new(pane, sink, design, Duration::from_millis(1))
    }

    #[tokio::test]
    async fn dead_pane_invokes_sink_once() {
        let pane = ScriptedPane::new(vec![Ok(true), Ok(true), Ok(false)], vec![]);
        let sink = Arc::new(RecordingSink::default());

        watcher(pane, sink.clone(), false).run().await;

        assert_eq!(*sink.reasons.lock().await, vec![AgentExitReason::PaneDead]);
    }

    #[tokio::test]
    async fn probe_error_invokes_sink() {
        let pane = ScriptedPane::new(vec![Err(anyhow::anyhow!("no server"))], vec![]);
        let sink = Arc::new(RecordingSink::default());

        watcher(pane, sink.clone(), false).run().await;

        assert_eq!(
            *sink.reasons.lock().await,
            vec![AgentExitReason::ProbeFailed]
        );
    }

    #[tokio::test]
    async fn design_worker_return_to_shell_invokes_sink() {
        let pane = ScriptedPane::new(vec![], vec!["zsh", "node", "node", "zsh"]);
        let sink = Arc::new(RecordingSink::default());

        watcher(pane, sink.clone(), true).run().await;

        assert_eq!(
            *sink.reasons.lock().await,
            vec![AgentExitReason::AgentReturnedToShell]
        );
    }

    #[tokio::test]
    async fn non_design_worker_ignores_shell_return() {
        // Shell in the foreground is not an exit for code workers; only the dead pane is.
        let pane = ScriptedPane::new(
            vec![Ok(true), Ok(true), Ok(true), Ok(false)],
            vec!["node", "zsh", "zsh"],
        );
        let sink = Arc::new(RecordingSink::default());

        watcher(pane, sink.clone(), false).run().await;

        assert_eq!(*sink.reasons.lock().await, vec![AgentExitReason::PaneDead]);
    }

    #[test]
    fn test_waiting_stays_waiting_on_shell() {
        let (state, stop) = advance_agent_watch_state(AgentWatchState::Waiting, "bash");
        assert_eq!(state, AgentWatchState::Waiting);
        assert!(!stop);
    }

    #[test]
    fn test_waiting_transitions_to_running_on_non_shell() {
        let (state, stop) = advance_agent_watch_state(AgentWatchState::Waiting, "vim");
        assert_eq!(state, AgentWatchState::Running);
        assert!(!stop);
    }

    #[test]
    fn test_running_stays_running_on_non_shell() {
        let (state, stop) = advance_agent_watch_state(AgentWatchState::Running, "vim");
        assert_eq!(state, AgentWatchState::Running);
        assert!(!stop);
    }

    #[test]
    fn test_running_triggers_stop_on_shell() {
        let (state, stop) = advance_agent_watch_state(AgentWatchState::Running, "bash");
        assert_eq!(state, AgentWatchState::Running);
        assert!(stop);
    }

    #[test]
    fn test_full_waiting_running_stop_sequence() {
        let state = AgentWatchState::Waiting;

        // Shell processes keep us in Waiting
        let (state, stop) = advance_agent_watch_state(state, "sh");
        assert_eq!(state, AgentWatchState::Waiting);
        assert!(!stop);

        // Non-shell transitions to Running
        let (state, stop) = advance_agent_watch_state(state, "vim");
        assert_eq!(state, AgentWatchState::Running);
        assert!(!stop);

        // Still running — Claude Code's real foreground process is `node`, not `claude`
        let (state, stop) = advance_agent_watch_state(state, "node");
        assert_eq!(state, AgentWatchState::Running);
        assert!(!stop);

        // Agent exits, shell returns → stop
        let (state, stop) = advance_agent_watch_state(state, "zsh");
        assert_eq!(state, AgentWatchState::Running);
        assert!(stop);
    }

    #[test]
    fn test_all_shell_names_recognized() {
        for shell in &["bash", "sh", "zsh", "dash", "fish"] {
            let (state, stop) = advance_agent_watch_state(AgentWatchState::Running, shell);
            assert_eq!(state, AgentWatchState::Running);
            assert!(stop, "shell '{shell}' should trigger stop");
        }
    }
}
