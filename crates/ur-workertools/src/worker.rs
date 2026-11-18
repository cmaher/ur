//! `ur-workertools worker list|read|send|keys|interrupt|wait-idle`: drive
//! other workers in this worker's project through the server's worker port.
//!
//! Mirrors `ur worker` (flags, `--output json` envelope, exit codes). The
//! server decides what is allowed: only manual workers may drive other
//! workers, and only same-project ones (`docs/plans/ur-o15mo-session-control.md`).

use clap::{Args, Subcommand, ValueEnum};
use serde_json::json;
use tonic::transport::Channel;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::{
    CaptureWorkerPaneRequest, InterruptWorkerRequest, ReadWorkerTranscriptRequest,
    SendWorkerKeysRequest, SendWorkerMessageRequest, WaitWorkerIdleRequest, WorkerListRequest,
    WorkerSummary,
};
use ur_rpc::session::{format_turns, parse_key};

use crate::{connect_core, inject_auth};

#[derive(Args)]
pub struct WorkerArgs {
    /// Output format
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    output: OutputFormat,
    #[command(subcommand)]
    command: WorkerCommands,
}

#[derive(ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
enum OutputFormat {
    Text,
    Json,
}

/// `--wait-idle [--timeout SECS]`, flattened into `send` and `read`.
#[derive(Args, Clone, Copy, Debug)]
struct WaitIdleArgs {
    /// Block until the agent finishes its turn
    #[arg(long = "wait-idle")]
    wait_idle: bool,
    /// With --wait-idle: seconds to wait (default 600, max 3600)
    #[arg(long, requires = "wait_idle")]
    timeout: Option<u32>,
}

