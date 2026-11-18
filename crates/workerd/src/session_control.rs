//! Host-facing session control for one worker: screen capture, transcript
//! reads, allowlisted key presses, interrupts and waiting for idle.
//!
//! [`SessionControlManager`] is injected into
//! [`WorkerDaemonServiceImpl`](crate::grpc_service::WorkerDaemonServiceImpl),
//! which delegates the `CapturePane` / `ReadTranscript` / `SendKeys` /
//! `Interrupt` / `WaitIdle` RPCs to it and reports busy/idle transitions
//! through [`SessionControlManager::idle`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tonic::Status;
use tracing::info;
use ur_config::AgentType;
use ur_rpc::proto::core::WorkerKey;
use ur_rpc::proto::workerd::{ReadTranscriptResponse, WaitIdleResponse};

use crate::transcript::{TranscriptError, read_transcript};

/// How long after an interrupt or key press the exit watcher defers a
/// shell-return verdict (see [`InterruptGrace`]).
pub const INTERRUPT_GRACE: Duration = Duration::from_secs(10);

/// Busy/idle state of the agent's turn. Idle is the agent's `Stop` hook
/// (`NotifyIdle`); a submitted message or dispatched command makes it busy.
/// Starts idle: a freshly launched agent sits at its prompt.
#[derive(Clone)]
pub struct IdleTracker {
    idle: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Default for IdleTracker {
    fn default() -> Self {
        IdleTracker {
            idle: Arc::new(tokio::sync::watch::Sender::new(true)),
        }
    }
}

impl IdleTracker {
    pub fn mark_busy(&self) {
        self.idle.send_replace(false);
    }

    pub fn mark_idle(&self) {
        self.idle.send_replace(true);
    }

    pub fn is_idle(&self) -> bool {
        *self.idle.borrow()
    }

    /// Wait until idle; `false` when `timeout` elapses first.
    pub async fn wait_idle(&self, timeout: Duration) -> bool {
        let mut rx = self.idle.subscribe();
        tokio::time::timeout(timeout, rx.wait_for(|idle| *idle))
            .await
            .is_ok_and(|r| r.is_ok())
    }
}

/// Shared between a worker's daemon and its exit watcher: while armed, the
/// watcher treats a shell foreground as "no change this poll" instead of an
/// agent exit, so an intentional interrupt (which can briefly hand the
/// terminal back while the agent tears a tool down) is never reported as the
/// agent exiting. A dead pane is still reported at once.
#[derive(Clone, Default)]
pub struct InterruptGrace {
    until: Arc<std::sync::Mutex<Option<Instant>>>,
}

impl InterruptGrace {
    pub fn arm(&self, duration: Duration) {
        *self.until.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now() + duration);
    }

    pub fn is_active(&self) -> bool {
        self.until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|until| Instant::now() < until)
    }
}

/// The tmux key name for an allowlisted [`WorkerKey`].
pub fn tmux_key_name(key: WorkerKey) -> Result<&'static str, Status> {
    Ok(match key {
        WorkerKey::Unspecified => {
            return Err(Status::invalid_argument("unspecified key"));
        }
        WorkerKey::Escape => "Escape",
        WorkerKey::CtrlC => "C-c",
        WorkerKey::Enter => "Enter",
        WorkerKey::Up => "Up",
        WorkerKey::Down => "Down",
        WorkerKey::Left => "Left",
        WorkerKey::Right => "Right",
        WorkerKey::Tab => "Tab",
        WorkerKey::Digit0 => "0",
        WorkerKey::Digit1 => "1",
        WorkerKey::Digit2 => "2",
        WorkerKey::Digit3 => "3",
        WorkerKey::Digit4 => "4",
        WorkerKey::Digit5 => "5",
        WorkerKey::Digit6 => "6",
        WorkerKey::Digit7 => "7",
        WorkerKey::Digit8 => "8",
        WorkerKey::Digit9 => "9",
    })
}

#[derive(Clone)]
pub struct SessionControlManager {
    session: tmux::Session,
    agent: AgentType,
    /// Home directory the agent runs under (its logs are in
    /// `<home>/<agent.home_subdir()>`).
    home: PathBuf,
    idle: IdleTracker,
    grace: InterruptGrace,
}

impl SessionControlManager {
    pub fn new(
        session: tmux::Session,
        agent: AgentType,
        home: PathBuf,
        idle: IdleTracker,
        grace: InterruptGrace,
    ) -> Self {
        SessionControlManager {
            session,
            agent,
            home,
            idle,
            grace,
        }
    }

    pub fn idle(&self) -> &IdleTracker {
        &self.idle
    }

    pub async fn capture(&self, lines: u32) -> Result<String, Status> {
        self.session
            .capture_agent_pane(lines)
            .await
            .map_err(|e| Status::internal(format!("{e:#}")))
    }

