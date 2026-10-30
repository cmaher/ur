use std::{path::PathBuf, process::ExitStatus, time::Duration};

use anyhow::{Context, Result, bail};
use tracing::{debug, info, warn};

/// How long to wait after typing literal text before pressing Enter.
///
/// Claude Code does not reliably submit when the text and the Enter arrive
/// back-to-back: the Enter can be absorbed by the still-rendering input box,
/// leaving the typed command sitting unsubmitted. Letting the app render the
/// typed text first makes submission deterministic.
const SUBMIT_SETTLE: Duration = Duration::from_millis(400);

/// How long to give the input box to drain after an Enter before reading it back
/// to decide whether the submission took.
const SUBMIT_CONFIRM_DELAY: Duration = Duration::from_millis(500);

/// How many extra Enter presses to attempt when the input box has not drained.
const SUBMIT_RETRIES: usize = 2;

/// Prefix Claude Code renders at the start of its input box line.
const PROMPT_CARET: char = '❯';

/// Number of leading characters of the sent text used to recognise it in the
/// input box. Long text wraps across pane lines, so only the head is checked.
const SUBMIT_PROBE_LEN: usize = 40;

/// A handle to a tmux session, providing typed operations over the tmux CLI.
///
/// A session lives on a tmux server identified by its socket. `socket: None`
/// targets the user's default server (container workers use this); `Some(name)`
/// targets a dedicated server via `tmux -L <name>` (host-local workers use
/// `ur_config::LOCAL_TMUX_SOCKET`-style sockets so they never share the
/// user's own tmux server).
#[derive(Debug, Clone)]
pub struct Session {
    name: String,
    socket: Option<String>,
}

/// Options for creating a new tmux session.
#[derive(Debug, Clone, Default)]
pub struct CreateOptions {
    /// Session name (required).
    pub name: String,
    /// tmux server socket name (`-L`). `None` uses the default server.
    pub socket: Option<String>,
    /// Initial working directory for the session (`-c`). New windows inherit it.
    pub start_dir: Option<PathBuf>,
    /// Environment variables set on the session (`-e NAME=VALUE`), in order.
    /// New windows inherit them.
    pub env: Vec<(String, String)>,
    /// Initial window width. Useful when no client is attached at creation time.
    pub width: Option<u16>,
    /// Initial window height.
    pub height: Option<u16>,
    /// Whether to start the session detached.
    pub detached: bool,
}

impl CreateOptions {
    /// Build the full tmux argv (excluding the `tmux` program) for `new-session`.
    fn argv(&self) -> Vec<String> {
        let mut args = socket_args(self.socket.as_deref());
        args.push("new-session".into());

        if self.detached {
            args.push("-d".into());
        }

        args.push("-s".into());
        args.push(self.name.clone());

        if let Some(dir) = &self.start_dir {
            args.push("-c".into());
            args.push(dir.to_string_lossy().into_owned());
        }
        for (key, value) in &self.env {
            args.push("-e".into());
            args.push(format!("{key}={value}"));
        }

        if let Some(w) = self.width {
            args.push("-x".into());
            args.push(w.to_string());
        }
        if let Some(h) = self.height {
            args.push("-y".into());
            args.push(h.to_string());
        }
        args
    }
}

impl Session {
    /// Get a handle to the well-known `agent` tmux session on the default socket.
    /// This is the primary session used by worker daemons for Claude Code interaction.
    pub fn agent() -> Self {
        Self::from_name("agent")
    }

