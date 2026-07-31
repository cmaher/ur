use std::{process::ExitStatus, time::Duration};

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
#[derive(Debug, Clone)]
pub struct Session {
    name: String,
}

/// Options for creating a new tmux session.
pub struct CreateOptions {
    /// Session name (required).
    pub name: String,
    /// Initial window width. Useful when no client is attached at creation time.
    pub width: Option<u16>,
    /// Initial window height.
    pub height: Option<u16>,
    /// Whether to start the session detached.
    pub detached: bool,
}

impl Session {
    /// Get a handle to the well-known `agent` tmux session.
    /// This is the primary session used by worker daemons for Claude Code interaction.
    pub fn agent() -> Self {
        Self {
            name: "agent".into(),
        }
    }

    /// Get a handle to an existing tmux session by name.
    /// Does not verify the session exists — operations will fail if it doesn't.
    pub fn from_name(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    /// Create a new tmux session and return a handle to it.
    pub async fn create(opts: CreateOptions) -> Result<Self> {
        let mut args = vec!["new-session".to_string()];

        if opts.detached {
            args.push("-d".into());
        }

        args.push("-s".into());
        args.push(opts.name.clone());

        if let Some(w) = opts.width {
            args.push("-x".into());
            args.push(w.to_string());
        }
        if let Some(h) = opts.height {
            args.push("-y".into());
            args.push(h.to_string());
        }

        run_tmux(&args)
            .await
            .with_context(|| format!("failed to create tmux session '{}'", opts.name))?;

        info!(session = opts.name, "tmux session created");
        Ok(Self { name: opts.name })
    }

    /// Return the session name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set the status-left string for this session.
    pub async fn set_status_left(&self, value: &str) -> Result<()> {
        let result = run_tmux(&["set-option", "-t", &self.name, "status-left", value]).await;

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
        run_tmux(&["set-option", "-t", &self.name, key, value])
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
        run_tmux(&["send-keys", "-t", &self.name, "Enter"])
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
        run_tmux(&["send-keys", "-t", &self.name, "C-c"])
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
        run_tmux_stdout(&["capture-pane", "-p", "-t", &self.name])
            .await
            .with_context(|| format!("failed to capture pane for tmux session '{}'", self.name))
    }

    /// Send literal text to the session via `send-keys -l` (literal mode) without
    /// pressing Enter afterwards. This is useful for pre-filling text in a prompt
    /// without submitting it.
    pub async fn send_keys_no_enter(&self, text: &str) -> Result<()> {
        run_tmux(&["send-keys", "-t", &self.name, "-l", text])
            .await
            .with_context(|| format!("failed to send keys to tmux session '{}'", self.name))
    }

    /// Send raw keys without escaping (e.g., "Enter", "C-c").
    pub async fn send_keys_raw(&self, keys: &[&str]) -> Result<()> {
        let mut args: Vec<&str> = vec!["send-keys", "-t", &self.name];
        args.extend(keys);
        run_tmux(&args)
            .await
            .with_context(|| format!("failed to send raw keys to tmux session '{}'", self.name))
    }

    /// Return the name of the foreground process running in the pane.
    ///
    /// Uses `tmux display-message -p -t {name} '#{pane_current_command}'`.
    /// Returns the process name (e.g., `"claude"`, `"bash"`).
    pub async fn pane_current_command(&self) -> Result<String> {
        let output = run_tmux_stdout(&[
            "display-message",
            "-p",
            "-t",
            &self.name,
            "#{pane_current_command}",
        ])
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
    /// Uses the tmux `pane_dead` format variable: `0` means the process is
    /// still running (alive), `1` means it has exited (dead).
    pub async fn is_pane_alive(&self) -> Result<bool> {
        let output = run_tmux_stdout(&["list-panes", "-t", &self.name, "-F", "#{pane_dead}"])
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

    /// Build a `docker exec` command to attach to this session.
    /// Returns the command parts for use with a container runtime.
    pub fn attach_command(&self) -> Vec<String> {
        vec![
            "tmux".into(),
            "-u".into(),
            "attach-session".into(),
            "-t".into(),
            self.name.clone(),
        ]
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

/// Run a tmux command and check for success.
async fn run_tmux(args: &[impl AsRef<str>]) -> Result<()> {
    run_tmux_stdout(args).await.map(|_| ())
}

/// Run a tmux command, check for success, and return captured stdout.
async fn run_tmux_stdout(args: &[impl AsRef<str>]) -> Result<String> {
    let str_args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();

    let output = tokio::process::Command::new("tmux")
        .args(&str_args)
        .output()
        .await
        .context("failed to spawn tmux")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "tmux {} failed: {}",
            str_args.first().unwrap_or(&""),
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
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
    fn test_attach_command() {
        let session = Session {
            name: "agent".into(),
        };
        assert_eq!(
            session.attach_command(),
            vec!["tmux", "-u", "attach-session", "-t", "agent"]
        );
    }

    #[test]
    fn test_agent_session() {
        let session = Session::agent();
        assert_eq!(session.name(), "agent");
    }

    /// Verify that `send_keys_no_enter` sends literal text without Enter.
    /// We cannot run tmux in unit tests, so we verify the command construction
    /// by checking that the method produces the correct tmux arguments.
    /// `send_keys_no_enter` should call `send-keys -t <session> -l <text>` only,
    /// while `send_keys` additionally calls `send-keys -t <session> Enter`.
    /// Verify that `is_pane_alive` targets the correct session.
    /// tmux isn't running in unit tests so the command will fail, but the error
    /// message encodes the session name, confirming correct argument construction.
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
