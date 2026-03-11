mod compose;
mod credential;
mod hostd;
mod init;
mod lifecycle_log;
mod logging;
mod proxy;

use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use container::{ContainerId, ContainerRuntime};
use tonic::transport::{Channel, Endpoint};
use tracing::{debug, error, info, instrument, warn};
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
        /// Overwrite squid/ files (allowlist.txt)
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
    /// Kill all containers and stop the server
    Stop,
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
enum ProcessCommands {
    /// Launch a new agent process
    Launch {
        ticket_id: String,
        /// Mount a host directory as the container workspace
        #[arg(short = 'w', long = "workspace")]
        workspace: Option<PathBuf>,
        /// Attach to the process after launching
        #[arg(short = 'a', long = "attach")]
        attach: bool,
        /// Kill existing container with this ID before launching
        #[arg(short = 'f', long = "force")]
        force: bool,
    },
    /// Show process status
    Status { process_id: Option<String> },
    /// Attach to a running process
    Attach { process_id: String },
    /// Stop a running agent process
    Stop { process_id: String },
    /// Force-remove a container (docker rm -f)
    Kill { process_id: String },
    /// Save credentials from a running container for reuse
    SaveCredentials { process_id: String },
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
    let port = cli_port.unwrap_or(config.daemon_port);
    debug!(cli_port = ?cli_port, config_port = config.daemon_port, resolved_port = port, "resolved daemon port");
    port
}

#[instrument(skip_all, fields(addr))]
async fn try_connect(addr: &str) -> Option<CoreServiceClient<Channel>> {
    debug!(addr, "attempting gRPC connection");
    let channel = Endpoint::try_from(addr.to_string())
        .ok()?
        .connect()
        .await
        .ok()?;
    info!(addr, "gRPC connection established");
    Some(CoreServiceClient::new(channel))
}

#[instrument(skip(config, compose))]
fn start_server(config: &ur_config::Config, compose: &ComposeManager) -> Result<()> {
    let log = lifecycle_log::LifecycleLog::open(&config.config_dir);
    log.info("ur start: beginning");
    info!("starting server");

    match hostd::start_hostd(config) {
        Ok(()) => log.info("ur start: hostd started"),
        Err(e) => {
            log.error(&format!("ur start: hostd failed: {e}"));
            return Err(e);
        }
    }

    match compose.up() {
        Ok(()) => log.info("ur start: compose up succeeded"),
        Err(e) => {
            log.error(&format!("ur start: compose up failed: {e}"));
            return Err(e);
        }
    }

    info!("server started successfully");
    println!("server started");
    log.info("ur start: complete");

    // Check if shared credentials exist; if not, hint about Keychain seeding.
    let has_credentials = credential::CredentialManager::host_credentials_path()
        .ok()
        .and_then(|p| std::fs::metadata(&p).ok())
        .is_some_and(|m| m.len() > 0);
    if !has_credentials {
        warn!("no shared credentials found");
        println!();
        println!("No shared credentials found. Log in to Claude Code on this machine first.");
        println!("Credentials will be seeded from the macOS Keychain on first process launch.");
    }

    Ok(())
}

#[instrument(skip(config, compose))]
fn stop_server(config: &ur_config::Config, compose: &ComposeManager) -> Result<()> {
    let log = lifecycle_log::LifecycleLog::open(&config.config_dir);
    log.info("ur stop: beginning");
    info!("stopping server");
    kill_all_containers(&config.network.agent_prefix)?;
    if !compose.is_running()? {
        info!("server is not running, nothing to stop");
        println!("server is not running");
        log.info("ur stop: server was not running");
        return Ok(());
    }
    compose.down()?;
    info!("server stopped successfully");
    println!("server stopped");
    log.info("ur stop: compose down succeeded");

    hostd::stop_hostd(config)?;
    log.info("ur stop: hostd stopped");
    log.info("ur stop: complete");
    Ok(())
}

#[instrument]
async fn connect(port: u16) -> Result<CoreServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");

    match try_connect(&addr).await {
        Some(client) => Ok(client),
        None => {
            error!(port, "server is not running");
            bail!("server is not running — run 'ur start' first")
        }
    }
}

#[instrument]
fn kill_container(name: &str, agent_prefix: &str) -> Result<()> {
    let rt = container::runtime_from_env();
    let id = ContainerId(format!("{agent_prefix}{name}"));
    info!(container = %id.0, "killing container");
    rt.stop(&id)
        .with_context(|| format!("failed to stop container {}", id.0))?;
    rt.rm(&id)
        .with_context(|| format!("failed to remove container {}", id.0))?;
    info!(container = %id.0, "container killed");
    println!("Killed {}", id.0);
    Ok(())
}