#[derive(Subcommand, Debug)]
enum WorkerCommands {
    /// List running workers in this worker's project
    List,
    /// Read a worker's conversation (default) or its screen
    Read {
        worker_id: String,
        /// Show the rendered agent pane instead of the transcript
        #[arg(long)]
        screen: bool,
        /// With --screen: scrollback lines above the visible screen
        #[arg(long, default_value_t = 0, requires = "screen")]
        lines: u32,
        /// Number of most recent turns (default 10)
        #[arg(long, conflicts_with = "screen")]
        turns: Option<u32>,
        /// Same as --output json
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        wait: WaitIdleArgs,
    },
    /// Type a message into a worker's agent prompt
    Send {
        worker_id: String,
        message: String,
        /// Type the message without submitting it
        #[arg(long = "no-submit", conflicts_with = "wait_idle")]
        no_submit: bool,
        #[command(flatten)]
        wait: WaitIdleArgs,
    },
    /// Press keys in a worker's agent pane (escape, ctrl-c, enter, up, down,
    /// left, right, tab, 0-9)
    Keys {
        worker_id: String,
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Interrupt the worker agent's current turn
    Interrupt { worker_id: String },
    /// Block until the worker's agent is idle
    WaitIdle {
        worker_id: String,
        /// Seconds to wait (default 600, max 3600)
        #[arg(long)]
        timeout: Option<u32>,
    },
}

/// Why a command failed; maps to `ur`'s error codes and exit codes.
#[derive(Debug)]
enum Failure {
    Rpc(tonic::Status),
    Connect(String),
    Input(String),
    TimedOut {
        worker_id: String,
        screen_tail: String,
    },
}

impl From<tonic::Status> for Failure {
    fn from(status: tonic::Status) -> Self {
        Self::Rpc(status)
    }
}

impl Failure {
    /// `(code, exit code)`, matching `ur`'s `ErrorCode`.
    fn code(&self) -> (&'static str, i32) {
        match self {
            Self::Rpc(s) => match s.code() {
                tonic::Code::NotFound => ("not_found", 3),
                tonic::Code::InvalidArgument => ("invalid_input", 5),
                tonic::Code::Unavailable => ("server_not_running", 2),
                _ => ("grpc_error", 6),
            },
            Self::Connect(_) => ("server_not_running", 2),
            Self::Input(_) => ("invalid_input", 5),
            Self::TimedOut { .. } => ("timed_out", 10),
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Rpc(s) => s.message().to_owned(),
            Self::Connect(m) | Self::Input(m) => m.clone(),
            Self::TimedOut {
                worker_id,
                screen_tail,
            } => format!(
                "worker {worker_id} did not go idle before the timeout; last screen lines:\n{screen_tail}"
            ),
        }
    }
}

/// What a successful command prints: `data` in the JSON envelope, `text` otherwise.
struct Output {
    data: serde_json::Value,
    text: String,
}

pub async fn run(args: WorkerArgs) -> i32 {
    let json = args.output == OutputFormat::Json
        || matches!(args.command, WorkerCommands::Read { json: true, .. });
    match execute(args.command).await {
        Ok(out) if json => {
            println!("{}", json!({ "ok": true, "data": out.data }));
            0
        }
        Ok(out) => {
            print!("{}", out.text);
            0
        }
        Err(failure) => {
            let (code, exit) = failure.code();
            if json {
                eprintln!(
                    "{}",
                    json!({ "ok": false, "error": { "code": code, "message": failure.message() } })
                );
            } else {
                eprintln!("worker: {}", failure.message());
            }
            exit
        }
    }
}

async fn execute(command: WorkerCommands) -> Result<Output, Failure> {
    // Validate keys before connecting, like `ur worker keys`.
    let keys = match &command {
        WorkerCommands::Keys { keys, .. } => keys
            .iter()
            .map(|k| parse_key(k))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Failure::Input)?,
        _ => Vec::new(),
    };
    let client = &mut connect_core().await.map_err(Failure::Connect)?;
    match command {
        WorkerCommands::List => list(client).await,
        WorkerCommands::Read {
            worker_id,
            screen,
            lines,
            turns,
            wait,
            ..
        } => {
            if wait.wait_idle {
                wait_idle(client, &worker_id, wait.timeout).await?;
            }
            if screen {
                read_screen(client, worker_id, lines).await
            } else {
                read_transcript(client, worker_id, turns.unwrap_or(0)).await
            }
        }
        WorkerCommands::Send {
            worker_id,
            message,
            no_submit,
            wait,
        } => {
            client
                .send_worker_message(authed(SendWorkerMessageRequest {
                    worker_id: worker_id.clone(),
                    message,
                    submit: Some(!no_submit),
                }))
                .await?;
            if wait.wait_idle {
                wait_idle(client, &worker_id, wait.timeout).await?;
                return Ok(status(
                    &worker_id,
                    "idle",
                    "Message sent to {}; agent is idle.",
                ));
            }
            Ok(status(&worker_id, "sent", "Message sent to {}."))
        }
        WorkerCommands::Keys { worker_id, .. } => {
            client
                .send_worker_keys(authed(SendWorkerKeysRequest {
                    worker_id: worker_id.clone(),
                    keys: keys.iter().map(|k| *k as i32).collect(),
                }))
                .await?;
            Ok(status(&worker_id, "keys_sent", "Keys sent to {}."))
        }
        WorkerCommands::Interrupt { worker_id } => {
            client
                .interrupt_worker(authed(InterruptWorkerRequest {
                    worker_id: worker_id.clone(),
                }))
                .await?;
            Ok(status(&worker_id, "interrupted", "Interrupted {}."))
        }
        WorkerCommands::WaitIdle { worker_id, timeout } => {
            wait_idle(client, &worker_id, timeout).await?;
            Ok(status(&worker_id, "idle", "{} is idle."))
        }
    }
}

fn authed<T>(msg: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    inject_auth(&mut req);
    req
}

/// `{worker_id, status}` / a one-line message with `{}` replaced by the id.
fn status(worker_id: &str, status: &str, text: &str) -> Output {
    Output {
        data: json!({ "worker_id": worker_id, "status": status }),
        text: format!("{}\n", text.replace("{}", worker_id)),
    }
}

async fn wait_idle(
    client: &mut CoreServiceClient<Channel>,
    worker_id: &str,
    timeout: Option<u32>,
) -> Result<(), Failure> {
    let resp = client
        .wait_worker_idle(authed(WaitWorkerIdleRequest {
            worker_id: worker_id.to_owned(),
            timeout_secs: timeout.unwrap_or(0),
        }))
        .await?
        .into_inner();
    if resp.timed_out {
        return Err(Failure::TimedOut {
            worker_id: worker_id.to_owned(),
            screen_tail: resp.screen_tail,
        });
    }
    Ok(())
}

