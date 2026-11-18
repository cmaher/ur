//! `ur worker read` / `keys` / `interrupt` and `--wait-idle`: drive and
//! observe a running worker's agent session through the server's
//! session-control RPCs (see `crates/server/src/grpc_session.rs`).

use anyhow::{Result, bail};
use serde::Serialize;
use tracing::info;
use ur_rpc::connection::HostChannel;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::{
    CaptureWorkerPaneRequest, InterruptWorkerRequest, ReadWorkerTranscriptRequest,
    SendWorkerKeysRequest, TranscriptTurn, WaitWorkerIdleRequest, WorkerKey,
};

use crate::output::OutputManager;

/// `--wait-idle [--timeout SECS]` as given on the command line.
#[derive(Clone, Copy, Debug)]
pub struct WaitIdle {
    /// `None` uses the server default.
    pub timeout_secs: Option<u32>,
}

/// The wait ended before the agent went idle. Carries the last screen lines
/// so the caller can see what the agent is stuck on (a permission prompt, a
/// dialog). Maps to its own exit code (`ErrorCode::TimedOut`).
#[derive(Debug)]
pub struct WaitIdleTimedOut {
    pub worker_id: String,
    pub screen_tail: String,
}

impl std::fmt::Display for WaitIdleTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "worker {} did not go idle before the timeout; last screen lines:\n{}",
            self.worker_id, self.screen_tail
        )
    }
}

impl std::error::Error for WaitIdleTimedOut {}

/// `--wait-idle [--timeout SECS]`, flattened into `send` and `read`.
#[derive(clap::Args, Debug, Clone, Copy)]
pub struct WaitIdleArgs {
    /// Block until the agent finishes its turn (its Stop hook fires)
    #[arg(long = "wait-idle")]
    pub wait_idle: bool,
    /// With --wait-idle: seconds to wait (default 600, max 3600)
    #[arg(long, requires = "wait_idle")]
    pub timeout: Option<u32>,
}

impl WaitIdleArgs {
    pub fn wait(self) -> Option<WaitIdle> {
        self.wait_idle.then_some(WaitIdle {
            timeout_secs: self.timeout,
        })
    }
}

/// `ur worker read` arguments.
#[derive(clap::Args, Debug, Clone)]
pub struct ReadArgs {
    pub worker_id: String,
    /// Show the rendered agent pane instead of the transcript
    #[arg(long)]
    pub screen: bool,
    /// With --screen: scrollback lines above the visible screen
    #[arg(long, default_value_t = 0, requires = "screen")]
    pub lines: u32,
    /// Number of most recent turns (default: server default, 10)
    #[arg(long, conflicts_with = "screen")]
    pub turns: Option<u32>,
    /// Print JSON (same as --output json; `main` switches the global output
    /// mode, so errors are JSON too)
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub wait: WaitIdleArgs,
}

/// What `ur worker read` shows.
#[derive(Clone, Copy, Debug)]
pub enum ReadMode {
    /// Rendered pane with this many scrollback lines.
    Screen { lines: u32 },
    /// The last turns of the agent's session log.
    Transcript { turns: u32 },
}

#[derive(Serialize)]
struct ScreenOutput<'a> {
    worker_id: &'a str,
    text: &'a str,
}

#[derive(Serialize)]
struct TranscriptOutput<'a> {
    worker_id: &'a str,
    source_path: &'a str,
    turns: &'a [TranscriptTurn],
}

#[derive(Serialize)]
struct KeysOutput<'a> {
    worker_id: &'a str,
    status: &'a str,
}

/// Parse a CLI key name (case-insensitive) into the allowlisted key.
pub fn parse_key(name: &str) -> Result<WorkerKey> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "escape" | "esc" => WorkerKey::Escape,
        "ctrl-c" | "c-c" | "ctrlc" => WorkerKey::CtrlC,
        "enter" | "return" => WorkerKey::Enter,
        "up" => WorkerKey::Up,
        "down" => WorkerKey::Down,
        "left" => WorkerKey::Left,
        "right" => WorkerKey::Right,
        "tab" => WorkerKey::Tab,
        "0" => WorkerKey::Digit0,
        "1" => WorkerKey::Digit1,
        "2" => WorkerKey::Digit2,
        "3" => WorkerKey::Digit3,
        "4" => WorkerKey::Digit4,
        "5" => WorkerKey::Digit5,
        "6" => WorkerKey::Digit6,
        "7" => WorkerKey::Digit7,
        "8" => WorkerKey::Digit8,
        "9" => WorkerKey::Digit9,
        other => bail!(
            "invalid key '{other}': expected one of escape, ctrl-c, enter, up, down, left, \
             right, tab, 0-9"
        ),
    })
}

