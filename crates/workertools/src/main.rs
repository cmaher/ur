mod agent;
mod repo;
mod status;

use std::io::Write;

use clap::{Parser, Subcommand};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Endpoint;

use ur_rpc::proto::core::command_output::Payload;
use ur_rpc::proto::hostexec::host_exec_message::Payload as HostExecPayload;
use ur_rpc::proto::hostexec::host_exec_service_client::HostExecServiceClient;
use ur_rpc::proto::hostexec::{HostExecMessage, HostExecRequest};
use ur_rpc::proto::rag::rag_service_client::RagServiceClient;
use ur_rpc::proto::rag::{Language, RagSearchRequest};
use ur_rpc::proto::workerd::NotifyIdleRequest;
use ur_rpc::proto::workerd::worker_daemon_service_client::WorkerDaemonServiceClient;

/// Inject worker auth headers (worker ID and secret) into a tonic request from environment variables.
pub(crate) fn inject_auth<T>(request: &mut tonic::Request<T>) {
    if let Ok(worker_id) = std::env::var(ur_config::UR_WORKER_ID_ENV)
        && let Ok(val) = worker_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::WORKER_ID_HEADER, val);
    }
    if let Ok(worker_secret) = std::env::var(ur_config::UR_WORKER_SECRET_ENV)
        && let Ok(val) =
            worker_secret.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::WORKER_SECRET_HEADER, val);
    }
}

#[derive(Parser)]
#[command(name = "workertools", about = "Ur worker toolkit")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a command on the host via ur-server
    HostExec {
        /// Enable bidirectional streaming (forward stdin to the remote command)
        #[arg(long)]
        bidi: bool,
        /// The command to execute
        command: String,
        /// Arguments to the command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// RAG operations
    Rag {
        #[command(subcommand)]
        command: RagCommands,
    },
    /// Interact with remote repositories
    Repo {
        #[command(subcommand)]
        command: repo::RepoCommands,
    },
    /// Notify workerd that Claude Code is idle (waiting for user input)
    NotifyIdle,
    /// Agent status signaling commands
    Status {
        #[command(subcommand)]
        command: status::StatusCommands,
    },
    /// Signal workerd that the current lifecycle step completed successfully (hidden alias for `status step-complete`)
    #[command(hide = true)]
    StepComplete,
    /// Agent lifecycle commands (hidden alias for `status request-human`)
    #[command(hide = true)]
    Agent {
        #[command(subcommand)]
        command: agent::AgentCommands,
    },
}

#[derive(Subcommand)]
enum RagCommands {
    /// Search indexed documentation via RAG
    Search {
        /// The search query
        query: String,
        /// Language to search (default: rust)
        #[arg(long, default_value = "rust")]
        language: String,
        /// Number of results to return (default: 5)
        #[arg(long, default_value_t = 5)]
        top_k: u32,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::HostExec {
            bidi,
            command,
            args,
        } => {
            std::process::exit(run_host_exec(&command, args, bidi).await);
        }
        Commands::Rag { command } => match command {
            RagCommands::Search {
                query,
                language,
                top_k,
            } => {
                std::process::exit(run_rag_search(&query, &language, top_k).await);
            }
        },
        Commands::Repo { command } => {
            std::process::exit(repo::run(command).await);
        }
        Commands::NotifyIdle => {
            run_notify_idle().await;
        }
        Commands::Status { command } => {
            std::process::exit(status::run(command).await);
        }
        Commands::StepComplete => {
            std::process::exit(status::run(status::StatusCommands::StepComplete).await);
        }
        Commands::Agent { command } => {
            std::process::exit(agent::run(command).await);
        }
    }
}

async fn forward_stdin_to_stream(tx: mpsc::Sender<HostExecMessage>) {
    use tokio::io::AsyncReadExt;
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 4096];
    loop {
        let n = match stdin.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let msg = HostExecMessage {
            payload: Some(HostExecPayload::Stdin(buf[..n].to_vec())),
        };
        if tx.send(msg).await.is_err() {
            break;
        }
    }
}