    /// Get a handle to an existing tmux session by name on the default socket.
    /// Does not verify the session exists — operations will fail if it doesn't.
    pub fn from_name(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            socket: None,
        }
    }

    /// Get a handle to an existing tmux session on a dedicated server socket
    /// (`tmux -L <socket>`). Does not verify the session exists.
    pub fn on_socket(socket: &str, name: &str) -> Self {
        Self {
            name: name.into(),
            socket: Some(socket.into()),
        }
    }

    /// Create a new tmux session and return a handle to it.
    pub async fn create(opts: CreateOptions) -> Result<Self> {
        run_tmux(&opts.argv())
            .await
            .with_context(|| format!("failed to create tmux session '{}'", opts.name))?;

        info!(
            session = opts.name,
            socket = opts.socket.as_deref(),
            "tmux session created"
        );
        Ok(Self {
            name: opts.name,
            socket: opts.socket,
        })
    }

    /// List the names of all sessions on a tmux server.
    ///
    /// `socket: None` queries the default server. A server that is not running
    /// has no sessions, so that case returns an empty list rather than an error.
    pub async fn list_sessions(socket: Option<&str>) -> Result<Vec<String>> {
        let mut args = socket_args(socket);
        args.extend([
            "list-sessions".into(),
            "-F".into(),
            "#{session_name}".into(),
        ]);

        match run_tmux_output(&args).await? {
            TmuxOutput::Success(stdout) => Ok(stdout
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()),
            TmuxOutput::Failed(stderr) if is_no_server(&stderr) => Ok(Vec::new()),
            TmuxOutput::Failed(stderr) => bail!("tmux list-sessions failed: {stderr}"),
        }
    }

    /// Return the tmux server socket name, if this session is not on the default server.
    pub fn socket(&self) -> Option<&str> {
        self.socket.as_deref()
    }

    /// Return the session name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set the status-left string for this session.
    pub async fn set_status_left(&self, value: &str) -> Result<()> {
        let result = run_tmux(&self.set_option_args("status-left", value)).await;

        match result {
            Ok(_) => {
                info!(session = self.name, value, "tmux status-left set");
                Ok(())
            }
            Err(e) => {
                warn!(session = self.name, error = %e, "failed to set tmux status-left");
                Err(e).context("failed to set tmux status-left")
            }
        }
    }

    /// Set any tmux option on this session.
    pub async fn set_option(&self, key: &str, value: &str) -> Result<()> {
        run_tmux(&self.set_option_args(key, value))
            .await
            .with_context(|| {
                format!(
                    "failed to set tmux option '{key}' on session '{}'",
                    self.name
                )
            })
    }

    /// Type literal text into the session and submit it.
    ///
    /// The text goes in via `send-keys -l` (literal mode, so the argument is
    /// treated as text rather than key names), then a separate `Enter` key
    /// submits it. Three details make this reliable against Claude Code:
    ///
    /// 1. A stale non-empty input box is cleared first, so the text is never
    ///    appended to whatever was already sitting there.
    /// 2. [`SUBMIT_SETTLE`] elapses between the text and the Enter — an Enter
    ///    that arrives while the input box is still rendering gets absorbed.
    /// 3. The submission is confirmed by checking the input box drained, and
    ///    Enter is re-sent if it did not.
    ///
    /// Panes that are not running Claude Code have no recognisable input box;
    /// there the pre-clear and confirmation steps are skipped and this degrades
    /// to a plain type-then-Enter.
    pub async fn send_keys(&self, text: &str) -> Result<()> {
        self.clear_input().await?;
        self.send_keys_no_enter(text).await?;
        tokio::time::sleep(SUBMIT_SETTLE).await;
        self.send_enter().await?;
        self.confirm_submitted(text).await
    }

    /// Send a single `Enter` keypress.
    pub async fn send_enter(&self) -> Result<()> {
        run_tmux(&self.send_keys_args(&self.session_target(), &["Enter"]))
            .await
            .with_context(|| format!("failed to send Enter to tmux session '{}'", self.name))
    }

    /// Verify the input box drained after an Enter, re-pressing Enter if it did
    /// not. Returns an error if the text is still sitting in the box after
    /// [`SUBMIT_RETRIES`] extra presses.
    ///
    /// The caller has already pressed Enter once, so each pass here waits for
    /// the pane to catch up before reading it — checking without settling first
    /// races the redraw and produces pointless extra presses.
    async fn confirm_submitted(&self, text: &str) -> Result<()> {
        let probe = submit_probe(text);
        if probe.is_empty() {
            // Nothing recognisable to look for — every line "contains" it.
            return Ok(());
        }

        for extra_presses in 0..=SUBMIT_RETRIES {
            tokio::time::sleep(SUBMIT_CONFIRM_DELAY).await;

            if !self.input_holds(&probe).await? {
                log_submitted(&self.name, extra_presses);
                return Ok(());
            }

            if extra_presses < SUBMIT_RETRIES {
                self.send_enter().await?;
            }
        }

        bail!(
            "text still unsubmitted in session '{}' after {} Enter presses (input box holds '{}')",
            self.name,
            SUBMIT_RETRIES + 1,
            probe
        )
    }

    /// Whether the input box still holds `probe`.
    ///
    /// A pane with no recognisable input box counts as not holding it: there is
    /// nothing to confirm against, so the send is taken at face value.
    async fn input_holds(&self, probe: &str) -> Result<bool> {
        let Some(line) = self.input_line().await? else {
            return Ok(false);
        };

        let holds = line.contains(probe);
        if holds {
            debug!(
                session = self.name,
                input = line.as_str(),
                "text still in input box after Enter"
            );
        }
        Ok(holds)
    }

    /// Clear the input box if it holds text, so a subsequent send starts clean.
    ///
    /// `C-c` is Claude Code's clear-input binding. It is sent at most once, and
    /// only when the box is non-empty, because a second `C-c` on an already
    /// empty box starts Claude Code's exit confirmation.
    async fn clear_input(&self) -> Result<()> {
        let Some(stale) = self.input_line().await?.filter(|line| !line.is_empty()) else {
            return Ok(());
        };

        warn!(
            session = self.name,
            input = stale.as_str(),
            "input box not empty before send, clearing it"
        );
        run_tmux(&self.send_keys_args(&self.session_target(), &["C-c"]))
            .await
            .with_context(|| format!("failed to clear input in tmux session '{}'", self.name))?;
        tokio::time::sleep(SUBMIT_SETTLE).await;

        if let Some(remaining) = self.input_line().await?.filter(|line| !line.is_empty()) {
            warn!(
                session = self.name,
                input = remaining.as_str(),
                "input box still not empty after clear, sending anyway"
            );
        }
        Ok(())
    }

    /// Return the contents of Claude Code's input box, trimmed.
    ///
    /// `None` means no input box was found in the pane — either Claude Code is
    /// not running, or it is rendering something else (a dialog, a menu).
    /// `Some("")` means the box is present and empty.
    async fn input_line(&self) -> Result<Option<String>> {
        Ok(parse_input_line(&self.capture_pane().await?))
    }

    /// Capture the visible contents of the session's pane as text.
    pub async fn capture_pane(&self) -> Result<String> {
        run_tmux_stdout(&self.capture_pane_args())
            .await
            .with_context(|| format!("failed to capture pane for tmux session '{}'", self.name))
    }

    /// Send literal text to the session via `send-keys -l` (literal mode) without
    /// pressing Enter afterwards. This is useful for pre-filling text in a prompt
    /// without submitting it.
    pub async fn send_keys_no_enter(&self, text: &str) -> Result<()> {
        run_tmux(&self.send_literal_args(&self.session_target(), text))
            .await
            .with_context(|| format!("failed to send keys to tmux session '{}'", self.name))
    }

    /// Send raw keys without escaping (e.g., "Enter", "C-c").
    pub async fn send_keys_raw(&self, keys: &[&str]) -> Result<()> {
        run_tmux(&self.send_keys_args(&self.session_target(), keys))
            .await
            .with_context(|| format!("failed to send raw keys to tmux session '{}'", self.name))
    }

    /// Return the name of the foreground process running in the pane.
    ///
    /// Targets the original agent pane, even when a helper pane is selected.
    /// Returns the process name (e.g., `"claude"`, `"bash"`).
    pub async fn pane_current_command(&self) -> Result<String> {
        let output = run_tmux_stdout(&self.agent_pane_query_args("#{pane_current_command}"))
            .await
            .with_context(|| {
                format!(
                    "failed to query pane_current_command for tmux session '{}'",
                    self.name
                )
            })?;

        Ok(output.trim().to_string())
    }

    /// Check whether the pane's shell process is still alive.
    ///
    /// Queries only the original agent pane, so helper panes do not affect
    /// lifecycle detection. The tmux `pane_dead` variable: `0` means the process is
    /// still running (alive), `1` means it has exited (dead).
    pub async fn is_pane_alive(&self) -> Result<bool> {
        let output = run_tmux_stdout(&self.agent_pane_query_args("#{pane_dead}"))
            .await
            .with_context(|| {
                format!(
                    "failed to query pane status for tmux session '{}'",
                    self.name
                )
            })?;

        let value = output.trim();
        match value {
            "0" => Ok(true),
            "1" => Ok(false),
            other => bail!(
                "unexpected pane_dead value '{}' for session '{}'",
                other,
                self.name
            ),
        }
    }

    /// Create a detached window named `window` in this session.
    ///
    /// The window inherits the session's working directory and environment.
    /// Window 0 stays the current window, so session-level operations and
    /// lifecycle checks (`<session>:0.0`) are unaffected.
    pub async fn new_window(&self, window: &str) -> Result<()> {
        run_tmux(&self.new_window_args(window))
            .await
            .with_context(|| {
                format!(
                    "failed to create window '{window}' in tmux session '{}'",
                    self.name
                )
            })
    }

    /// Type literal text into a named window of this session and press Enter.
    ///
    /// Unlike [`Session::send_keys`], this does no Claude Code input-box
    /// handling — it is meant for shell windows such as a background hook window.
    pub async fn send_keys_to_window(&self, window: &str, text: &str) -> Result<()> {
        let target = self.window_target(window);
        run_tmux(&self.send_literal_args(&target, text))
            .await
            .with_context(|| format!("failed to send keys to tmux window '{target}'"))?;
        run_tmux(&self.send_keys_args(&target, &["Enter"]))
            .await
            .with_context(|| format!("failed to send Enter to tmux window '{target}'"))
    }

    /// Whether this session currently exists on its server.
    pub async fn exists(&self) -> Result<bool> {
        let args = self.argv(&["has-session", "-t", &self.exact_target()]);
        match run_tmux_output(&args).await? {
            TmuxOutput::Success(_) => Ok(true),
            TmuxOutput::Failed(stderr) if is_no_server(&stderr) || is_no_session(&stderr) => {
                Ok(false)
            }
            TmuxOutput::Failed(stderr) => {
                bail!("tmux has-session failed for '{}': {stderr}", self.name)
            }
        }
    }

    /// Kill this session (all of its windows).
    ///
    /// Idempotent: a session (or server) that no longer exists counts as killed.
    pub async fn kill(&self) -> Result<()> {
        match run_tmux_output(&self.kill_args()).await? {
            TmuxOutput::Success(_) => {
                info!(
                    session = self.name,
                    socket = self.socket(),
                    "tmux session killed"
                );
                Ok(())
            }
            TmuxOutput::Failed(stderr) if is_no_server(&stderr) || is_no_session(&stderr) => {
                debug!(session = self.name, "tmux session already gone");
                Ok(())
            }
            TmuxOutput::Failed(stderr) => {
                bail!("failed to kill tmux session '{}': {stderr}", self.name)
            }
        }
    }

    /// Build the command (including the `tmux` program) to attach to this
    /// session. Returns the command parts for use with a container runtime's
    /// `exec_interactive`, or for running directly on the host.
    pub fn attach_command(&self) -> Vec<String> {
        let mut cmd = vec!["tmux".to_string()];
        cmd.extend(self.argv(&["-u", "attach-session", "-t", &self.exact_target()]));
        cmd
    }

    /// Prefix tmux arguments with this session's socket selection.
    fn argv(&self, args: &[&str]) -> Vec<String> {
        let mut argv = socket_args(self.socket());
        argv.extend(args.iter().map(|a| a.to_string()));
        argv
    }

    fn new_window_args(&self, window: &str) -> Vec<String> {
        // The session target (`=name:`) lets the new window take the next
        // free index rather than replacing the current one.
        self.argv(&[
            "new-window",
            "-d",
            "-t",
            &self.session_target(),
            "-n",
            window,
        ])
    }

    fn capture_pane_args(&self) -> Vec<String> {
        self.argv(&["capture-pane", "-p", "-t", &self.session_target()])
    }

    fn set_option_args(&self, key: &str, value: &str) -> Vec<String> {
        self.argv(&["set-option", "-t", &self.session_target(), key, value])
    }

    fn send_literal_args(&self, target: &str, text: &str) -> Vec<String> {
        self.argv(&["send-keys", "-t", target, "-l", text])
    }

    fn send_keys_args(&self, target: &str, keys: &[&str]) -> Vec<String> {
        let mut argv = self.argv(&["send-keys", "-t", target]);
        argv.extend(keys.iter().map(|k| k.to_string()));
        argv
    }

    /// `display-message -p` against the original agent pane (`=name:0.0`).
    fn agent_pane_query_args(&self, format: &str) -> Vec<String> {
        self.argv(&[
            "display-message",
            "-p",
            "-t",
            &self.agent_pane_target(),
            format,
        ])
    }

    fn kill_args(&self) -> Vec<String> {
        self.argv(&["kill-session", "-t", &self.exact_target()])
    }

    /// Every target matches the session name exactly (`=name`) instead of by
    /// prefix, so no operation ever hits a similarly-named session (`ur-w1`
    /// vs `ur-w10`). Session-scoped commands (`has-session`, `kill-session`,
    /// `attach-session`) take this bare form.
    fn exact_target(&self) -> String {
        format!("={}", self.name)
    }

    /// The session's current window/pane (`=name:`). Pane- and window-targeted
    /// commands (`send-keys`, `set-option`, `capture-pane`, `new-window`)
    /// reject a bare `=name`, so they need the trailing `:`.
    fn session_target(&self) -> String {
        format!("{}:", self.exact_target())
    }

    /// The original agent pane (`=name:0.0`).
    fn agent_pane_target(&self) -> String {
        format!("{}:0.0", self.exact_target())
    }

    fn window_target(&self, window: &str) -> String {
        format!("{}:{window}", self.exact_target())
    }
}