#[instrument]
fn kill_all_containers(agent_prefix: &str) -> Result<()> {
    let rt = container::runtime_from_env();
    let containers = rt.list_by_prefix(agent_prefix)?;
    if containers.is_empty() {
        debug!(agent_prefix, "no agent containers running");
        println!("No agent containers running (prefix: {agent_prefix})");
        return Ok(());
    }
    info!(
        count = containers.len(),
        agent_prefix, "killing all agent containers"
    );
    for id in &containers {
        if let Err(e) = rt.stop(id) {
            warn!(container = %id.0, error = %e, "failed to stop container");
            eprintln!("Warning: failed to stop {}: {e}", id.0);
        }
        if let Err(e) = rt.rm(id) {
            warn!(container = %id.0, error = %e, "failed to remove container");
            eprintln!("Warning: failed to remove {}: {e}", id.0);
        }
        info!(container = %id.0, "container killed");
        println!("Killed {}", id.0);
    }
    Ok(())
}

#[instrument]
fn process_attach(process_id: &str, agent_prefix: &str) -> Result<()> {
    let runtime = container::runtime_from_env();
    let id = ContainerId(format!("{agent_prefix}{process_id}"));
    info!(container = %id.0, "attaching to process");
    // Create an independent tmux session instead of attaching to "agent".
    // `tmux attach -t agent` kills the session if the user exits the shell,
    // preventing reconnection. A separate session survives agent-session death
    // and vice versa. `-A` reattaches if the session already exists.
    let command: Vec<String> = vec![
        "tmux".into(),
        "-u".into(),
        "new-session".into(),
        "-A".into(),
        "-s".into(),
        "attach".into(),
    ];
    let status = runtime.exec_interactive(&id, &command)?;
    process::exit(status.code().unwrap_or(1));
}

#[instrument(skip(client), fields(ticket_id, workspace = ?workspace))]
async fn process_launch(
    client: &mut CoreServiceClient<Channel>,
    ticket_id: &str,
    workspace: Option<PathBuf>,
    agent_prefix: &str,
) -> Result<()> {
    info!(ticket_id, "launching agent process");

    // Refresh credentials from macOS Keychain and ensure config exists
    let cred_mgr = credential::CredentialManager;
    cred_mgr.ensure_credentials()?;
    debug!(ticket_id, "credentials ensured");

    // Resolve workspace to an absolute path if provided
    let workspace_dir = match workspace {
        Some(path) => {
            let abs = std::fs::canonicalize(&path)
                .with_context(|| format!("failed to resolve workspace path: {}", path.display()))?;
            debug!(workspace = %abs.display(), "resolved workspace path");
            abs.to_string_lossy().into_owned()
        }
        None => String::new(),
    };

    let image_id = "ur-worker-rust:latest";
    let container_name = format!("{agent_prefix}{ticket_id}");
    println!("Launching agent {container_name}...");
    let resp = client
        .process_launch(ProcessLaunchRequest {
            process_id: ticket_id.into(),
            image_id: image_id.into(),
            cpus: 2,
            memory: "8G".into(),
            workspace_dir,
            claude_credentials: String::new(),
            template: String::new(),
            skills: Vec::new(),
        })
        .await?;

    let container_id = resp.into_inner().container_id;
    info!(
        ticket_id,
        container_name, container_id, image_id, "agent process launched"
    );
    println!("Agent {container_name} running (container {container_id})");
    Ok(())
}

#[instrument(skip(client))]
async fn process_stop(client: &mut CoreServiceClient<Channel>, process_id: &str) -> Result<()> {
    info!(process_id, "stopping agent process");
    println!("Stopping {process_id}...");
    client
        .process_stop(ProcessStopRequest {
            process_id: process_id.into(),
        })
        .await?;
    info!(process_id, "agent process stopped");
    println!("Agent {process_id} stopped.");
    Ok(())
}

