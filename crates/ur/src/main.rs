mod admin;
mod builderd;
mod compose;
mod credential;
mod db;
mod describe;
mod init;
mod input;
mod lifecycle_log;
mod logging;
mod output;
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
use ur_rpc::error::StatusResultExt;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::*;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::*;

use compose::{ComposeManager, compose_manager_from_config};
use output::{
    ContainerKilled, CredentialsSaved, ErrorCode, OutputManager, StructuredError, WorkerDir,
    WorkerLaunched, WorkerStopped,
};

#[derive(Parser)]
#[command(name = "ur", about = "Coding LLM coordination framework")]
struct Cli {
    /// TCP port of the server gRPC server (overrides ur.toml)
    #[arg(long)]
    port: Option<u16>,

    /// Output format: text or json (also: OUTPUT_FORMAT env var)
    #[arg(long, global = true)]
    output: Option<String>,

    /// Print command schema as JSON and exit
    #[arg(long, global = true)]
    describe: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Privileged admin operations (blocked from workers)
    Admin {
        #[command(subcommand)]
        command: admin::AdminCommands,
    },
    /// Database backup and restore
    Db {
        #[command(subcommand)]
        command: DbCommands,
    },
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
    /// Manage projects
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
    /// Manage the forward proxy domain allowlist
    Proxy {
        #[command(subcommand)]
        command: ProxyCommands,
    },
    /// RAG documentation and search
    Rag {
        #[command(subcommand)]
        command: RagCommands,
    },
    /// Manage the ur-server lifecycle
    Server {
        #[command(subcommand)]
        command: ServerCommands,
    },
    /// Manage tickets
    Ticket {
        #[command(subcommand)]
        command: ticket_client::TicketArgs,
    },
    /// Manage workers
    Worker {
        #[command(subcommand)]
        command: WorkerCommands,
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
    /// List all configured projects with pool usage
    List,
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
    /// Manage embedding models
    Model {
        #[command(subcommand)]
        command: ModelCommands,
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
}

#[derive(Subcommand)]
enum ModelCommands {
    /// Download the configured embedding model to the local cache
    Download,
}

#[derive(Subcommand)]
enum ServerCommands {
    /// Redeploy a single infrastructure component without rebuilding
    Redeploy {
        /// Component to redeploy
        component: Component,
    },
    /// Restart the server (stop then start)
    Restart,
    /// Start the server
    Start,
    /// Kill all containers and stop the server
    Stop,
}

#[derive(Clone, Debug, clap::ValueEnum)]
enum Component {
    /// ur-server container
    Server,
    /// builderd host-native process
    Builderd,
    /// ur-squid proxy container
    Squid,
    /// ur-qdrant vector DB container
    Qdrant,
    /// All components (builderd, squid, qdrant, server)
    All,
}

#[derive(Subcommand)]
enum DbCommands {
    /// Create an on-demand database backup
    Backup,
    /// List available backup files
    List,
    /// Restore a database from a backup file
    Restore {
        /// Path to the backup file to restore
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum WorkerCommands {
    /// Attach to a running process
    Attach {
        worker_id: String,
        /// Stop the process when the attach session exits
        #[arg(long)]
        rm: bool,
    },
    /// Print the host directory assigned to a running process
    Dir { worker_id: String },
    /// Force-stop a running worker process (via server)
    Kill { worker_id: String },
    /// Launch a new worker process
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
        /// Dispatch a ticket: validate it exists and is open, then transition to implementing
        #[arg(short = 'd', long = "dispatch")]
        dispatch: Option<String>,
    },
    /// List all running processes
    List,
    /// Save credentials from a running container for reuse
    SaveCredentials { worker_id: String },
    /// Show process status
    Status { worker_id: Option<String> },
    /// Send a message to a running worker's agent
    Send { worker_id: String, message: String },
    /// Stop a running worker process
    Stop { worker_id: String },
    /// Open the host directory for a running process in VS Code
    Vscode { worker_id: String },
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

#[instrument(skip(config, compose, output))]
fn start_server(
    config: &ur_config::Config,
    compose: &ComposeManager,
    output: &OutputManager,
) -> Result<()> {
    let log = lifecycle_log::LifecycleLog::open(&config.config_dir);
    log.info("ur start: beginning");
    info!("starting server");

    match builderd::start_builderd(config, output) {
        Ok(()) => log.info("ur start: builderd started"),
        Err(e) => {
            log.error(&format!("ur start: builderd failed: {e}"));
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
    output.print_text("server started");
    log.info("ur start: complete");

    // Check if shared credentials exist; if not, hint about Keychain seeding.
    let has_credentials = credential::CredentialManager::host_credentials_path()
        .ok()
        .and_then(|p| std::fs::metadata(&p).ok())
        .is_some_and(|m| m.len() > 0);
    if !has_credentials {
        warn!("no shared credentials found");
        if !output.is_json() {
            println!();
            println!("No shared credentials found. Log in to Claude Code on this machine first.");
            println!("Credentials will be seeded from the macOS Keychain on first process launch.");
        }
    }

    Ok(())
}

#[instrument(skip(config, compose, output))]
async fn stop_server(
    config: &ur_config::Config,
    compose: &ComposeManager,
    output: &OutputManager,
) -> Result<()> {
    let log = lifecycle_log::LifecycleLog::open(&config.config_dir);
    log.info("ur stop: beginning");
    info!("stopping server");

    // Try graceful stop via gRPC (proper slot release + DB cleanup), fall back to Docker
    let port = config.daemon_port;
    let addr = format!("http://127.0.0.1:{port}");
    if let Some(mut client) = try_connect(&addr).await {
        info!("server reachable — stopping workers via gRPC");
        log.info("ur stop: stopping workers via gRPC");
        stop_workers_via_grpc(&mut client, output).await;
    } else {
        info!("server unreachable — stopping workers via Docker");
        log.info("ur stop: stopping workers via Docker (server unreachable)");
        kill_all_containers(&config.network.worker_prefix, output)?;
    }

    if !compose.is_running()? {
        info!("server is not running, nothing to stop");
        output.print_text("server is not running");
        log.info("ur stop: server was not running");
        return Ok(());
    }
    compose.down()?;
    info!("server stopped successfully");
    output.print_text("server stopped");
    log.info("ur stop: compose down succeeded");

    builderd::stop_builderd(config, output)?;
    log.info("ur stop: builderd stopped");
    log.info("ur stop: complete");
    Ok(())
}

#[instrument(skip(config, compose, output))]
fn redeploy_component(
    component: &Component,
    config: &ur_config::Config,
    compose: &ComposeManager,
    output: &OutputManager,
) -> Result<()> {
    match component {
        Component::Builderd => {
            output.print_text("redeploying builderd...");
            builderd::stop_builderd(config, output)?;
            builderd::start_builderd(config, output)?;
        }
        Component::Squid => {
            output.print_text("redeploying squid...");
            compose.recreate_service("ur-squid")?;
            output.print_text("squid redeployed");
        }
        Component::Qdrant => {
            output.print_text("redeploying qdrant...");
            compose.recreate_service("ur-qdrant")?;
            output.print_text("qdrant redeployed");
        }
        Component::Server => {
            output.print_text("redeploying server...");
            compose.recreate_service("ur-server")?;
            output.print_text("server redeployed");
        }
        Component::All => {
            redeploy_component(&Component::Builderd, config, compose, output)?;
            redeploy_component(&Component::Squid, config, compose, output)?;
            redeploy_component(&Component::Qdrant, config, compose, output)?;
            redeploy_component(&Component::Server, config, compose, output)?;
            output.print_text("all components redeployed");
        }
    }
    Ok(())
}

/// Stop all running workers via the server's gRPC API in parallel.
///
/// Best-effort: logs warnings for individual failures but does not propagate errors,
/// since the server is about to be stopped anyway.
async fn stop_workers_via_grpc(client: &mut CoreServiceClient<Channel>, output: &OutputManager) {
    let workers = match client.worker_list(WorkerListRequest {}).await {
        Ok(resp) => resp.into_inner().workers,
        Err(e) => {
            warn!(error = %e, "failed to list workers via gRPC");
            return;
        }
    };

    if workers.is_empty() {
        output.print_text("No workers running");
        return;
    }

    info!(
        count = workers.len(),
        "stopping workers via gRPC in parallel"
    );

    let mut set = tokio::task::JoinSet::new();
    for w in &workers {
        let mut c = client.clone();
        let wid = w.worker_id.clone();
        set.spawn(async move {
            let result = c
                .worker_stop(WorkerStopRequest {
                    worker_id: wid.clone(),
                })
                .await;
            (wid, result)
        });
    }

    while let Some(join_result) = set.join_next().await {
        let (wid, result) = join_result.expect("worker stop task panicked");
        let result: Result<(), tonic::Status> = result.map(|_| ());
        match result {
            Ok(_) => {
                info!(worker_id = %wid, "worker stopped via gRPC");
                if output.is_json() {
                    output.print_success(&WorkerStopped {
                        worker_id: wid.clone(),
                    });
                } else {
                    println!("Stopped {wid}");
                }
            }
            Err(e) => {
                warn!(worker_id = %wid, error = %e, "failed to stop worker via gRPC");
                eprintln!("Warning: failed to stop {wid}: {e}");
            }
        }
    }
}

#[instrument]
async fn connect(port: u16) -> Result<CoreServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");

    match try_connect(&addr).await {
        Some(client) => Ok(client),
        None => {
            error!(port, "server is not running");
            bail!("server is not running — run 'ur server start' first")
        }
    }
}

#[instrument(skip(output))]
fn kill_all_containers(worker_prefix: &str, output: &OutputManager) -> Result<()> {
    let rt = container::runtime_from_env();
    let containers = rt.list_by_prefix(worker_prefix)?;
    if containers.is_empty() {
        debug!(worker_prefix, "no worker containers running");
        output.print_text(&format!(
            "No worker containers running (prefix: {worker_prefix})"
        ));
        return Ok(());
    }
    info!(
        count = containers.len(),
        worker_prefix, "killing all worker containers"
    );

    // Stop and remove all containers in parallel
    let handles: Vec<_> = containers
        .into_iter()
        .map(|id| {
            std::thread::spawn(move || {
                let rt = container::runtime_from_env();
                let stop_err = rt.stop(&id).err();
                let rm_err = rt.rm(&id).err();
                (id, stop_err, rm_err)
            })
        })
        .collect();

    for handle in handles {
        let (id, stop_err, rm_err) = handle.join().expect("container kill thread panicked");
        if let Some(e) = stop_err {
            warn!(container = %id.0, error = %e, "failed to stop container");
            eprintln!("Warning: failed to stop {}: {e}", id.0);
        }
        if let Some(e) = rm_err {
            warn!(container = %id.0, error = %e, "failed to remove container");
            eprintln!("Warning: failed to remove {}: {e}", id.0);
        }
        info!(container = %id.0, "container killed");
        if output.is_json() {
            output.print_success(&ContainerKilled {
                container_id: id.0.clone(),
            });
        } else {
            println!("Killed {}", id.0);
        }
    }
    Ok(())
}

/// Wait for a container's Docker HEALTHCHECK to report "healthy".
/// Polls every 500ms for up to 60s.
fn wait_for_healthy(worker_id: &str, worker_prefix: &str) -> Result<()> {
    let runtime = container::runtime_from_env();
    let id = ContainerId(format!("{worker_prefix}{worker_id}"));
    let max_attempts = 120; // 60s at 500ms intervals
    let mut printed = false;
    for i in 0..max_attempts {
        let status = runtime.health_status(&id).unwrap_or_default();
        if status == "healthy" {
            if printed {
                eprintln!(" ready");
            }
            return Ok(());
        }
        if status == "unhealthy" {
            if printed {
                eprintln!();
            }
            bail!("container {} became unhealthy", id.0);
        }
        if i == 0 {
            eprint!("Waiting for worker to initialize");
            printed = true;
        }
        eprint!(".");
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if printed {
        eprintln!();
    }
    bail!("container {} did not become healthy after 60s", id.0);
}

#[instrument]
fn process_attach(worker_id: &str, worker_prefix: &str) -> Result<i32> {
    let runtime = container::runtime_from_env();
    let id = ContainerId(format!("{worker_prefix}{worker_id}"));
    info!(container = %id.0, "attaching to agent session");
    // Attach to the `agent` tmux session managed by workerd. This lets the user
    // see the live Claude Code session. Multiple clients can attach simultaneously
    // and send-keys works regardless of attached clients.
    let session = tmux::Session::from_name("agent");
    let command = session.attach_command();
    let status = runtime.exec_interactive(&id, &command)?;
    Ok(status.code().unwrap_or(1))
}

#[instrument(skip(client, output))]
async fn process_list(
    client: &mut CoreServiceClient<Channel>,
    output: &OutputManager,
) -> Result<()> {
    info!("listing processes");
    let resp = client.worker_list(WorkerListRequest {}).await?;
    let workers = resp.into_inner().workers;
    if workers.is_empty() {
        output.print_text("No running workers.");
        return Ok(());
    }
    output.print_items(&workers, |workers| {
        let mut out = format!(
            "{:<20} {:<12} {:<16} {:<8} {:<12} {:<14} {}\n",
            "WORKER", "PROJECT", "CONTAINER", "MODE", "STATUS", "AGENT", "DIRECTORY"
        );
        for w in workers {
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
            let directory = if w.directory.is_empty() {
                "-"
            } else {
                &w.directory
            };
            let container_status = if w.container_status.is_empty() {
                "-"
            } else {
                &w.container_status
            };
            let agent_status = if w.agent_status.is_empty() {
                "-"
            } else {
                &w.agent_status
            };
            out.push_str(&format!(
                "{:<20} {:<12} {:<16} {:<8} {:<12} {:<14} {}\n",
                w.worker_id,
                project,
                container_short,
                w.mode,
                container_status,
                agent_status,
                directory
            ));
        }
        if out.ends_with('\n') {
            out.pop();
        }
        out
    });
    Ok(())
}

#[instrument(skip(client, output))]
async fn process_status(
    client: &mut CoreServiceClient<Channel>,
    worker_id: Option<&str>,
    output: &OutputManager,
) -> Result<()> {
    info!("querying process status");
    let resp = client.worker_list(WorkerListRequest {}).await?;
    let workers = resp.into_inner().workers;

    let filtered: Vec<_> = if let Some(id) = worker_id {
        workers.into_iter().filter(|w| w.worker_id == id).collect()
    } else {
        workers
    };

    if filtered.is_empty() {
        if let Some(id) = worker_id {
            bail!("unknown process: {id}");
        }
        output.print_text("No running workers.");
        return Ok(());
    }

    output.print_items(&filtered, |workers| {
        let mut out = format!(
            "{:<20} {:<12} {:<14} {:<8} {}\n",
            "WORKER", "STATUS", "AGENT", "MODE", "DIRECTORY"
        );
        for w in workers {
            let container_status = if w.container_status.is_empty() {
                "-"
            } else {
                &w.container_status
            };
            let agent_status = if w.agent_status.is_empty() {
                "-"
            } else {
                &w.agent_status
            };
            let directory = if w.directory.is_empty() {
                "-"
            } else {
                &w.directory
            };
            out.push_str(&format!(
                "{:<20} {:<12} {:<14} {:<8} {}\n",
                w.worker_id, container_status, agent_status, w.mode, directory
            ));
        }
        if out.ends_with('\n') {
            out.pop();
        }
        out
    });
    Ok(())
}

/// Validate a ticket exists and is in "open" lifecycle_status, then transition it to "implementing".
async fn dispatch_ticket(port: u16, ticket_id: &str) -> Result<()> {
    let addr = format!("http://127.0.0.1:{port}");
    let channel = Endpoint::try_from(addr)?
        .connect()
        .await
        .context("server is not running — run 'ur server start' first")?;
    let mut ticket_client = TicketServiceClient::new(channel);

    let resp = ticket_client
        .get_ticket(GetTicketRequest {
            id: ticket_id.to_owned(),
        })
        .await
        .with_status_context("get ticket for dispatch")?;

    let ticket = resp
        .into_inner()
        .ticket
        .ok_or_else(|| anyhow::anyhow!("ticket {ticket_id} not found"))?;

    if ticket.lifecycle_status != "open" {
        bail!(
            "ticket {ticket_id} has lifecycle_status '{}', expected 'open'",
            ticket.lifecycle_status
        );
    }

    ticket_client
        .update_ticket(UpdateTicketRequest {
            id: ticket_id.to_owned(),
            status: None,
            priority: None,
            title: None,
            body: None,
            force: false,
            ticket_type: None,
            parent_id: None,
            lifecycle_status: Some("implementing".to_owned()),
            branch: None,
        })
        .await
        .with_status_context("transition ticket to implementing")?;

    info!(ticket_id, "dispatched ticket to implementing");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[instrument(skip(client, output), fields(ticket_id, workspace = ?workspace, project_key = ?project_key, mode, skills = ?skills))]
async fn process_launch(
    client: &mut CoreServiceClient<Channel>,
    ticket_id: &str,
    workspace: Option<PathBuf>,
    project_key: &str,
    worker_prefix: &str,
    mode: &str,
    skills: &[String],
    output: &OutputManager,
) -> Result<()> {
    info!(ticket_id, project_key, "launching worker process");

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
    let container_name = format!("{worker_prefix}{ticket_id}");
    if !output.is_json() {
        println!("Launching worker {container_name}...");
    }
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
        container_name, container_id, image_id, "worker process launched"
    );
    if output.is_json() {
        output.print_success(&WorkerLaunched {
            worker_id: ticket_id.to_string(),
            container_id,
        });
    } else {
        println!("Worker {container_name} running (container {container_id})");
    }
    Ok(())
}

#[instrument(skip(client, output))]
async fn process_stop(
    client: &mut CoreServiceClient<Channel>,
    worker_id: &str,
    output: &OutputManager,
) -> Result<()> {
    info!(worker_id, "stopping worker process");
    if !output.is_json() {
        println!("Stopping {worker_id}...");
    }
    client
        .worker_stop(WorkerStopRequest {
            worker_id: worker_id.into(),
        })
        .await?;
    info!(worker_id, "worker process stopped");
    if output.is_json() {
        output.print_success(&WorkerStopped {
            worker_id: worker_id.to_string(),
        });
    } else {
        println!("Worker {worker_id} stopped.");
    }
    Ok(())
}

#[instrument(skip(command, project_keys, output), fields(command_name = command_name(&command)))]
async fn handle_worker(
    command: WorkerCommands,
    port: u16,
    worker_prefix: &str,
    project_keys: &[String],
    output: &OutputManager,
) -> Result<()> {
    match command {
        WorkerCommands::List => {
            let mut client = connect(port).await?;
            process_list(&mut client, output).await
        }
        WorkerCommands::Attach { worker_id, rm } => {
            if output.is_json() {
                let err = StructuredError::new(
                    ErrorCode::InteractiveNotSupported,
                    "attach is an interactive command and cannot produce JSON output",
                );
                output.print_error(&err);
                process::exit(err.code.exit_code());
            }
            let exit_code = process_attach(&worker_id, worker_prefix)?;
            if rm {
                println!("Stopping {worker_id} (--rm)...");
                let mut client = connect(port).await?;
                process_stop(&mut client, &worker_id, output).await?;
            }
            process::exit(exit_code);
        }
        WorkerCommands::Kill { worker_id } => {
            input::validate_id(&worker_id, "worker_id")?;
            let mut client = connect(port).await?;
            process_stop(&mut client, &worker_id, output).await
        }
        WorkerCommands::SaveCredentials { worker_id } => {
            input::validate_id(&worker_id, "worker_id")?;
            info!(worker_id = %worker_id, "saving credentials from container");
            let runtime = container::runtime_from_env();
            let id = container::ContainerId(format!("{worker_prefix}{worker_id}"));
            let cred_mgr = credential::CredentialManager;
            let paths = cred_mgr.save_from_container(&runtime, &id)?;
            if output.is_json() {
                output.print_success(&CredentialsSaved {
                    paths: paths.iter().map(|p| p.display().to_string()).collect(),
                });
            } else {
                for path in &paths {
                    info!(path = %path.display(), "saved credential file");
                    println!("Saved {}", path.display());
                }
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
            dispatch,
        } => {
            input::validate_id(&ticket_id, "ticket_id")?;
            if let Some(ref p) = project {
                input::validate_id(p, "project")?;
            }
            if let Some(ref w) = workspace {
                input::reject_path_traversal(w, "workspace")?;
            }

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
                let _ = process_stop(&mut client, &ticket_id, output).await;
            }
            if let Some(ref dispatch_ticket_id) = dispatch {
                dispatch_ticket(port, dispatch_ticket_id).await?;
            }
            process_launch(
                &mut client,
                &ticket_id,
                workspace,
                &resolved_project,
                worker_prefix,
                &mode,
                &skills_vec,
                output,
            )
            .await?;
            if attach || rm {
                if output.is_json() {
                    let err = StructuredError::new(
                        ErrorCode::InteractiveNotSupported,
                        "attach is an interactive command and cannot produce JSON output",
                    );
                    output.print_error(&err);
                    process::exit(err.code.exit_code());
                }
                wait_for_healthy(&ticket_id, worker_prefix)?;
                let exit_code = process_attach(&ticket_id, worker_prefix)?;
                if rm {
                    println!("Stopping {ticket_id} (--rm)...");
                    let mut client = connect(port).await?;
                    process_stop(&mut client, &ticket_id, output).await?;
                }
                process::exit(exit_code);
            }
            Ok(())
        }
        WorkerCommands::Status { worker_id } => {
            debug!(worker_id = ?worker_id, "querying process status");
            let mut client = connect(port).await?;
            process_status(&mut client, worker_id.as_deref(), output).await
        }
        WorkerCommands::Send { worker_id, message } => {
            input::validate_id(&worker_id, "worker_id")?;
            let mut client = connect(port).await?;
            info!(worker_id = %worker_id, "sending message to worker");
            client
                .send_worker_message(SendWorkerMessageRequest {
                    worker_id: worker_id.clone(),
                    message,
                })
                .await?;
            if output.is_json() {
                output.print_text(&format!(
                    "{{\"worker_id\":\"{worker_id}\",\"status\":\"sent\"}}"
                ));
            } else {
                println!("Message sent to {worker_id}.");
            }
            Ok(())
        }
        WorkerCommands::Stop { worker_id } => {
            input::validate_id(&worker_id, "worker_id")?;
            let mut client = connect(port).await?;
            process_stop(&mut client, &worker_id, output).await
        }
        WorkerCommands::Dir { worker_id } => {
            input::validate_id(&worker_id, "worker_id")?;
            let dir = process_workspace_dir(port, &worker_id).await?;
            if output.is_json() {
                output.print_success(&WorkerDir { path: dir });
            } else {
                println!("{dir}");
            }
            Ok(())
        }
        WorkerCommands::Vscode { worker_id } => {
            input::validate_id(&worker_id, "worker_id")?;
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
        WorkerCommands::Send { .. } => "send",
        WorkerCommands::Launch { .. } => "launch",
        WorkerCommands::Status { .. } => "status",
        WorkerCommands::Stop { .. } => "stop",
        WorkerCommands::Dir { .. } => "dir",
        WorkerCommands::Vscode { .. } => "vscode",
    }
}

fn main() {
    let output_format = output::resolve_output_format_early();

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            output::handle_clap_error(e, output_format);
        }
    };

    let out = OutputManager::from_args(cli.output.as_deref());

    // Handle --describe: print schema JSON and exit
    if cli.describe {
        let cmd = <Cli as clap::CommandFactory>::command();
        let schema = describe::describe_command(&cmd);
        println!("{}", serde_json::to_string_pretty(&schema).unwrap());
        return;
    }

    let rt = tokio::runtime::Runtime::new().unwrap();

    if let Err(err) = rt.block_on(run(cli, &out)) {
        let structured = StructuredError::from_anyhow(&err);
        out.print_error(&structured);
        std::process::exit(structured.code.exit_code());
    }
}

async fn run(cli: Cli, output: &OutputManager) -> Result<()> {
    // Init bypasses config loading — it creates the config files.
    if let Commands::Init {
        force,
        force_config,
        force_squid,
    } = cli.command
    {
        return init::run(
            init::InitFlags {
                force,
                force_config,
                force_squid,
            },
            output,
        );
    }

    let config = load_config()?;

    // Initialize structured JSON file logging after config is loaded so we
    // know where to write the log file. The guard must live until main exits.
    let _log_guard = logging::init(&config.config_dir);

    info!(
        config_dir = %config.config_dir.display(),
        daemon_port = config.daemon_port,
        builderd_port = config.builderd_port,
        "ur CLI started"
    );

    let port = resolve_daemon_port(cli.port, &config);
    let compose = compose_manager_from_config(&config);

    match cli.command {
        Commands::Admin { command } => admin::handle(port, command, output).await?,
        Commands::Db { command } => match command {
            DbCommands::Backup => db::backup(&config, output).await?,
            DbCommands::List => db::list(&config, output)?,
            DbCommands::Restore { path } => {
                input::reject_path_traversal(&path, "path")?;
                db::restore(&config, &path, output).await?
            }
        },
        Commands::Init { .. } => unreachable!(),
        Commands::Project { command } => match command {
            ProjectCommands::Add {
                path,
                key,
                name,
                pool_limit,
            } => {
                if let Some(ref k) = key {
                    input::validate_id(k, "key")?;
                }
                if let Some(ref n) = name {
                    input::reject_control_chars(n, "name")?;
                }
                input::reject_path_traversal(&path, "path")?;
                project::add(
                    &config,
                    &path,
                    key.as_deref(),
                    name.as_deref(),
                    pool_limit,
                    output,
                )?
            }
            ProjectCommands::List => project::list(&config, output)?,
            ProjectCommands::Remove { key, force } => {
                input::validate_id(&key, "key")?;
                project::remove(&config, &key, force, output)?
            }
        },
        Commands::Proxy { command } => {
            let squid_dir = config.squid_dir();
            let allowlist_path = squid_dir.join("allowlist.txt");
            match command {
                ProxyCommands::Allow { domain } => {
                    input::validate_domain(&domain)?;
                    info!(domain = %domain, "allowing domain through proxy");
                    let domains = proxy::allow_domain(&allowlist_path, &domain)?;
                    proxy::signal_reconfigure(&config.proxy.hostname);
                    proxy::print_domains(&domains, output);
                }
                ProxyCommands::Block { domain } => {
                    input::validate_domain(&domain)?;
                    info!(domain = %domain, "blocking domain from proxy");
                    let domains = proxy::block_domain(&allowlist_path, &domain)?;
                    proxy::signal_reconfigure(&config.proxy.hostname);
                    proxy::print_domains(&domains, output);
                }
                ProxyCommands::List => {
                    debug!("listing proxy domains");
                    let domains = proxy::read_allowlist(&allowlist_path)?;
                    proxy::print_domains(&domains, output);
                }
            }
        }
        Commands::Rag { command } => match command {
            RagCommands::Docs => rag::generate_docs(&config, output)?,
            RagCommands::Index { language } => rag::index(port, &language, output).await?,
            RagCommands::Model { command } => match command {
                ModelCommands::Download => rag::download_model(&config, output)?,
            },
            RagCommands::Search {
                query,
                language,
                top_k,
            } => {
                input::reject_control_chars(&query, "query")?;
                rag::search(port, &query, &language, top_k, output).await?
            }
        },
        Commands::Server { command } => match command {
            ServerCommands::Redeploy { component } => {
                redeploy_component(&component, &config, &compose, output)?;
            }
            ServerCommands::Restart => {
                stop_server(&config, &compose, output).await?;
                start_server(&config, &compose, output)?;
            }
            ServerCommands::Start => start_server(&config, &compose, output)?,
            ServerCommands::Stop => stop_server(&config, &compose, output).await?,
        },
        Commands::Ticket { command } => ticket::handle(port, command, output).await?,
        Commands::Worker { command } => {
            let project_keys: Vec<String> = config.projects.keys().cloned().collect();
            handle_worker(
                command,
                port,
                &config.network.worker_prefix,
                &project_keys,
                output,
            )
            .await?
        }
    }
    Ok(())
}