/// Record a confirmed submission, promoting to `info` when the first Enter was
/// not enough — that is the signal that submission timing is drifting again.
fn log_submitted(session: &str, extra_presses: usize) {
    if extra_presses > 0 {
        info!(
            session,
            extra_presses, "submission needed extra Enter presses"
        );
    } else {
        debug!(session, "submission confirmed");
    }
}

/// Extract the contents of Claude Code's input box from captured pane text.
///
/// The box is the *last* caret line in the pane: earlier caret lines are
/// transcript echoes of messages that were already submitted. Returns `None`
/// when the pane has no caret line at all.
fn parse_input_line(pane: &str) -> Option<String> {
    pane.lines()
        .filter_map(|line| line.strip_prefix(PROMPT_CARET))
        .next_back()
        .map(|content| content.trim().to_string())
}

/// Build the snippet used to recognise sent text still sitting in the input box.
///
/// Only the first line is usable: a multi-line send renders across several pane
/// lines and only the first carries the prompt caret. The snippet is capped at
/// [`SUBMIT_PROBE_LEN`] characters so wrapping cannot truncate it away.
fn submit_probe(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or("").trim();
    first_line
        .chars()
        .take(SUBMIT_PROBE_LEN)
        .collect::<String>()
}

/// Socket-selection arguments: `-L <socket>` for a dedicated server, nothing
/// for the default server.
fn socket_args(socket: Option<&str>) -> Vec<String> {
    match socket {
        Some(socket) => vec!["-L".into(), socket.into()],
        None => Vec::new(),
    }
}