#[instrument(skip(command), fields(command_name = command_name(&command)))]
async fn handle_process(command: ProcessCommands, port: u16, agent_prefix: &str) -> Result<()> {
    match command {
        ProcessCommands::Attach { process_id } => process_attach(&process_id, agent_prefix),
        ProcessCommands::Kill { process_id } => kill_container(&process_id, agent_prefix),
        ProcessCommands::SaveCredentials { process_id } => {
            info!(process_id = %process_id, "saving credentials from container");
            let runtime = container::runtime_from_env();
            let id = container::ContainerId(format!("{agent_prefix}{process_id}"));
            let cred_mgr = credential::CredentialManager;
            let paths = cred_mgr.save_from_container(&runtime, &id)?;
            for path in &paths {
                info!(path = %path.display(), "saved credential file");
                println!("Saved {}", path.display());
            }
            Ok(())
        }
        ProcessCommands::Launch {
            ticket_id,
            workspace,
            attach,
            force,
        } => {
            if force {
                debug!(ticket_id = %ticket_id, "force-killing existing container before launch");
                let _ = kill_container(&ticket_id, agent_prefix);
            }
            let mut client = connect(port).await?;
            process_launch(&mut client, &ticket_id, workspace, agent_prefix).await?;
            if attach {
                process_attach(&ticket_id, agent_prefix)?;
            }
            Ok(())
        }
        ProcessCommands::Status { process_id } => {
            debug!(process_id = ?process_id, "querying process status");
            println!("Status: {process_id:?}");
            Ok(())
        }
        ProcessCommands::Stop { process_id } => {
            let mut client = connect(port).await?;
            process_stop(&mut client, &process_id).await
        }
    }
}

/// Extract the subcommand name for span fields.
fn command_name(cmd: &ProcessCommands) -> &'static str {
    match cmd {
        ProcessCommands::Attach { .. } => "attach",
        ProcessCommands::Kill { .. } => "kill",
        ProcessCommands::SaveCredentials { .. } => "save_credentials",
        ProcessCommands::Launch { .. } => "launch",
        ProcessCommands::Status { .. } => "status",
        ProcessCommands::Stop { .. } => "stop",
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Init bypasses config loading — it creates the config files.
    if let Commands::Init {
        force,
        force_config,
        force_squid,
    } = cli.command
    {
        return init::run(init::InitFlags {
            force,
            force_config,
            force_squid,
        });
    }

    let config = load_config()?;

    // Initialize structured JSON file logging after config is loaded so we
    // know where to write the log file. The guard must live until main exits.
    let _log_guard = logging::init(&config.config_dir);

    info!(
        config_dir = %config.config_dir.display(),
        daemon_port = config.daemon_port,
        hostd_port = config.hostd_port,
        "ur CLI started"
    );

    let port = resolve_daemon_port(cli.port, &config);
    let compose = compose_manager_from_config(&config);

    match cli.command {
        Commands::Init { .. } => unreachable!(),
        Commands::Start => start_server(&config, &compose)?,
        Commands::Stop => stop_server(&config, &compose)?,
        Commands::Tui => {
            info!("launching TUI");
            println!("Launching TUI...");
        }
        Commands::Process { command } => {
            handle_process(command, port, &config.network.agent_prefix).await?
        }
        Commands::Proxy { command } => {
            let squid_dir = config.squid_dir();
            let allowlist_path = squid_dir.join("allowlist.txt");
            match command {
                ProxyCommands::Allow { domain } => {
                    info!(domain = %domain, "allowing domain through proxy");
                    let domains = proxy::allow_domain(&allowlist_path, &domain)?;
                    proxy::signal_reconfigure(&config.proxy.hostname);
                    proxy::print_domains(&domains);
                }
                ProxyCommands::Block { domain } => {
                    info!(domain = %domain, "blocking domain from proxy");
                    let domains = proxy::block_domain(&allowlist_path, &domain)?;
                    proxy::signal_reconfigure(&config.proxy.hostname);
                    proxy::print_domains(&domains);
                }
                ProxyCommands::List => {
                    debug!("listing proxy domains");
                    let domains = proxy::read_allowlist(&allowlist_path)?;
                    proxy::print_domains(&domains);
                }
            }
        }
        Commands::Ticket { command } => match command {
            TicketCommands::Create { title, parent } => {
                info!(title = %title, parent = ?parent, "creating ticket");
                println!("Creating ticket: {title} (parent: {parent:?})");
            }
            TicketCommands::Ls => {
                debug!("listing tickets");
                println!("Listing tickets...");
            }
            TicketCommands::Show { ticket_id } => {
                debug!(ticket_id = %ticket_id, "showing ticket");
                println!("Showing ticket {ticket_id}...");
            }
        },
    }
    Ok(())
}