async fn list(client: &mut CoreServiceClient<Channel>) -> Result<Output, Failure> {
    let workers = client
        .worker_list(authed(WorkerListRequest {}))
        .await?
        .into_inner()
        .workers;
    Ok(Output {
        data: json!(workers),
        text: format_workers(&workers),
    })
}

async fn read_screen(
    client: &mut CoreServiceClient<Channel>,
    worker_id: String,
    lines: u32,
) -> Result<Output, Failure> {
    let text = client
        .capture_worker_pane(authed(CaptureWorkerPaneRequest {
            worker_id: worker_id.clone(),
            lines,
        }))
        .await?
        .into_inner()
        .text;
    Ok(Output {
        data: json!({ "worker_id": worker_id, "text": text }),
        text: format!("{text}\n"),
    })
}

async fn read_transcript(
    client: &mut CoreServiceClient<Channel>,
    worker_id: String,
    turns: u32,
) -> Result<Output, Failure> {
    let resp = client
        .read_worker_transcript(authed(ReadWorkerTranscriptRequest {
            worker_id: worker_id.clone(),
            turns,
        }))
        .await?
        .into_inner();
    Ok(Output {
        text: format_turns(&resp.turns),
        data: json!({
            "worker_id": worker_id,
            "source_path": resp.source_path,
            "turns": resp.turns,
        }),
    })
}

fn format_workers(workers: &[WorkerSummary]) -> String {
    if workers.is_empty() {
        return "No running workers.\n".to_owned();
    }
    fn dash(s: &str) -> &str {
        if s.is_empty() { "-" } else { s }
    }
    let mut out = format!(
        "{:<20} {:<12} {:<8} {:<12} {:<10} {}\n",
        "WORKER", "PROJECT", "MODE", "STATUS", "RUNTIME", "AGENT"
    );
    for w in workers {
        out.push_str(&format!(
            "{:<20} {:<12} {:<8} {:<12} {:<10} {}\n",
            w.worker_id,
            dash(&w.project_key),
            w.mode,
            dash(&w.agent_status),
            dash(&w.runtime),
            dash(&w.agent_type),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        worker: WorkerArgs,
    }

    #[test]
    fn failures_map_to_ur_exit_codes() {
        let rpc = |code| Failure::Rpc(tonic::Status::new(code, "m")).code();
        assert_eq!(rpc(tonic::Code::NotFound), ("not_found", 3));
        assert_eq!(rpc(tonic::Code::PermissionDenied), ("grpc_error", 6));
        assert_eq!(rpc(tonic::Code::InvalidArgument), ("invalid_input", 5));
        let timed_out = Failure::TimedOut {
            worker_id: "ur-a".into(),
            screen_tail: "Allow? 1/2".into(),
        };
        assert_eq!(timed_out.code(), ("timed_out", 10));
        assert!(timed_out.message().contains("Allow? 1/2"));
    }

    #[test]
    fn flags_mirror_ur_worker() {
        let parse = |args: &[&str]| {
            TestCli::try_parse_from([&["t"], args].concat()).map(|c| c.worker.command)
        };
        assert!(matches!(
            parse(&["read", "ur-a", "--screen", "--lines", "5"]).unwrap(),
            WorkerCommands::Read {
                screen: true,
                lines: 5,
                ..
            }
        ));
        assert!(parse(&["read", "ur-a", "--screen", "--turns", "2"]).is_err());
        assert!(parse(&["read", "ur-a", "--timeout", "5"]).is_err());
        assert!(parse(&["send", "ur-a", "hi", "--no-submit", "--wait-idle"]).is_err());
        assert!(parse(&["keys", "ur-a"]).is_err());
        let cli = TestCli::try_parse_from(["t", "list", "--output", "json"]).unwrap();
        assert_eq!(cli.worker.output, OutputFormat::Json);
    }

    #[test]
    fn text_output_shapes() {
        assert_eq!(format_workers(&[]), "No running workers.\n");
        assert_eq!(
            status("ur-a", "sent", "Message sent to {}.").text,
            "Message sent to ur-a.\n"
        );
    }
}
