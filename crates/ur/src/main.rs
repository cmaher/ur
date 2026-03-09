use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result, bail};
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
    /// Kill server, containers, or everything
    Kill {
        #[command(subcommand)]
        command: KillCommands,
    },
}

#[derive(Subcommand)]
enum KillCommands {
    /// Kill the urd daemon process
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

fn resolve_daemon_port(cli_port: Option<u16>) -> u16 {
    if let Some(port) = cli_port {
        return port;
    }
    match ur_config::Config::load() {
        Ok(cfg) => cfg.daemon_port,
        Err(_) => ur_config::DEFAULT_DAEMON_PORT,
    }
}

async fn try_connect(addr: &str) -> Option<CoreServiceClient<Channel>> {
    let channel = Endpoint::try_from(addr.to_string()).ok()?.connect().await.ok()?;
    Some(CoreServiceClient::new(channel))
}

fn spawn_daemon() -> Result<PathBuf> {
    let ur_bin = std::env::current_exe().context("cannot determine ur binary path")?;
    let urd_bin = ur_bin.with_file_name("urd");
    if !urd_bin.exists() {
        bail!("urd not found at {}", urd_bin.display());
    }

    let log_dir = ur_config::Config::load()
        .map(|c| c.config_dir.join("logs"))
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").expect("HOME not set")).join(".ur/logs")
        });
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("failed to create log dir: {}", log_dir.display()))?;

    let stdout = std::fs::File::options()
        .create(true)
        .append(true)
        .open(log_dir.join("urd.stdout.log"))
        .context("failed to open urd stdout log")?;
    let stderr = std::fs::File::options()
        .create(true)
        .append(true)
        .open(log_dir.join("urd.stderr.log"))
        .context("failed to open urd stderr log")?;

    std::process::Command::new(&urd_bin)
        .stdout(stdout)
        .stderr(stderr)
        .stdin(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("failed to spawn urd at {}", urd_bin.display()))?;

    Ok(urd_bin)
}

async fn connect(port: u16) -> Result<CoreServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");

    if let Some(client) = try_connect(&addr).await {
        return Ok(client);
    }

    let urd_bin = spawn_daemon()?;
    eprintln!("Starting urd ({})", urd_bin.display());

    for i in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(if i < 5 { 100 } else { 250 })).await;
        if let Some(client) = try_connect(&addr).await {
            return Ok(client);
        }
    }

    bail!("urd did not start within timeout — check ~/.ur/logs/")
}

fn kill_daemon() -> Result<()> {
    let cfg = ur_config::Config::load()?;
    let pid_file = cfg.config_dir.join(ur_config::URD_PID_FILE);

    let pid = match std::fs::read_to_string(&pid_file) {
        Ok(s) => s.trim().to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("urd is not running (no pid file)");
            return Ok(());
        }
        Err(e) => return Err(e).context("failed to read pid file"),
    };

    // Check if the process is actually alive
    let status = std::process::Command::new("kill")
        .args(["-0", &pid])
        .status()
        .context("failed to check urd process")?;

    if !status.success() {
        std::fs::remove_file(&pid_file).ok();
        println!("urd is not running (stale pid file removed)");
        return Ok(());
    }

    std::process::Command::new("kill")
        .arg(&pid)
        .status()
        .context("failed to kill urd")?;

    std::fs::remove_file(&pid_file).ok();
    println!("Stopped urd (pid {pid})");
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

fn handle_kill(command: KillCommands) -> Result<()> {
    match command {
        KillCommands::Server => kill_daemon(),
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
            kill_daemon()
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
    let port = resolve_daemon_port(cli.port);
    match cli.command {
        Commands::Kill { command } => return handle_kill(command),
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
