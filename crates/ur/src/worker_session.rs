//! `ur worker read` / `keys` / `interrupt` and `--wait-idle`: drive and
//! observe a running worker's agent session through the server's
//! session-control RPCs (see `crates/server/src/grpc_session.rs`).

use anyhow::Result;
use serde::Serialize;
use tonic::transport::Channel;
use tracing::info;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::{
    CaptureWorkerPaneRequest, InterruptWorkerRequest, ReadWorkerTranscriptRequest,
    SendWorkerKeysRequest, TranscriptTurn, WaitWorkerIdleRequest,
};
use ur_rpc::session::{format_turns, parse_key};

use crate::output::OutputManager;

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

/// Block until the worker is idle; a timeout is a [`WaitIdleTimedOut`] error.
/// `timeout_secs: None` uses the server default.
pub async fn wait_idle(
    client: &mut CoreServiceClient<Channel>,
    worker_id: &str,
    timeout_secs: Option<u32>,
) -> Result<()> {
    info!(worker_id, timeout = ?timeout_secs, "waiting for worker to go idle");
    let resp = client
        .wait_worker_idle(WaitWorkerIdleRequest {
            worker_id: worker_id.to_owned(),
            timeout_secs: timeout_secs.unwrap_or(0),
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
    let client = &mut crate::connect(port).await?;
    if args.wait.wait_idle {
        wait_idle(client, worker_id, args.wait.timeout).await?;
    }
    if args.screen {
        let text = client
            .capture_worker_pane(CaptureWorkerPaneRequest {
                worker_id: worker_id.to_owned(),
                lines: args.lines,
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
    } else {
        let resp = client
            .read_worker_transcript(ReadWorkerTranscriptRequest {
                worker_id: worker_id.to_owned(),
                turns: args.turns.unwrap_or(0),
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
        .collect::<Result<Vec<_>, _>>()
        .map_err(anyhow::Error::msg)?;
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
