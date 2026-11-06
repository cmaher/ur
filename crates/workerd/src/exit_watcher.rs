//! Agent exit detection.
//!
//! [`ExitWatcherManager`] polls the agent's tmux pane and reports through an
//! injected [`AgentExitSink`] when the agent has exited. It never decides what
//! "exited" means for the hosting process: the container binary's sink asks
//! the server to stop the container, while a host-side daemon can tear down
//! just its own worker. The owner spawns the watcher (see
//! [`ExitWatcherManager::spawn`]) and handles the task's result.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tracing::{debug, info, warn};

/// Why the exit watcher concluded the agent is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExitReason {
    /// The agent's tmux pane is dead.
    PaneDead,
    /// Code/design worker: the agent ran and the foreground process returned to a shell.
    AgentReturnedToShell,
    /// The pane could not be queried; treated as exited.
    ProbeFailed,
}

/// Receives the exit watcher's single "agent exited" notification.
///
/// Implementations own the consequence (container shutdown, local teardown).
/// The watcher stops polling once `agent_exited` returns and returns its
/// result from the watcher task, so a failed teardown reaches whoever awaits
/// the task's `JoinHandle`.
///
/// `agent_exited` runs *inside* the watcher task. A sink must not abort that
/// task (e.g. via the `JoinHandle` its owner holds) as part of teardown: the
/// abort would cancel the sink mid-teardown and drop its result. Report
/// failures through the returned `Result` instead.
#[tonic::async_trait]
pub trait AgentExitSink: Send + Sync {
    async fn agent_exited(&self, reason: AgentExitReason) -> anyhow::Result<()>;
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

/// State machine for tracking whether the agent has started and then exited
/// (workers where [`watches_agent_process`] holds).
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

/// Whether a worker of instruction strategy `strategy` (`UR_WORKER_INSTRUCTION_STRATEGY`,
/// a custom mode's base) is torn down when its agent process exits while the
/// pane's shell survives. Code and design workers are; manual workers (and an
/// unknown strategy) keep the pane so a person can keep working in it.
pub fn watches_agent_process(strategy: &str) -> bool {
    matches!(strategy, "code" | "design")
}

/// Polls the agent pane and notifies an [`AgentExitSink`] once the agent exits.
#[derive(Clone)]
pub struct ExitWatcherManager {
    pane: Arc<dyn AgentPane>,
    sink: Arc<dyn AgentExitSink>,
    watch_agent_process: bool,
    poll_interval: Duration,
}

impl ExitWatcherManager {
    /// `watch_agent_process` (see [`watches_agent_process`]) enables the
    /// shell-return check: the worker exits when the agent process ends even
    /// though the pane (a shell) survives.
    pub fn new(
        pane: Arc<dyn AgentPane>,
        sink: Arc<dyn AgentExitSink>,
        watch_agent_process: bool,
        poll_interval: Duration,
    ) -> Self {
        ExitWatcherManager {
            pane,
            sink,
            watch_agent_process,
            poll_interval,
        }
    }

    /// Run [`ExitWatcherManager::run`] on a new task. Await the handle for the
    /// sink's outcome, or abort it to stop watching (e.g. when the host tears
    /// this worker down for another reason).
    pub fn spawn(&self) -> tokio::task::JoinHandle<anyhow::Result<AgentExitReason>> {
        let watcher = self.clone();
        tokio::spawn(async move { watcher.run().await })
    }

    /// Poll until the agent exits, invoke the sink once, then return the exit
    /// reason, or the sink's error.
    pub async fn run(&self) -> anyhow::Result<AgentExitReason> {
        info!(
            watch_agent_process = self.watch_agent_process,
            poll_interval = ?self.poll_interval,
            "exit watcher started"
        );

        let mut state = AgentWatchState::Waiting;
        loop {
            tokio::time::sleep(self.poll_interval).await;
            if let Some(reason) = self.check(&mut state).await {
                self.sink
                    .agent_exited(reason)
                    .await
                    .with_context(|| format!("agent exit sink failed ({reason:?})"))?;
                return Ok(reason);
            }
        }
    }