/// Block until the worker is idle; a timeout is a [`WaitIdleTimedOut`] error.
pub async fn wait_idle(
    client: &mut CoreServiceClient<HostChannel>,
    worker_id: &str,
    wait: WaitIdle,
) -> Result<()> {
    info!(worker_id, timeout = ?wait.timeout_secs, "waiting for worker to go idle");
    let resp = client
        .wait_worker_idle(WaitWorkerIdleRequest {
            worker_id: worker_id.to_owned(),
            timeout_secs: wait.timeout_secs.unwrap_or(0),
            screen_lines: 0,
        })
        .await?
        .into_inner();
    if resp.timed_out {
        return Err(WaitIdleTimedOut {
            worker_id: worker_id.to_owned(),
            screen_tail: resp.screen_tail,
        }
        .into());
    }
    Ok(())
}

pub async fn read(port: u16, output: &OutputManager, args: ReadArgs) -> Result<()> {
    let worker_id = args.worker_id.as_str();
    crate::input::validate_id(worker_id, "worker_id")?;
    let mode = if args.screen {
        ReadMode::Screen { lines: args.lines }
    } else {
        ReadMode::Transcript {
            turns: args.turns.unwrap_or(0),
        }
    };
    let client = &mut crate::connect(port).await?;
    if let Some(wait) = args.wait.wait() {
        wait_idle(client, worker_id, wait).await?;
    }
    match mode {
        ReadMode::Screen { lines } => {
            let text = client
                .capture_worker_pane(CaptureWorkerPaneRequest {
                    worker_id: worker_id.to_owned(),
                    lines,
                })
                .await?
                .into_inner()
                .text;
            if output.is_json() {
                output.print_success(&ScreenOutput {
                    worker_id,
                    text: &text,
                });
            } else {
                println!("{text}");
            }
        }
        ReadMode::Transcript { turns } => {
            let resp = client
                .read_worker_transcript(ReadWorkerTranscriptRequest {
                    worker_id: worker_id.to_owned(),
                    turns,
                })
                .await?
                .into_inner();
            if output.is_json() {
                output.print_success(&TranscriptOutput {
                    worker_id,
                    source_path: &resp.source_path,
                    turns: &resp.turns,
                });
            } else {
                print!("{}", format_turns(&resp.turns));
            }
        }
    }
    Ok(())
}

pub async fn keys(
    port: u16,
    output: &OutputManager,
    worker_id: &str,
    names: &[String],
) -> Result<()> {
    crate::input::validate_id(worker_id, "worker_id")?;
    let keys = names
        .iter()
        .map(|n| parse_key(n))
        .collect::<Result<Vec<_>>>()?;
    let mut client = crate::connect(port).await?;
    client
        .send_worker_keys(SendWorkerKeysRequest {
            worker_id: worker_id.to_owned(),
            keys: keys.iter().map(|k| *k as i32).collect(),
        })
        .await?;
    report(
        output,
        worker_id,
        "keys_sent",
        &format!("Keys sent to {worker_id}."),
    );
    Ok(())
}

pub async fn interrupt(port: u16, output: &OutputManager, worker_id: &str) -> Result<()> {
    crate::input::validate_id(worker_id, "worker_id")?;
    let mut client = crate::connect(port).await?;
    client
        .interrupt_worker(InterruptWorkerRequest {
            worker_id: worker_id.to_owned(),
        })
        .await?;
    report(
        output,
        worker_id,
        "interrupted",
        &format!("Interrupted {worker_id}."),
    );
    Ok(())
}

fn report(output: &OutputManager, worker_id: &str, status: &str, text: &str) {
    if output.is_json() {
        output.print_success(&KeysOutput { worker_id, status });
    } else {
        println!("{text}");
    }
}

/// Plain-text rendering: a `[role]` header per turn, its text, then one
/// `  -> tool: summary` line per tool call.
fn format_turns(turns: &[TranscriptTurn]) -> String {
    let mut out = String::new();
    for turn in turns {
        out.push_str(&format!("[{}]\n", turn.role));
        if !turn.text.is_empty() {
            out.push_str(&turn.text);
            out.push('\n');
        }
        for call in &turn.tool_calls {
            out.push_str(&format!("  -> {}: {}\n", call.name, call.summary));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ur_rpc::proto::core::TranscriptToolCall;

    #[test]
    fn parse_key_accepts_allowlist_only() {
        assert_eq!(parse_key("Esc").unwrap(), WorkerKey::Escape);
        assert_eq!(parse_key("ctrl-c").unwrap(), WorkerKey::CtrlC);
        assert_eq!(parse_key("7").unwrap(), WorkerKey::Digit7);
        assert!(parse_key("C-x").is_err());
        assert!(parse_key("Enter; rm").is_err());
    }

    #[test]
    fn format_turns_lists_tool_calls() {
        let text = format_turns(&[TranscriptTurn {
            role: "assistant".into(),
            text: "Done.".into(),
            tool_calls: vec![TranscriptToolCall {
                name: "Bash".into(),
                summary: "cargo test".into(),
            }],
        }]);
        assert_eq!(text, "[assistant]\nDone.\n  -> Bash: cargo test\n\n");
    }
}