async fn run_host_exec(command: &str, args: Vec<String>, bidi: bool) -> i32 {
    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).expect("UR_SERVER_ADDR must be set");
    let addr = format!("http://{server_addr}");

    let channel = match Endpoint::try_from(addr).unwrap().connect().await {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("{command}: failed to connect to ur server: {e}");
            return 1;
        }
    };

    let mut client = HostExecServiceClient::new(channel);

    let working_dir = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/workspace".into());

    let start_msg = HostExecMessage {
        payload: Some(HostExecPayload::Start(HostExecRequest {
            command: command.into(),
            args,
            working_dir,
        })),
    };

    // Build the outbound stream: start frame first, then optional stdin forwarding.
    let (tx, rx) = mpsc::channel::<HostExecMessage>(32);
    tx.send(start_msg).await.unwrap();

    if bidi {
        tokio::spawn(forward_stdin_to_stream(tx));
    }
    // For non-bidi commands, `tx` is dropped here, closing the outbound stream
    // after the start frame. The server will not expect any stdin frames.

    let outbound = ReceiverStream::new(rx);

    let mut request = tonic::Request::new(outbound);
    inject_auth(&mut request);

    let response = match client.exec(request).await {
        Ok(resp) => resp,
        Err(status) => {
            eprintln!("{command}: {}", status.message());
            return 1;
        }
    };

    let mut stream = response.into_inner();
    let mut exit_code = 1;

    while let Ok(Some(msg)) = stream.message().await {
        let Some(payload) = msg.payload else {
            continue;
        };
        match payload {
            Payload::Stdout(data) => {
                let _ = std::io::stdout().write_all(&data);
                let _ = std::io::stdout().flush();
            }
            Payload::Stderr(data) => {
                let _ = std::io::stderr().write_all(&data);
                let _ = std::io::stderr().flush();
            }
            Payload::ExitCode(code) => exit_code = code,
            Payload::AlreadyRunning(_) => {}
        }
    }

    exit_code
}

fn parse_language(s: &str) -> Option<Language> {
    match s.to_lowercase().as_str() {
        "rust" => Some(Language::Rust),
        _ => None,
    }
}

async fn run_rag_search(query: &str, language: &str, top_k: u32) -> i32 {
    let lang = match parse_language(language) {
        Some(l) => l,
        None => {
            eprintln!("rag search: unsupported language: {language}");
            return 1;
        }
    };

    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).expect("UR_SERVER_ADDR must be set");
    let addr = format!("http://{server_addr}");

    let channel = match Endpoint::try_from(addr).unwrap().connect().await {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("rag search: failed to connect to ur server: {e}");
            return 1;
        }
    };

    let mut client = RagServiceClient::new(channel);

    let mut request = tonic::Request::new(RagSearchRequest {
        query: query.to_owned(),
        language: lang.into(),
        top_k: Some(top_k),
    });
    inject_auth(&mut request);

    let resp = match client.rag_search(request).await {
        Ok(resp) => resp,
        Err(status) => {
            eprintln!("rag search: {}", status.message());
            return 1;
        }
    };

    let results = resp.into_inner().results;
    if results.is_empty() {
        println!("No results found.");
        return 0;
    }

    for (i, result) in results.iter().enumerate() {
        println!("--- Result {} (score: {:.2}) ---", i + 1, result.score);
        println!("Source: {}", result.source_file);
        println!();
        println!("{}", result.text);
        println!();
    }

    0
}

const WORKERD_PORT: u16 = 9120;

async fn run_notify_idle() {
    eprintln!("[workertools] notify-idle: sending idle notification to workerd");
    let addr = format!("http://127.0.0.1:{WORKERD_PORT}");
    let channel = match Endpoint::try_from(addr).unwrap().connect().await {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("[workertools] notify-idle: failed to connect to workerd: {e}");
            return;
        }
    };
    let mut client = WorkerDaemonServiceClient::new(channel);
    match client.notify_idle(NotifyIdleRequest {}).await {
        Ok(_) => eprintln!("[workertools] notify-idle: success"),
        Err(e) => eprintln!("[workertools] notify-idle: RPC failed: {e}"),
    }
}