    /// One poll: `Some(reason)` when the agent has exited.
    async fn check(&self, state: &mut AgentWatchState) -> Option<AgentExitReason> {
        match self.pane.is_pane_alive().await {
            Ok(true) => {
                debug!("agent pane is alive");
                if self.watch_agent_process {
                    self.check_agent_process_exit(state).await
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

    /// Check the pane's foreground process and advance the state machine.
    async fn check_agent_process_exit(
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
                    "agent foreground process check"
                );
                *state = next_state;
                if stop {
                    info!(foreground = %foreground, "agent process exited; pane returned to a shell");
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
        fail: bool,
    }

    #[tonic::async_trait]
    impl AgentExitSink for RecordingSink {
        async fn agent_exited(&self, reason: AgentExitReason) -> anyhow::Result<()> {
            self.reasons.lock().await.push(reason);
            if self.fail {
                anyhow::bail!("teardown failed");
            }
            Ok(())
        }
    }

    fn watcher(
        pane: Arc<ScriptedPane>,
        sink: Arc<RecordingSink>,
        watch_agent_process: bool,
    ) -> ExitWatcherManager {
        ExitWatcherManager::new(pane, sink, watch_agent_process, Duration::from_millis(1))
    }

    #[tokio::test]
    async fn dead_pane_invokes_sink_once() {
        let pane = ScriptedPane::new(vec![Ok(true), Ok(true), Ok(false)], vec![]);
        let sink = Arc::new(RecordingSink::default());

        let reason = watcher(pane, sink.clone(), false).run().await.unwrap();

        assert_eq!(reason, AgentExitReason::PaneDead);
        assert_eq!(*sink.reasons.lock().await, vec![AgentExitReason::PaneDead]);
    }

    #[tokio::test]
    async fn sink_error_is_returned_from_spawned_task() {
        let pane = ScriptedPane::new(vec![Ok(false)], vec![]);
        let sink = Arc::new(RecordingSink {
            fail: true,
            ..Default::default()
        });

        let err = watcher(pane, sink.clone(), false)
            .spawn()
            .await
            .expect("watcher task panicked")
            .unwrap_err();

        assert!(format!("{err:#}").contains("teardown failed"), "{err:#}");
        assert_eq!(*sink.reasons.lock().await, vec![AgentExitReason::PaneDead]);
    }

    #[tokio::test]
    async fn probe_error_invokes_sink() {
        let pane = ScriptedPane::new(vec![Err(anyhow::anyhow!("no server"))], vec![]);
        let sink = Arc::new(RecordingSink::default());

        watcher(pane, sink.clone(), false).run().await.unwrap();

        assert_eq!(
            *sink.reasons.lock().await,
            vec![AgentExitReason::ProbeFailed]
        );
    }

    /// Runs a watcher for `strategy` over a pane whose agent starts, runs a
    /// while, then returns to the shell; the pane dies a poll later.
    async fn shell_return_reasons(strategy: &str) -> Vec<AgentExitReason> {
        let pane = ScriptedPane::new(
            vec![Ok(true), Ok(true), Ok(true), Ok(true), Ok(true), Ok(false)],
            vec!["zsh", "node", "node", "zsh", "zsh"],
        );
        let sink = Arc::new(RecordingSink::default());
        watcher(pane, sink.clone(), watches_agent_process(strategy))
            .run()
            .await
            .unwrap();
        sink.reasons.lock().await.clone()
    }

    #[tokio::test]
    async fn code_and_design_workers_stop_on_shell_return() {
        for strategy in ["code", "design"] {
            assert_eq!(
                shell_return_reasons(strategy).await,
                vec![AgentExitReason::AgentReturnedToShell],
                "{strategy}"
            );
        }
    }

    #[tokio::test]
    async fn manual_worker_ignores_shell_return() {
        // Shell in the foreground is not an exit for manual workers (or an
        // unknown strategy); only the dead pane is.
        for strategy in ["manual", ""] {
            assert_eq!(
                shell_return_reasons(strategy).await,
                vec![AgentExitReason::PaneDead],
                "{strategy:?}"
            );
        }
    }

    #[tokio::test]
    async fn shell_before_agent_start_is_not_an_exit() {
        // The launch window: the pane is a shell until the agent's argv runs.
        let pane = ScriptedPane::new(
            vec![Ok(true), Ok(true), Ok(true), Ok(false)],
            vec!["zsh", "sh", "zsh"],
        );
        let sink = Arc::new(RecordingSink::default());

        watcher(pane, sink.clone(), true).run().await.unwrap();

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
