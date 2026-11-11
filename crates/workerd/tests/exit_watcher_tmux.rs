//! The exit watcher against a real tmux pane on a throwaway socket, with
//! `python3` stand-in agents. Requires `tmux` and `python3` on PATH. Never
//! touches the `ur` socket.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use tmux::Session;
use workerd::{AgentExitReason, AgentExitSink, ExitWatcherManager};

const POLL: Duration = Duration::from_millis(100);

/// Kills the test's tmux server when dropped, even if an assertion panics.
struct ServerGuard {
    socket: String,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
    }
}

#[derive(Default)]
struct RecordingSink {
    reasons: Mutex<Vec<AgentExitReason>>,
}

#[tonic::async_trait]
impl AgentExitSink for RecordingSink {
    async fn agent_exited(&self, reason: AgentExitReason) -> anyhow::Result<()> {
        self.reasons.lock().unwrap().push(reason);
        Ok(())
    }
}

fn is_shell(cmd: &str) -> bool {
    matches!(cmd, "bash" | "sh" | "zsh" | "dash" | "fish")
}

struct Agent {
    session: Session,
    sink: Arc<RecordingSink>,
    watcher: tokio::task::JoinHandle<anyhow::Result<AgentExitReason>>,
    _rc: TempDir,
    _guard: ServerGuard,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

fn is_python(cmd: &str) -> bool {
    cmd.to_lowercase().starts_with("python")
}

impl Agent {
    /// A code-worker watcher polling every `poll` on a fresh `zsh` pane whose
    /// `.zshrc` is `rc`, with `python3 -c <script>` typed ahead into it (as a
    /// launch does). Returns once the agent has been foreground for a few polls.
    async fn start(tag: &str, rc_body: &str, poll: Duration, script: &str) -> Agent {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let socket = format!("ur-test-ew-{tag}-{}-{nanos}", std::process::id());
        let guard = ServerGuard {
            socket: socket.clone(),
        };
        // Our own zsh rc dir, so the user's rc files don't run in the pane.
        let rc = TempDir::new().unwrap();
        std::fs::write(rc.path().join(".zshrc"), rc_body).unwrap();
        let zdotdir = format!("ZDOTDIR={}", rc.path().display());
        let created = std::process::Command::new("tmux")
            .args(["-L", &socket, "new-session", "-d", "-s", "agent"])
            .args(["-x", "220", "-y", "55", "-e", &zdotdir, "zsh"])
            .output()
            .unwrap();
        assert!(created.status.success(), "{created:?}");
        let session = Session::on_socket(&socket, "agent");
        let sink = Arc::new(RecordingSink::default());
        let watcher =
            ExitWatcherManager::new(Arc::new(session.clone()), sink.clone(), true, poll).spawn();
        session
            .send_keys(&format!("python3 -c '{script}'"))
            .await
            .unwrap();
        let agent = Agent {
            session,
            sink,
            watcher,
            _rc: rc,
            _guard: guard,
        };
        agent.wait_foreground("agent running", is_python).await;
        tokio::time::sleep(poll * 3).await;
        agent
    }

    async fn wait_foreground(&self, what: &str, want: impl Fn(&str) -> bool) {
        for _ in 0..200 {
            if want(&self.session.pane_current_command().await.unwrap()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// Several watcher polls with a shell in the foreground and no exit.
    async fn assert_not_stopped(&self) {
        tokio::time::sleep(POLL * 10).await;
        assert!(
            is_shell(&self.session.pane_current_command().await.unwrap()),
            "shell foreground for the whole window"
        );
        assert!(self.sink.reasons.lock().unwrap().is_empty());
        assert!(!self.watcher.is_finished());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn suspended_agent_is_not_an_exit() {
    let agent = Agent::start("ctrlz", "", POLL, "import time; time.sleep(30)").await;
    agent.session.send_keys_raw(&["C-z"]).await.unwrap();
    agent.wait_foreground("shell after Ctrl-Z", is_shell).await;
    agent.assert_not_stopped().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_child_holding_the_terminal_is_not_an_exit() {
    // An interactive shell takes the terminal (its own foreground process
    // group) and leaves it on exit, so tmux reports a shell name throughout.
    let agent = Agent::start(
        "childtty",
        "",
        POLL,
        r#"import subprocess, time; time.sleep(1); subprocess.run(["bash", "--norc", "--noprofile", "-i", "-c", "sleep 0.5"]); time.sleep(30)"#,
    )
    .await;
    agent
        .wait_foreground("child shell foreground", is_shell)
        .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    agent.assert_not_stopped().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_exit_to_shell_stops() {
    let mut agent = Agent::start("exit", "", POLL, "import time; time.sleep(1)").await;
    let reason = tokio::time::timeout(Duration::from_secs(20), &mut agent.watcher)
        .await
        .expect("watcher fired")
        .unwrap()
        .unwrap();
    assert_eq!(reason, AgentExitReason::AgentReturnedToShell);
    assert_eq!(
        *agent.sink.reasons.lock().unwrap(),
        vec![AgentExitReason::AgentReturnedToShell]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn short_rc_command_before_the_agent_is_not_an_exit() {
    // With zsh starting at s < 1.5s: the 2s poll lands in the rc's `sleep`
    // (a child; ends before 4s), the 4s poll in the childless shell gap
    // (builtin `zselect`) before the typed-ahead agent starts. One non-shell
    // poll must not arm the exit.
    let agent = Agent::start(
        "rcgap",
        "sleep 2.5\nzmodload zsh/zselect\nzselect -t 250\n",
        Duration::from_secs(2),
        "import time; time.sleep(30)",
    )
    .await;
    assert!(agent.sink.reasons.lock().unwrap().is_empty());
    assert!(!agent.watcher.is_finished());
    assert!(is_python(
        &agent.session.pane_current_command().await.unwrap()
    ));
}
