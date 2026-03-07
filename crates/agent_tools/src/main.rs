use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use tarpc::client;
use tarpc::context;
use tarpc::tokio_serde::formats::Bincode;
use ur_rpc::stream::{connect_stream, recv_output};
use ur_rpc::{CommandOutput, ExecGitRequest, UrAgentBridgeClient};

#[derive(Parser)]
#[command(name = "agent_tools", about = "Worker CLI for Ur containers")]
struct Cli {
    /// Path to the urd Unix domain socket (default: $UR_CONFIG/ur.sock or ~/.ur/ur.sock)
    #[arg(long, env = "UR_SOCKET")]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ping the urd server to verify connectivity
    Ping,
    /// Ask a blocking question to the human operator
    Ask { question: String },
    /// Proxy git commands to the host
    Git {
        /// Git arguments
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Interact with the ticket system
    Ticket {
        #[command(subcommand)]
        command: TicketCommands,
    },
}

#[derive(Subcommand)]
enum TicketCommands {
    /// Read the current ticket spec
    Read,
    /// Append a note to the current ticket
    Note { message: String },
    /// Spawn a child ticket
    Spawn {
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Update ticket status
    Status { status: String },
}

/// Connect to a streaming command socket and write output chunks
/// to stdout/stderr as they arrive. Returns the exit code.
async fn consume_command_stream(socket_path: &Path) -> anyhow::Result<i32> {
    let mut stream = connect_stream(socket_path).await?;
    let mut exit_code = -1;

    while let Some(result) = recv_output(&mut stream).await {
        match result? {
            CommandOutput::Stdout(data) => {
                std::io::stdout().write_all(&data)?;
                std::io::stdout().flush()?;
            }
            CommandOutput::Stderr(data) => {
                std::io::stderr().write_all(&data)?;
                std::io::stderr().flush()?;
            }
            CommandOutput::Exit(code) => {
                exit_code = code;
            }
        }
    }

    Ok(exit_code)
}

async fn connect(socket: &Path) -> anyhow::Result<UrAgentBridgeClient> {
    let transport = tarpc::serde_transport::unix::connect(socket, Bincode::default).await?;
    let client = UrAgentBridgeClient::new(client::Config::default(), transport).spawn();
    Ok(client)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(ur_rpc::default_socket_path);
    match cli.command {
        Commands::Ping => {
            let client = connect(&socket).await?;
            let resp = client.ping(context::current()).await?;
            println!("{resp}");
        }
        Commands::Ask { question } => {
            println!("Asking: {question}");
        }
        Commands::Git { args } => {
            let client = connect(&socket).await?;
            let resp = client
                .exec_git_stream(context::current(), ExecGitRequest { args: args.clone() })
                .await?
                .map_err(|e| anyhow::anyhow!(e))?;

            // Stream socket filename is relative to the control socket's directory.
            let stream_socket = socket
                .parent()
                .expect("socket path must have a parent")
                .join(&resp.stream_socket);

            // Try streaming first; fall back to non-streaming exec_git if the
            // stream socket isn't reachable (e.g. Apple VM-based runtime).
            match consume_command_stream(&stream_socket).await {
                Ok(exit_code) => std::process::exit(exit_code),
                Err(_) => {
                    let resp = client
                        .exec_git(context::current(), ExecGitRequest { args })
                        .await?
                        .map_err(|e| anyhow::anyhow!(e))?;
                    print!("{}", resp.stdout);
                    eprint!("{}", resp.stderr);
                    std::process::exit(resp.exit_code);
                }
            }
        }
        Commands::Ticket { command } => match command {
            TicketCommands::Read => println!("Reading ticket..."),
            TicketCommands::Note { message } => {
                println!("Adding note: {message}");
            }
            TicketCommands::Spawn { title, description } => {
                println!("Spawning: {title} ({description:?})");
            }
            TicketCommands::Status { status } => {
                println!("Setting status: {status}");
            }
        },
    }
    Ok(())
}
