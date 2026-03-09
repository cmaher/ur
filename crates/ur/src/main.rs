mod compose;
mod init;
mod proxy;

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
    /// TCP port of the server gRPC server (overrides ur.toml)
    #[arg(long)]
    port: Option<u16>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Bootstrap the ~/.ur/ config directory
    Init {
        /// Overwrite all files
        #[arg(long)]
        force: bool,
        /// Overwrite ur.toml only
        #[arg(long)]
        force_config: bool,
        /// Overwrite docker-compose.yml only
        #[arg(long)]
        force_compose: bool,
        /// Overwrite squid/allowlist.txt only
        #[arg(long)]
        force_squid: bool,
    },
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
    /// Start the server
    Start,
    /// Stop the server
    Stop,
    /// Kill server, containers, or everything
    Kill {
        #[command(subcommand)]
        command: KillCommands,
    },
    /// Manage the forward proxy domain allowlist
    Proxy {
        #[command(subcommand)]
        command: ProxyCommands,
    },
}

#[derive(Subcommand)]
enum ProxyCommands {
    /// Allow a domain through the proxy
    Allow { domain: String },
    /// Block a domain (remove from allowlist)
    Block { domain: String },
    /// List allowed domains
    List,
}

#[derive(Subcommand)]
enum KillCommands {
    /// Stop the server (docker compose down)
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

fn load_config() -> Result<ur_config::Config> {
    ur_config::Config::load().context("failed to load config")
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

fn start_server(compose: &ComposeManager) -> Result<()> {
    compose
        .up()
        .context("failed to start server via docker compose")?;
    println!("server started");
    Ok(())
}

fn stop_server(compose: &ComposeManager) -> Result<()> {
    if !compose.is_running()? {
        println!("server is not running");
        return Ok(());
    }
    compose.down()?;
    println!("server stopped");
    Ok(())
}

async fn connect(port: u16) -> Result<CoreServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");

    match try_connect(&addr).await {
        Some(client) => Ok(client),
        None => bail!("server is not running — run 'ur start' first"),
    }
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
        KillCommands::Server => stop_server(compose),
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
            stop_server(compose)
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

    // Init bypasses config loading — it creates the config files.
    if let Commands::Init {
        force,
        force_config,
        force_compose,
        force_squid,
    } = cli.command
    {
        return init::run(init::InitFlags {
            force,
            force_config,
            force_compose,
            force_squid,
        });
    }

    let config = load_config()?;
    let port = resolve_daemon_port(cli.port, &config);
    let compose = compose_manager_from_config(&config);

    match cli.command {
        Commands::Init { .. } => unreachable!(),
        Commands::Start => start_server(&compose)?,
        Commands::Stop => stop_server(&compose)?,
        Commands::Kill { command } => return handle_kill(command, &compose),
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
        Commands::Proxy { command } => {
            let squid_dir = config.squid_dir();
            let allowlist_path = squid_dir.join("allowlist.txt");
            match command {
                ProxyCommands::Allow { domain } => {
                    let domains = proxy::allow_domain(&allowlist_path, &domain)?;
                    proxy::signal_reconfigure();
                    proxy::print_domains(&domains);
                }
                ProxyCommands::Block { domain } => {
                    let domains = proxy::block_domain(&allowlist_path, &domain)?;
                    proxy::signal_reconfigure();
                    proxy::print_domains(&domains);
                }
                ProxyCommands::List => {
                    let domains = proxy::read_allowlist(&allowlist_path)?;
                    proxy::print_domains(&domains);
                }
            }
        }
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
