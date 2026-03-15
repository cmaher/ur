mod compose;
mod credential;
mod db;
mod hostd;
mod init;
mod lifecycle_log;
mod logging;
mod project;
mod proxy;
mod rag;
mod ticket;

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
    /// Manage workers
    Worker {
        #[command(subcommand)]
        command: WorkerCommands,
    },
    /// Manage tickets
    Ticket {
        #[command(subcommand)]
        command: ticket_client::TicketArgs,
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
    /// Manage projects
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
    /// RAG documentation and search
    Rag {
        #[command(subcommand)]
        command: RagCommands,
    },
    /// Query agent information
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
    /// Database backup and restore
    Db {
        #[command(subcommand)]
        command: DbCommands,
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
enum ProjectCommands {
    /// List all configured projects with pool usage
    List,
    /// Add a new project from a local git directory
    Add {
        /// Path to a git repository directory (e.g. "." for current directory)
        path: PathBuf,
        /// Project key (derived from repo name if omitted)
        #[arg(long)]
        key: Option<String>,
        /// Display-friendly project name
        #[arg(long)]
        name: Option<String>,
        /// Maximum number of cached repo clones (default: 10)
        #[arg(long)]
        pool_limit: Option<u32>,
    },
    /// Remove a project and delete all pool clones
    Remove {
        /// Project key to remove
        key: String,
        /// Required to confirm deletion of pool clones
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum RagCommands {
    /// Generate Rust documentation for RAG indexing
    Docs,
    /// Index generated docs into the vector store
    Index {
        /// Language to index (default: rust)
        #[arg(long, default_value = "rust")]
        language: String,
    },
    /// Search indexed documentation
    Search {
        /// Search query
        query: String,
        /// Language to search (default: rust)
        #[arg(long, default_value = "rust")]
        language: String,
        /// Number of results to return (default: 5)
        #[arg(long, default_value = "5")]
        top_k: u32,
    },
    /// Manage embedding models
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },
}

#[derive(Subcommand)]
enum ModelCommands {
    /// Download the configured embedding model to the local cache
    Download,
}

#[derive(Subcommand)]
enum DbCommands {
    /// Create an on-demand database backup
    Backup,
    /// Restore a database from a backup file
    Restore {
        /// Path to the backup file to restore
        path: PathBuf,
    },
    /// List available backup files
    List,
}

#[derive(Subcommand)]
enum WorkerCommands {
    /// Launch a new agent process
    Launch {
        ticket_id: String,
        /// Mount a host directory as the container workspace (mutually exclusive with -p)
        #[arg(short = 'w', long = "workspace", conflicts_with = "project")]
        workspace: Option<PathBuf>,
        /// Project key for repo pool launch (mutually exclusive with -w)
        #[arg(short = 'p', long = "project", conflicts_with = "workspace")]
        project: Option<String>,
        /// Attach to the process after launching
        #[arg(short = 'a', long = "attach")]
        attach: bool,
        /// Stop the process when the attach session exits (implies -a)
        #[arg(long)]
        rm: bool,
        /// Stop existing process with this ID before launching
        #[arg(short = 'f', long = "force")]
        force: bool,
        /// Prompt mode name (default: "code")
        #[arg(short = 'm', long = "mode", default_value = "code")]
        mode: String,
        /// Comma-separated skill list; overrides mode when provided
        #[arg(short = 's', long = "skills")]
        skills: Option<String>,
    },
    /// List all running processes
    List,
    /// Show process status
    Status { worker_id: Option<String> },
    /// Attach to a running process
    Attach {
        worker_id: String,
        /// Stop the process when the attach session exits
        #[arg(long)]
        rm: bool,
    },
    /// Stop a running agent process
    Stop { worker_id: String },
    /// Force-stop a running agent process (via server)
    Kill { worker_id: String },
    /// Save credentials from a running container for reuse
    SaveCredentials { worker_id: String },
    /// Print the host directory assigned to a running process
    Dir { worker_id: String },
    /// Open the host directory for a running process in VS Code
    Vscode { worker_id: String },
}

#[derive(Subcommand)]
enum AgentCommands {
    /// Print the host workspace directory for a running agent
    Dir { worker_id: String },
}

#[instrument]
fn load_config() -> Result<ur_config::Config> {
    debug!("loading ur config");
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
fn process_attach(worker_id: &str, agent_prefix: &str) -> Result<i32> {
    let runtime = container::runtime_from_env();
    let id = ContainerId(format!("{agent_prefix}{worker_id}"));
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
    Ok(status.code().unwrap_or(1))
}

#[instrument(skip(client))]
async fn process_list(client: &mut CoreServiceClient<Channel>) -> Result<()> {
    info!("listing processes");
    let resp = client.worker_list(WorkerListRequest {}).await?;
    let workers = resp.into_inner().workers;
    if workers.is_empty() {
        println!("No running workers.");
        return Ok(());
    }
    // Print header
    println!(
        "{:<20} {:<12} {:<16} {:<8}",
        "WORKER", "PROJECT", "CONTAINER", "MODE"
    );
    for w in &workers {
        let container_short = if w.container_id.len() > 12 {
            &w.container_id[..12]
        } else {
            &w.container_id
        };
        let project = if w.project_key.is_empty() {
            "-"
        } else {
            &w.project_key
        };
        println!(
            "{:<20} {:<12} {:<16} {:<8}",
            w.worker_id, project, container_short, w.mode
        );
    }
    Ok(())
}

#[instrument(skip(client), fields(ticket_id, workspace = ?workspace, project_key = ?project_key, mode, skills = ?skills))]
async fn process_launch(
    client: &mut CoreServiceClient<Channel>,
    ticket_id: &str,
    workspace: Option<PathBuf>,
    project_key: &str,
    agent_prefix: &str,
    mode: &str,
    skills: &[String],
) -> Result<()> {
    info!(ticket_id, project_key, "launching agent process");

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
        .worker_launch(WorkerLaunchRequest {
            worker_id: ticket_id.into(),
            image_id: image_id.into(),
            cpus: 2,
            memory: "8G".into(),
            workspace_dir,
            claude_credentials: String::new(),
            mode: mode.to_owned(),
            skills: skills.to_vec(),
            project_key: project_key.to_owned(),
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
async fn process_stop(client: &mut CoreServiceClient<Channel>, worker_id: &str) -> Result<()> {
    info!(worker_id, "stopping agent process");
    println!("Stopping {worker_id}...");
    client
        .worker_stop(WorkerStopRequest {
            worker_id: worker_id.into(),
        })
        .await?;
    info!(worker_id, "agent process stopped");
    println!("Agent {worker_id} stopped.");
    Ok(())
}

#[instrument(skip(command, project_keys), fields(command_name = command_name(&command)))]
async fn handle_worker(
    command: WorkerCommands,
    port: u16,
    agent_prefix: &str,
    project_keys: &[String],
) -> Result<()> {
    match command {
        WorkerCommands::List => {
            let mut client = connect(port).await?;
            process_list(&mut client).await
        }
        WorkerCommands::Attach { worker_id, rm } => {
            let exit_code = process_attach(&worker_id, agent_prefix)?;
            if rm {
                println!("Stopping {worker_id} (--rm)...");
                let mut client = connect(port).await?;
                process_stop(&mut client, &worker_id).await?;
            }
            process::exit(exit_code);
        }
        WorkerCommands::Kill { worker_id } => {
            let mut client = connect(port).await?;
            process_stop(&mut client, &worker_id).await
        }
        WorkerCommands::SaveCredentials { worker_id } => {
            info!(worker_id = %worker_id, "saving credentials from container");
            let runtime = container::runtime_from_env();
            let id = container::ContainerId(format!("{agent_prefix}{worker_id}"));
            let cred_mgr = credential::CredentialManager;
            let paths = cred_mgr.save_from_container(&runtime, &id)?;
            for path in &paths {
                info!(path = %path.display(), "saved credential file");
                println!("Saved {}", path.display());
            }
            Ok(())
        }
        WorkerCommands::Launch {
            ticket_id,
            workspace,
            project,
            attach,
            rm,
            force,
            mode,
            skills,
        } => {
            // Parse comma-separated skills; when provided they override the mode server-side
            let skills_vec: Vec<String> = skills
                .iter()
                .flat_map(|s| s.split(',').map(|s| s.trim().to_owned()))
                .filter(|s| !s.is_empty())
                .collect();

            // Resolve project key: explicit -p flag, derive from ticket ID prefix,
            // derive from cwd name, or empty when -w is specified.
            let resolved_project = if let Some(p) = project {
                p
            } else if workspace.is_none() {
                // Try to derive from ticket ID prefix (before first '-' or '.')
                let id_prefix = ticket_id
                    .split(&['-', '.'][..])
                    .next()
                    .unwrap_or("")
                    .to_owned();
                if !id_prefix.is_empty() && project_keys.contains(&id_prefix) {
                    debug!(project_key = %id_prefix, "derived project from ticket ID prefix");
                    id_prefix
                } else {
                    // Fall back to current working directory name
                    let cwd = std::env::current_dir()
                        .context("failed to get current working directory")?;
                    let dir_name = cwd
                        .file_name()
                        .and_then(|n| n.to_str())
                        .ok_or_else(|| anyhow::anyhow!("cannot determine directory name from cwd"))?
                        .to_owned();
                    if project_keys.contains(&dir_name) {
                        debug!(project_key = %dir_name, "derived project from cwd");
                        dir_name
                    } else {
                        bail!(
                            "could not derive project from ticket ID prefix '{}' or \
                             cwd directory name '{}' \
                             (neither is a configured project key). Use -p <project> or -w <path>.",
                            id_prefix,
                            dir_name
                        );
                    }
                }
            } else {
                // -w specified: no project association
                String::new()
            };

            let mut client = connect(port).await?;
            if force {
                debug!(ticket_id = %ticket_id, "force-stopping existing process before launch");
                let _ = process_stop(&mut client, &ticket_id).await;
            }
            process_launch(
                &mut client,
                &ticket_id,
                workspace,
                &resolved_project,
                agent_prefix,
                &mode,
                &skills_vec,
            )
            .await?;
            if attach || rm {
                let exit_code = process_attach(&ticket_id, agent_prefix)?;
                if rm {
                    println!("Stopping {ticket_id} (--rm)...");
                    let mut client = connect(port).await?;
                    process_stop(&mut client, &ticket_id).await?;
                }
                process::exit(exit_code);
            }
            Ok(())
        }
        WorkerCommands::Status { worker_id } => {
            debug!(worker_id = ?worker_id, "querying process status");
            println!("Status: {worker_id:?}");
            Ok(())
        }
        WorkerCommands::Stop { worker_id } => {
            let mut client = connect(port).await?;
            process_stop(&mut client, &worker_id).await
        }
        WorkerCommands::Dir { worker_id } => {
            let dir = process_workspace_dir(port, &worker_id).await?;
            println!("{dir}");
            Ok(())
        }
        WorkerCommands::Vscode { worker_id } => {
            let dir = process_workspace_dir(port, &worker_id).await?;
            let status = process::Command::new("code")
                .arg(&dir)
                .status()
                .context("failed to launch VS Code — is `code` on your PATH?")?;
            if !status.success() {
                bail!("VS Code exited with {status}");
            }
            Ok(())
        }
    }
}

/// Fetch the host workspace directory for a running process via gRPC.
async fn process_workspace_dir(port: u16, worker_id: &str) -> Result<String> {
    let mut client = connect(port).await?;
    let resp = client
        .worker_info(WorkerInfoRequest {
            worker_id: worker_id.to_owned(),
        })
        .await?;
    let workspace_dir = resp.into_inner().workspace_dir;
    if workspace_dir.is_empty() {
        bail!("no workspace directory for process {worker_id}");
    }
    Ok(workspace_dir)
}

/// Extract the subcommand name for span fields.
fn command_name(cmd: &WorkerCommands) -> &'static str {
    match cmd {
        WorkerCommands::Attach { .. } => "attach",
        WorkerCommands::Kill { .. } => "kill",
        WorkerCommands::List => "list",
        WorkerCommands::SaveCredentials { .. } => "save_credentials",
        WorkerCommands::Launch { .. } => "launch",
        WorkerCommands::Status { .. } => "status",
        WorkerCommands::Stop { .. } => "stop",
        WorkerCommands::Dir { .. } => "dir",
        WorkerCommands::Vscode { .. } => "vscode",
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
        Commands::Worker { command } => {
            let project_keys: Vec<String> = config.projects.keys().cloned().collect();
            handle_worker(command, port, &config.network.agent_prefix, &project_keys).await?
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
        Commands::Project { command } => match command {
            ProjectCommands::List => project::list(&config)?,
            ProjectCommands::Add {
                path,
                key,
                name,
                pool_limit,
            } => project::add(&config, &path, key.as_deref(), name.as_deref(), pool_limit)?,
            ProjectCommands::Remove { key, force } => project::remove(&config, &key, force)?,
        },
        Commands::Rag { command } => match command {
            RagCommands::Docs => rag::generate_docs(&config)?,
            RagCommands::Index { language } => rag::index(port, &language).await?,
            RagCommands::Search {
                query,
                language,
                top_k,
            } => rag::search(port, &query, &language, top_k).await?,
            RagCommands::Model { command } => match command {
                ModelCommands::Download => rag::download_model(&config)?,
            },
        },
        Commands::Db { command } => match command {
            DbCommands::Backup => db::backup(&config).await?,
            DbCommands::Restore { path } => db::restore(&config, &path).await?,
            DbCommands::List => db::list(&config)?,
        },
        Commands::Ticket { command } => ticket::handle(port, command).await?,
        Commands::Agent { command } => match command {
            AgentCommands::Dir { worker_id } => {
                let mut client = connect(port).await?;
                let resp = client
                    .worker_info(WorkerInfoRequest {
                        worker_id: worker_id.clone(),
                    })
                    .await?;
                let workspace_dir = resp.into_inner().workspace_dir;
                if workspace_dir.is_empty() {
                    bail!("no workspace directory for agent {worker_id}");
                }
                println!("{workspace_dir}");
            }
        },
    }
    Ok(())
}