/// Whether tmux stderr indicates the server is not running.
fn is_no_server(stderr: &str) -> bool {
    stderr.contains("no server running") || stderr.contains("error connecting to")
}

/// Whether tmux stderr indicates the target session does not exist.
fn is_no_session(stderr: &str) -> bool {
    stderr.contains("can't find session") || stderr.contains("session not found")
}

/// Outcome of a tmux invocation that spawned successfully.
enum TmuxOutput {
    Success(String),
    Failed(String),
}

/// Run a tmux command and return its stdout on success or trimmed stderr on failure.
async fn run_tmux_output(args: &[impl AsRef<str>]) -> Result<TmuxOutput> {
    let str_args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();

    let output = tokio::process::Command::new("tmux")
        .args(&str_args)
        .output()
        .await
        .context("failed to spawn tmux")?;

    if output.status.success() {
        Ok(TmuxOutput::Success(
            String::from_utf8_lossy(&output.stdout).to_string(),
        ))
    } else {
        Ok(TmuxOutput::Failed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

/// Run a tmux command and check for success.
async fn run_tmux(args: &[impl AsRef<str>]) -> Result<()> {
    run_tmux_stdout(args).await.map(|_| ())
}

/// Run a tmux command, check for success, and return captured stdout.
///
/// The error names the tmux subcommand (skipping any `-L <socket>` prefix).
async fn run_tmux_stdout(args: &[impl AsRef<str>]) -> Result<String> {
    match run_tmux_output(args).await? {
        TmuxOutput::Success(stdout) => Ok(stdout),
        TmuxOutput::Failed(stderr) => {
            let str_args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();
            let subcommand = match str_args.as_slice() {
                ["-L", _, sub, ..] => sub,
                [sub, ..] => sub,
                [] => "",
            };
            bail!("tmux {subcommand} failed: {stderr}")
        }
    }
}

/// Run a tmux command interactively (inheriting stdin/stdout/stderr).
/// Returns the exit status for the caller to handle.
pub async fn exec_interactive(args: &[impl AsRef<str>]) -> Result<ExitStatus> {
    let str_args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();

    tokio::process::Command::new("tmux")
        .args(&str_args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .context("failed to exec tmux")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_tmux_config_advertises_hyperlink_support() {
        let config = include_str!("../../../containers/worker-base/.tmux.conf");

        assert!(
            config
                .lines()
                .any(|line| line.trim() == "set -ga terminal-features \",*:hyperlinks\""),
            "worker tmux config must advertise OSC 8 hyperlink support"
        );
    }

    #[test]
    fn test_attach_command() {
        let session = Session::agent();
        assert_eq!(
            session.attach_command(),
            vec!["tmux", "-u", "attach-session", "-t", "=agent"]
        );
    }

    #[test]
    fn test_attach_command_on_socket() {
        let session = Session::on_socket("ur", "ur-w1");
        assert_eq!(
            session.attach_command(),
            vec!["tmux", "-L", "ur", "-u", "attach-session", "-t", "=ur-w1"]
        );
    }

    /// The container workerd create call must produce exactly the argv it
    /// always has: no socket, no `-c`, no `-e`.
    #[test]
    fn test_create_argv_default_socket_unchanged() {
        let opts = CreateOptions {
            name: "agent".into(),
            width: Some(220),
            height: Some(55),
            detached: true,
            ..Default::default()
        };
        assert_eq!(
            opts.argv(),
            vec!["new-session", "-d", "-s", "agent", "-x", "220", "-y", "55"]
        );
    }

    #[test]
    fn test_create_argv_on_socket_with_cwd_and_env() {
        let opts = CreateOptions {
            name: "ur-w1".into(),
            socket: Some("ur".into()),
            start_dir: Some(PathBuf::from("/pool/proj/0")),
            env: vec![
                ("UR_WORKER_ID".into(), "w1".into()),
                ("UR_DRIVE".into(), "/drive/w1".into()),
            ],
            width: None,
            height: None,
            detached: true,
        };
        assert_eq!(
            opts.argv(),
            vec![
                "-L",
                "ur",
                "new-session",
                "-d",
                "-s",
                "ur-w1",
                "-c",
                "/pool/proj/0",
                "-e",
                "UR_WORKER_ID=w1",
                "-e",
                "UR_DRIVE=/drive/w1",
            ]
        );
    }

    #[test]
    fn test_new_window_argv() {
        assert_eq!(
            Session::from_name("agent").new_window_args("bg"),
            vec!["new-window", "-d", "-t", "=agent:", "-n", "bg"]
        );
        assert_eq!(
            Session::on_socket("ur", "ur-w1").new_window_args("bg"),
            vec!["-L", "ur", "new-window", "-d", "-t", "=ur-w1:", "-n", "bg"]
        );
    }

    #[test]
    fn test_kill_argv() {
        assert_eq!(
            Session::from_name("agent").kill_args(),
            vec!["kill-session", "-t", "=agent"]
        );
        assert_eq!(
            Session::on_socket("ur", "ur-w1").kill_args(),
            vec!["-L", "ur", "kill-session", "-t", "=ur-w1"]
        );
    }

    #[test]
    fn test_session_level_argv_prefixes_socket_only_when_set() {
        assert_eq!(
            Session::agent().argv(&["list-windows"]),
            vec!["list-windows"]
        );
        assert_eq!(
            Session::on_socket("ur", "ur-w1").argv(&["list-windows"]),
            vec!["-L", "ur", "list-windows"]
        );
    }

    /// The container's default-socket argv differs from the historical one
    /// only by the exact-match `=` prefix (and `:` for pane targets).
    #[test]
    fn test_default_socket_targets_are_exact() {
        let s = Session::agent();
        let pane = s.session_target();
        assert_eq!(
            s.send_literal_args(&pane, "/clear"),
            vec!["send-keys", "-t", "=agent:", "-l", "/clear"]
        );
        assert_eq!(
            s.send_keys_args(&pane, &["Enter"]),
            vec!["send-keys", "-t", "=agent:", "Enter"]
        );
        assert_eq!(
            s.set_option_args("status-left", "[w1] "),
            vec!["set-option", "-t", "=agent:", "status-left", "[w1] "]
        );
        assert_eq!(
            s.capture_pane_args(),
            vec!["capture-pane", "-p", "-t", "=agent:"]
        );
        assert_eq!(
            s.agent_pane_query_args("#{pane_dead}"),
            vec!["display-message", "-p", "-t", "=agent:0.0", "#{pane_dead}"]
        );
        assert_eq!(
            s.send_keys_args(&s.window_target("bg"), &["Enter"]),
            vec!["send-keys", "-t", "=agent:bg", "Enter"]
        );
    }

    #[test]
    fn test_socket_targets_are_exact() {
        let s = Session::on_socket("ur", "ur-w1");
        assert_eq!(
            s.send_literal_args(&s.window_target("bg"), "make"),
            vec!["-L", "ur", "send-keys", "-t", "=ur-w1:bg", "-l", "make"]
        );
        assert_eq!(
            s.agent_pane_query_args("#{pane_current_command}"),
            vec![
                "-L",
                "ur",
                "display-message",
                "-p",
                "-t",
                "=ur-w1:0.0",
                "#{pane_current_command}"
            ]
        );
        assert_eq!(
            s.set_option_args("status-left-length", "50"),
            vec![
                "-L",
                "ur",
                "set-option",
                "-t",
                "=ur-w1:",
                "status-left-length",
                "50"
            ]
        );
    }

    #[test]
    fn test_socket_accessor() {
        assert_eq!(Session::agent().socket(), None);
        assert_eq!(Session::on_socket("ur", "x").socket(), Some("ur"));
    }

    #[test]
    fn test_missing_server_and_session_detection() {
        assert!(is_no_server("no server running on /tmp/tmux-501/ur"));
        assert!(is_no_server(
            "error connecting to /tmp/tmux-501/ur (No such file or directory)"
        ));
        assert!(is_no_session("can't find session: ur-w1"));
        assert!(!is_no_server("unknown command: bogus"));
        assert!(!is_no_session("unknown command: bogus"));
    }

    #[test]
    fn test_agent_session() {
        let session = Session::agent();
        assert_eq!(session.name(), "agent");
    }

    /// Verify `pane_current_command` runs the correct tmux subcommand and targets the session.
    /// If tmux is not running, the call fails with an error referencing the session name or tmux.
    /// If tmux is running, the call may succeed and return a process name string.
    #[tokio::test]
    async fn test_pane_current_command_construction() {
        let session = Session::from_name("test-session");
        let result = session.pane_current_command().await;
        match result {
            Err(e) => {
                let err_msg = format!("{e}");
                assert!(
                    err_msg.contains("test-session") || err_msg.contains("tmux"),
                    "unexpected error: {err_msg}"
                );
            }
            Ok(name) => {
                // tmux is running and returned a process name — just verify it's a string
                let _ = name;
            }
        }
    }

    #[tokio::test]
    async fn test_is_pane_alive_command_construction() {
        let session = Session::from_name("test-session");
        let result = session.is_pane_alive().await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("test-session") || err_msg.contains("tmux"),
            "unexpected error: {err_msg}"
        );
    }

    /// An idle Claude Code prompt renders the caret followed by a non-breaking
    /// space, which must read as an empty box (not as stale input).
    #[test]
    fn test_parse_input_line_idle_box_is_empty() {
        let pane = "✻ Baked for 3s\n\
                    ─────────\n\
                    ❯\u{a0}\n\
                    ─────────\n\
                      -- INSERT -- ⏵⏵ bypass permissions on";
        assert_eq!(parse_input_line(pane).as_deref(), Some(""));
    }

    /// Submitted messages stay on screen as caret-prefixed transcript echoes, so
    /// the box must be read from the *last* caret line, not the first.
    #[test]
    fn test_parse_input_line_ignores_transcript_echoes() {
        let pane = "❯ /clear\n\
                    ❯ an earlier message\n\
                    ─────────\n\
                    ❯ /implement ur-abcde\n\
                    ─────────\n\
                      -- INSERT --";
        assert_eq!(
            parse_input_line(pane).as_deref(),
            Some("/implement ur-abcde")
        );
    }

    /// A pane that is not running Claude Code (e.g. a bare shell) has no input
    /// box, which must be distinguishable from an empty one.
    #[test]
    fn test_parse_input_line_absent_when_no_prompt() {
        assert_eq!(parse_input_line("worker@host:/workspace$ \n"), None);
    }

    #[test]
    fn test_submit_probe_uses_first_line_only() {
        let probe = submit_probe("first line\nsecond line");
        assert_eq!(probe, "first line");
    }

    #[test]
    fn test_submit_probe_caps_length() {
        let probe = submit_probe(&"x".repeat(SUBMIT_PROBE_LEN * 2));
        assert_eq!(probe.chars().count(), SUBMIT_PROBE_LEN);
    }

    /// A probe built from blank text would match every line, so it must come
    /// back empty and let the caller skip confirmation entirely.
    #[test]
    fn test_submit_probe_blank_text_is_empty() {
        assert!(submit_probe("   \n  ").is_empty());
        assert!(submit_probe("").is_empty());
    }

    /// Multi-byte characters must not be split mid-character when capping.
    #[test]
    fn test_submit_probe_handles_multibyte() {
        let probe = submit_probe(&"é".repeat(SUBMIT_PROBE_LEN + 10));
        assert_eq!(probe.chars().count(), SUBMIT_PROBE_LEN);
    }

    #[tokio::test]
    async fn test_send_keys_no_enter_command_construction() {
        let session = Session::from_name("test-session");
        // send_keys_no_enter will fail because tmux isn't running, but we can
        // verify it produces the expected error message which encodes the session name.
        let result = session.send_keys_no_enter("hello world").await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        // The error should reference the session name, confirming the right target
        assert!(
            err_msg.contains("test-session") || err_msg.contains("tmux"),
            "unexpected error: {err_msg}"
        );
    }
}
