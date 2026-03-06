use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use container::ContainerId;
use tarpc::tokio_serde::formats::Bincode;

use ur_rpc::*;

#[derive(Parser)]
#[command(name = "ur", about = "Coding LLM coordination framework")]
struct Cli {
    /// Path to the urd control socket
    #[arg(long, default_value = "/tmp/ur/sockets/ur.sock")]
    socket: PathBuf,

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
    Launch { ticket_id: String },
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

async fn connect(socket: &PathBuf) -> Result<UrAgentBridgeClient> {
    let transport = tarpc::serde_transport::unix::connect(&socket, Bincode::default)
        .await
        .with_context(|| format!("failed to connect to urd at {}", socket.display()))?;
    let client = UrAgentBridgeClient::new(tarpc::client::Config::default(), transport).spawn();
    Ok(client)
}

fn process_attach(process_id: &str) -> Result<()> {
    let runtime = container::runtime_from_env();
    let id = ContainerId(process_id.to_string());
    let command: Vec<String> = vec!["tmux".into(), "attach".into(), "-t".into(), "agent".into()];
    let status = runtime.exec_interactive(&id, &command)?;
    process::exit(status.code().unwrap_or(1));
}

async fn process_launch(client: &UrAgentBridgeClient, ticket_id: &str) -> Result<()> {
    let ctx = tarpc::context::current();

    // Build the worker image
    let project_root = std::env::current_dir()?;
    let context_dir = project_root.join("containers/claude-worker");
    println!("Building worker image...");
    let build_resp = client
        .container_build(
            ctx,
            ContainerBuildRequest {
                tag: "ur-worker:latest".into(),
                dockerfile: context_dir.join("Dockerfile").display().to_string(),
                context: context_dir.display().to_string(),
            },
        )
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;

    // Run the container
    let name = format!("ur-agent-{ticket_id}");
    let socket_dir = PathBuf::from("/tmp/ur/sockets");
    let host_socket = socket_dir.join("ur.sock");

    println!("Starting container {name}...");
    let run_resp = client
        .container_run(
            tarpc::context::current(),
            ContainerRunRequest {
                image_id: build_resp.image_id,
                name: name.clone(),
                cpus: 4,
                memory: "8G".into(),
                volumes: vec![],
                socket_mounts: vec![(host_socket.display().to_string(), "/var/run/ur.sock".into())],
                workdir: Some("/workspace".into()),
                command: vec![],
            },
        )
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;

    println!("Agent {name} running (container {})", run_resp.container_id);
    Ok(())
}

async fn process_stop(client: &UrAgentBridgeClient, process_id: &str) -> Result<()> {
    println!("Stopping {process_id}...");
    client
        .container_stop(
            tarpc::context::current(),
            ContainerIdRequest {
                container_id: process_id.into(),
            },
        )
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;

    println!("Removing {process_id}...");
    client
        .container_rm(
            tarpc::context::current(),
            ContainerIdRequest {
                container_id: process_id.into(),
            },
        )
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;

    println!("Agent {process_id} stopped and removed.");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Tui => println!("Launching TUI..."),
        Commands::Process { command } => match command {
            ProcessCommands::Attach { process_id } => {
                process_attach(&process_id)?;
            }
            other => {
                let client = connect(&cli.socket).await?;
                match other {
                    ProcessCommands::Launch { ticket_id } => {
                        process_launch(&client, &ticket_id).await?;
                    }
                    ProcessCommands::Status { process_id } => {
                        println!("Status: {process_id:?}");
                    }
                    ProcessCommands::Stop { process_id } => {
                        process_stop(&client, &process_id).await?;
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
