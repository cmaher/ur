mod compose;

use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use container::{ContainerId, ContainerRuntime};
use tonic::transport::{Channel, Endpoint};
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::*;

use compose::{ComposeManager, compose_manager_from_config};

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
    /// Kill server, containers, or everything
    Kill {
        #[command(subcommand)]
        command: KillCommands,
    },
}

#[derive(Subcommand)]
enum KillCommands {
    /// Stop the urd daemon (docker compose down)
    Server,
    /// Kill a specific container, or all ur-agent containers with --all
    Container {
        /// Container name (without the ur-agent- prefix)
        name: Option<String>,
        /// Kill all ur-agent containers
        #[arg(long)]
        all: bool,
    },
    /// Kill all containers and the server
    All,
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

fn load_config() -> ur_config::Config {
    ur_config::Config::load().unwrap_or_else(|_| {
        let home = dirs::home_dir().expect("cannot determine home directory");
        let config_dir = home.join(".ur");
        ur_config::Config {
            config_dir: config_dir.clone(),
            workspace: config_dir.join("workspace"),
            daemon_port: ur_config::DEFAULT_DAEMON_PORT,
            compose_file: config_dir.join("docker-compose.yml"),
            proxy: ur_config::ProxyConfig {
                port: ur_config::DEFAULT_PROXY_PORT,
                allowlist: vec!["api.anthropic.com".to_string()],
            },
            network: ur_config::NetworkConfig {
                name: ur_config::DEFAULT_NETWORK_NAME.to_string(),
                urd_hostname: ur_config::DEFAULT_URD_HOSTNAME.to_string(),
            },
        }
    })
}

fn resolve_daemon_port(cli_port: Option<u16>, config: &ur_config::Config) -> u16 {
    cli_port.unwrap_or(config.daemon_port)
}

async fn try_connect(addr: &str) -> Option<CoreServiceClient<Channel>> {
    let channel = Endpoint::try_from(addr.to_string())
        .ok()?
        .connect()
        .await
        .ok()?;
    Some(CoreServiceClient::new(channel))
}

/// Start urd via Docker Compose and return the compose manager used.
fn start_urd_compose(compose: &ComposeManager) -> Result<()> {
    compose.up().context("failed to start urd via docker compose")
}

async fn connect(port: u16, compose: &ComposeManager) -> Result<CoreServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");

    // Fast path: urd is already running and accepting connections
    if let Some(client) = try_connect(&addr).await {
        return Ok(client);
    }

    // Start urd via docker compose (includes --wait for health/readiness)
    eprintln!("Starting urd via docker compose...");
    start_urd_compose(compose)?;

    // Poll for gRPC readiness after compose reports the service is up.
    // The --wait flag handles container health checks, but the gRPC server
    // inside the container may need a moment after the container is "healthy".
    for i in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(if i < 5 {
            100
        } else {
            250
        }))
        .await;
        if let Some(client) = try_connect(&addr).await {
            return Ok(client);
        }
    }

    bail!("urd did not become reachable within timeout — check docker compose logs")
}

fn kill_daemon(compose: &ComposeManager) -> Result<()> {
    if !compose.is_running()? {
        println!("urd is not running");
        return Ok(());
    }

    compose.down()?;
    println!("Stopped urd (docker compose down)");
    Ok(())
}

fn kill_container(name: &str) -> Result<()> {
    let rt = container::runtime_from_env();
    let id = ContainerId(format!("{}{name}", container::AGENT_CONTAINER_PREFIX));
    rt.stop(&id)
        .with_context(|| format!("failed to stop container {}", id.0))?;
    rt.rm(&id)
        .with_context(|| format!("failed to remove container {}", id.0))?;
    println!("Killed {}", id.0);
    Ok(())
}

fn kill_all_containers() -> Result<()> {
    let rt = container::runtime_from_env();
    let containers = rt.list_by_prefix(container::AGENT_CONTAINER_PREFIX)?;
    if containers.is_empty() {
        println!("No ur-agent containers running");
        return Ok(());
    }
    for id in &containers {
        if let Err(e) = rt.stop(id) {
            eprintln!("Warning: failed to stop {}: {e}", id.0);
        }
        if let Err(e) = rt.rm(id) {
            eprintln!("Warning: failed to remove {}: {e}", id.0);
        }
        println!("Killed {}", id.0);
    }
    Ok(())
}

fn handle_kill(command: KillCommands, compose: &ComposeManager) -> Result<()> {
    match command {
        KillCommands::Server => kill_daemon(compose),
        KillCommands::Container { name, all } => {
            if all {
                kill_all_containers()
            } else if let Some(name) = name {
                kill_container(&name)
            } else {
                bail!("specify a container name or --all")
            }
        }
        KillCommands::All => {
            kill_all_containers()?;
            kill_daemon(compose)
        }
    }
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

    let image_id = "ur-worker:latest";
    let container_name = format!("ur-agent-{ticket_id}");
    println!("Launching agent {container_name}...");
    let resp = client
        .process_launch(ProcessLaunchRequest {
            process_id: ticket_id.into(),
            image_id: image_id.into(),
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
    let config = load_config();
    let port = resolve_daemon_port(cli.port, &config);
    let compose = compose_manager_from_config(&config);

    match cli.command {
        Commands::Kill { command } => return handle_kill(command, &compose),
        Commands::Tui => println!("Launching TUI..."),
        Commands::Process { command } => match command {
            ProcessCommands::Attach { process_id } => {
                process_attach(&process_id)?;
            }
            other => {
                let mut client = connect(port, &compose).await?;
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
