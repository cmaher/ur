use std::process::ExitStatus;

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

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

    /// Send literal text to the session via `send-keys -l` (literal mode).
    /// The `-l` flag tells tmux to treat the argument as literal text, not key names.
    /// A separate `Enter` key is sent afterwards to submit the input.
    pub async fn send_keys(&self, text: &str) -> Result<()> {
        run_tmux(&["send-keys", "-t", &self.name, "-l", text])
            .await
            .with_context(|| format!("failed to send keys to tmux session '{}'", self.name))?;
        run_tmux(&["send-keys", "-t", &self.name, "Enter"])
            .await
            .with_context(|| format!("failed to send Enter to tmux session '{}'", self.name))
    }

    /// Send raw keys without escaping (e.g., "Enter", "C-c").
    pub async fn send_keys_raw(&self, keys: &[&str]) -> Result<()> {
        let mut args: Vec<&str> = vec!["send-keys", "-t", &self.name];
        args.extend(keys);
        run_tmux(&args)
            .await
            .with_context(|| format!("failed to send raw keys to tmux session '{}'", self.name))
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

/// Run a tmux command and check for success.
async fn run_tmux(args: &[impl AsRef<str>]) -> Result<()> {
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

    Ok(())
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
}