    pub async fn read_transcript(&self, turns: u32) -> Result<ReadTranscriptResponse, Status> {
        if turns == 0 {
            return Err(Status::invalid_argument("turns must be at least 1"));
        }
        // Checked before touching tmux: an agent with no log format fails fast.
        if self
            .agent
            .transcript_location(&self.home, std::path::Path::new("/"))
            .is_none()
        {
            return Err(transcript_status(TranscriptError::Unsupported {
                agent: self.agent.name(),
            }));
        }
        let cwd = self
            .session
            .agent_pane_current_path()
            .await
            .map_err(|e| Status::internal(format!("{e:#}")))?;
        let (agent, home) = (self.agent, self.home.clone());
        let transcript = tokio::task::spawn_blocking(move || {
            read_transcript(agent, &home, &cwd, turns as usize)
        })
        .await
        .map_err(|e| Status::internal(format!("transcript reader failed: {e}")))?
        .map_err(transcript_status)?;
        Ok(ReadTranscriptResponse {
            turns: transcript.turns,
            source_path: transcript.source_path.to_string_lossy().into_owned(),
        })
    }

    pub async fn send_keys(&self, keys: &[i32]) -> Result<(), Status> {
        if keys.is_empty() {
            return Err(Status::invalid_argument("no keys given"));
        }
        let names = keys
            .iter()
            .map(|k| {
                WorkerKey::try_from(*k)
                    .map_err(|_| Status::invalid_argument(format!("unknown key {k}")))
                    .and_then(tmux_key_name)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.press(&names).await
    }

    /// Interrupt the current turn with the agent's own interrupt keys. The
    /// agent does not run its `Stop` hook for an interrupted turn, so the
    /// worker is marked idle here.
    pub async fn interrupt(&self) -> Result<(), Status> {
        self.press(self.agent.interrupt_keys()).await?;
        self.idle.mark_idle();
        Ok(())
    }

    /// Wait for idle; on timeout, return the last `screen_lines` lines of the
    /// visible screen so the caller can see a prompt or dialog.
    pub async fn wait_idle(
        &self,
        timeout: Duration,
        screen_lines: usize,
    ) -> Result<WaitIdleResponse, Status> {
        if self.idle.wait_idle(timeout).await {
            return Ok(WaitIdleResponse {
                timed_out: false,
                screen_tail: String::new(),
            });
        }
        let screen = self.capture(0).await?;
        let lines: Vec<&str> = screen.lines().collect();
        let tail = lines[lines.len().saturating_sub(screen_lines)..].join("\n");
        Ok(WaitIdleResponse {
            timed_out: true,
            screen_tail: tail,
        })
    }

    async fn press(&self, names: &[&str]) -> Result<(), Status> {
        self.grace.arm(INTERRUPT_GRACE);
        info!(keys = ?names, "sending named keys to agent pane");
        self.session
            .send_named_keys_to_agent_pane(names)
            .await
            .map_err(|e| Status::internal(format!("{e:#}")))
    }
}

fn transcript_status(e: TranscriptError) -> Status {
    let message = e.to_string();
    match e {
        TranscriptError::Unsupported { .. } => Status::failed_precondition(message),
        TranscriptError::NotFound { .. } | TranscriptError::Empty { .. } => {
            Status::not_found(message)
        }
        TranscriptError::Io { .. } => Status::internal(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn wait_owned(idle: IdleTracker) -> bool {
        idle.wait_idle(Duration::from_secs(5)).await
    }

    #[tokio::test]
    async fn idle_tracker_wakes_waiter_and_times_out() {
        let idle = IdleTracker::default();
        assert!(idle.wait_idle(Duration::from_millis(1)).await);

        idle.mark_busy();
        assert!(!idle.wait_idle(Duration::from_millis(20)).await);

        let waiter = tokio::spawn(wait_owned(idle.clone()));
        tokio::time::sleep(Duration::from_millis(20)).await;
        idle.mark_idle();
        assert!(waiter.await.unwrap());
    }

    #[test]
    fn interrupt_grace_expires() {
        let grace = InterruptGrace::default();
        assert!(!grace.is_active());
        grace.arm(Duration::from_secs(60));
        assert!(grace.is_active());
        grace.arm(Duration::ZERO);
        assert!(!grace.is_active());
    }

    #[test]
    fn every_allowlisted_key_maps_to_a_tmux_name() {
        for k in 1..=19 {
            if let Ok(key) = WorkerKey::try_from(k) {
                assert!(tmux_key_name(key).is_ok(), "{key:?}");
            }
        }
        assert!(tmux_key_name(WorkerKey::Unspecified).is_err());
        assert_eq!(tmux_key_name(WorkerKey::Digit7).unwrap(), "7");
    }

    #[tokio::test]
    async fn agy_transcript_is_failed_precondition() {
        let mgr = SessionControlManager::new(
            tmux::Session::on_socket("ur-test-none", "missing"),
            AgentType::Agy,
            PathBuf::from("/nonexistent"),
            IdleTracker::default(),
            InterruptGrace::default(),
        );
        let err = mgr.read_transcript(5).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert!(err.message().contains("--screen"), "{}", err.message());
    }
}
