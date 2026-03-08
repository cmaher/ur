use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use container::ContainerId;
use tonic::transport::{Channel, Endpoint};
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::*;

#[derive(Parser)]
#[command(name = "ur", about = "Coding LLM coordination framework")]
struct Cli {
    /// TCP port of the urd gRPC server (overrides ur.toml)
    #[arg(long)]
    port: Option<u16>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the TUI dashboard
    Tui,
    /// Manage processes
    Process {
        #[command(subcommand)]
        command: ProcessCommands,
    },
    /// Manage tickets
    Ticket {
        #[command(subcommand)]
        command: TicketCommands,
    },
}

#[derive(Subcommand)]
enum ProcessCommands {
    /// Launch a new agent process
    Launch {
        ticket_id: String,
        /// Mount a host directory as the container workspace
        #[arg(short = 'w', long = "workspace")]
        workspace: Option<PathBuf>,
    },
    /// Show process status
    Status { process_id: Option<String> },
    /// Attach to a running process
    Attach { process_id: String },
    /// Stop a running agent process
    Stop { process_id: String },
}

#[derive(Subcommand)]
enum TicketCommands {
    /// Create a new ticket
    Create {
        title: String,
        #[arg(long)]
        parent: Option<String>,
    },
    /// List tickets
    Ls,
    /// Show ticket details
    Show { ticket_id: String },
}

fn resolve_daemon_port(cli_port: Option<u16>) -> u16 {
    if let Some(port) = cli_port {
        return port;
    }
    match ur_config::Config::load() {
        Ok(cfg) => cfg.daemon_port,
        Err(_) => ur_config::DEFAULT_DAEMON_PORT,
    }
}

async fn connect(port: u16) -> Result<CoreServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");
    let channel = Endpoint::try_from(addr.clone())?
        .connect()
        .await
        .with_context(|| format!("failed to connect to urd at {addr}"))?;
    Ok(CoreServiceClient::new(channel))
}

fn process_attach(process_id: &str) -> Result<()> {
    let runtime = container::runtime_from_env();
    let id = ContainerId(format!("ur-agent-{process_id}"));
    let command: Vec<String> = vec!["tmux".into(), "attach".into(), "-t".into(), "agent".into()];
    let status = runtime.exec_interactive(&id, &command)?;
    process::exit(status.code().unwrap_or(1));
}

async fn process_launch(
    client: &mut CoreServiceClient<Channel>,
    ticket_id: &str,
    workspace: Option<PathBuf>,
) -> Result<()> {
    // Resolve workspace to an absolute path if provided
    let workspace_dir = match workspace {
        Some(path) => {
            let abs = std::fs::canonicalize(&path)
                .with_context(|| format!("failed to resolve workspace path: {}", path.display()))?;
            abs.to_string_lossy().into_owned()
        }
        None => String::new(),
    };

    // Build the container image directly using the container crate
    // (container_build is not part of CoreService proto).
    let project_root = std::env::current_dir()?;
    let context_dir = project_root.join("containers/claude-worker");
    println!("Building worker image...");
    let rt = container::runtime_from_env();
    let image = rt.build(&container::BuildOpts {
        tag: "ur-worker:latest".into(),
        dockerfile: context_dir.join("Dockerfile"),
        context: context_dir.clone(),
    })?;

    let container_name = format!("ur-agent-{ticket_id}");
    println!("Launching agent {container_name}...");
    let resp = client
        .process_launch(ProcessLaunchRequest {
            process_id: ticket_id.into(),
            image_id: image.0,
            cpus: 2,
            memory: "8G".into(),
            workspace_dir,
        })
        .await?;

    println!(
        "Agent {container_name} running (container {})",
        resp.into_inner().container_id
    );
    Ok(())
}

async fn process_stop(client: &mut CoreServiceClient<Channel>, process_id: &str) -> Result<()> {
    println!("Stopping {process_id}...");
    client
        .process_stop(ProcessStopRequest {
            process_id: process_id.into(),
        })
        .await?;
    println!("Agent {process_id} stopped.");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let port = resolve_daemon_port(cli.port);
    match cli.command {
        Commands::Tui => println!("Launching TUI..."),
        Commands::Process { command } => match command {
            ProcessCommands::Attach { process_id } => {
                process_attach(&process_id)?;
            }
            other => {
                let mut client = connect(port).await?;
                match other {
                    ProcessCommands::Launch {
                        ticket_id,
                        workspace,
                    } => {
                        process_launch(&mut client, &ticket_id, workspace).await?;
                    }
                    ProcessCommands::Status { process_id } => {
                        println!("Status: {process_id:?}");
                    }
                    ProcessCommands::Stop { process_id } => {
                        process_stop(&mut client, &process_id).await?;
                    }
                    ProcessCommands::Attach { .. } => unreachable!(),
                }
            }
        },
        Commands::Ticket { command } => match command {
            TicketCommands::Create { title, parent } => {
                println!("Creating ticket: {title} (parent: {parent:?})");
            }
            TicketCommands::Ls => println!("Listing tickets..."),
            TicketCommands::Show { ticket_id } => {
                println!("Showing ticket {ticket_id}...");
            }
        },
    }
    Ok(())
}
