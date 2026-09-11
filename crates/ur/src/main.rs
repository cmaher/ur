mod builder;
mod builderd;
mod compose;
pub(crate) mod connection;
mod credential;
mod db;
mod describe;
mod flow;
mod init;
mod input;
mod logging;
mod output;
mod project;
mod proxy;
mod ticket;
mod tui;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use container::{ContainerId, ContainerRuntime};
use tonic::transport::Channel;
use tracing::{debug, info, instrument, warn};
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
    /// Inspect and interact with the builderd exec helper
    Builder {
        #[command(subcommand)]
        command: builder::BuilderCommands,
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
    /// Manage the ur-server lifecycle
    Server {
        #[command(subcommand)]
        command: ServerCommands,
    },
    /// Manage tickets
    Ticket {
        #[command(subcommand)]
        command: crate::ticket::TicketArgs,
    },
    /// Manage TUI settings (themes, keymaps)
    Tui {
        #[command(subcommand)]
        command: Box<crate::tui::TuiArgs>,
    },
    /// Manage workflows
    Flow {
        #[command(subcommand)]
        command: crate::flow::FlowArgs,
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
    /// Add a new project from a local git directory (or `--local` for a repo-less directory)
    Add {
        /// Path to a git repository directory (e.g. "." for current directory).
        /// With --local, any directory — it need not be a git repo.
        path: PathBuf,
        /// Container image alias (`ur-worker`) or full image reference
        /// [default: ur-worker]
        #[arg(long)]
        image: Option<String>,
        /// Project key (derived from repo name, or from the directory name with --local)
        #[arg(long)]
        key: Option<String>,
        /// Display-friendly project name
        #[arg(long)]
        name: Option<String>,
        /// Maximum number of cached repo clones (default: 10). Not valid with --local.
        #[arg(long)]
        pool_limit: Option<u32>,
        /// Add a repo-less local project: no git remote, no repo pool, no dispatch.
        /// Usable only in workspace mode (`ur worker launch -m manual -w <dir>`).
        #[arg(long)]
        local: bool,
    },
    /// List all configured projects with pool usage
    List,
    /// Remove a project and delete all pool clones
    Remove {
        /// Project key to remove
        key: String,
        /// Required to confirm deletion of pool clones (not needed for local projects)
        #[arg(long)]
        force: bool,
    },
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
    /// Launch a new worker process (requires -p project or -w workspace; manual mode accepts both)
    Launch {
        ticket_id: Option<String>,
        /// Mount a host directory as the container workspace
        #[arg(short = 'w', long = "workspace")]
        workspace: Option<PathBuf>,
        /// Project key — determines container image and config from project config
        #[arg(short = 'p', long = "project")]
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
        dispatch: bool,
        /// Comma-separated list of project keys to mount as read-only context repositories
        #[arg(long = "context-repos")]
        context_repos: Option<String>,
        /// Which agent to run (default: the mode's `agent` field, else the
        /// configured top-level agent). Overrides both.
        #[arg(long)]
        agent: Option<String>,
    },
    /// List all running processes
    List,
    /// Force re-seed shared agent credentials from the host
    ReseedCredentials {
        /// Which agent to reseed credentials for (default: the configured top-level agent)
        #[arg(long)]
        agent: Option<String>,
    },
    /// Save credentials from a running container for reuse
    SaveCredentials {
        worker_id: String,
        /// Which agent to save credentials for (default: the configured top-level agent)
        #[arg(long)]
        agent: Option<String>,
    },
    /// Show detailed worker information
    Describe { worker_id: Option<String> },
    /// Send a message to a running worker's agent
    Send {
        worker_id: String,
        message: String,
        /// Type the message into the agent's prompt without submitting it
        #[arg(long = "no-submit")]
        no_submit: bool,
    },
    /// Stop a running worker process
    Stop { worker_id: String },
    /// Open the host directory for a running process in VS Code
    Code { worker_id: String },
}

#[instrument]
fn load_config() -> Result<ur_config::Config> {
    debug!("loading ur config");
    ur_config::Config::load().context("failed to load config")
}

/// Pre-create writable mount source directories for all projects.
///
/// The server runs in a container and cannot reach arbitrary macOS host paths.
/// If a writable mount source doesn't exist, Docker creates it as root, making
/// it unwritable by the worker user inside the container.
fn prepare_project_mounts(config: &ur_config::Config) {
    for (key, project) in &config.projects {
        for mount in &project.container.mounts {
            if mount.readonly {
                continue;
            }
            let resolved = ur_config::resolve_template_path(&mount.source, &config.config_dir);
            let host_path = match resolved {
                Ok(ur_config::ResolvedTemplatePath::HostPath(p)) => p,
                _ => continue,
            };
            if let Err(e) = std::fs::create_dir_all(&host_path) {
                warn!(
                    project = key,
                    path = %host_path.display(),
                    error = %e,
                    "failed to create mount source directory"
                );
            }
        }
    }
}

/// Warn when the configured default agent has no seeded credentials, naming
/// that agent's own remediation.
///
/// Only the default agent is checked, not every agent in `AgentType::ALL`: a
/// Claude-only user must not be nagged about an unconfigured codex (and vice
/// versa), and a launch that resolves to some *other* agent still fails loudly
/// at the RPC via `check_credentials_seeded` with the same remediation text.
fn warn_if_default_agent_unseeded(agent: ur_config::AgentType, output: &OutputManager) {
    if agent
        .auth()
        .is_some_and(|auth| auth.source == ur_config::AuthSource::InContainer)
    {
        return;
    }
    let Some(cred_mgr) = credential::credential_manager_for(agent) else {
        return;
    };
    let seeded = cred_mgr
        .host_credentials_path()
        .is_ok_and(|p| ur_config::credentials_file_is_seeded(&p));
    if seeded {
        return;
    }
    // Reuse the launch-time remediation text verbatim so `ur start`'s guidance
    // and the `MissingCredentials` RPC error a later launch would produce say
    // exactly the same thing.
    let remediation = agent.credentials_remediation();
    warn!(agent = agent.name(), %remediation, "no shared credentials found");
    if !output.is_json() {
        println!();
        println!("{remediation}");
    }
}

#[instrument(skip(config, compose, output))]
fn start_server(
    config: &ur_config::Config,
    compose: &ComposeManager,
    output: &OutputManager,
) -> Result<()> {
    info!("starting server");

    prepare_project_mounts(config);

    // Seed credentials for every known agent before starting anything so
    // they're available for bind-mounting into worker containers. Force a
    // re-seed on every start so host re-logins propagate after a restart.
    if let Err(e) = credential::ensure_credentials_for_all_agents(Duration::ZERO) {
        warn!(error = %e, "credential seeding failed");
    }
    warn_if_default_agent_unseeded(config.agent, output);

    match builderd::start_builderd(config, output) {
        Ok(()) => info!("builderd started"),
        Err(e) => {
            info!(error = %e, "builderd failed to start");
            return Err(e);
        }
    }

    match compose.up() {
        Ok(()) => info!("compose up succeeded"),
        Err(e) => {
            info!(error = %e, "compose up failed");
            return Err(e);
        }
    }

    info!("server started successfully");
    output.print_text("server started");

    Ok(())
}

#[instrument(skip(config, compose, output))]
async fn stop_server(
    config: &ur_config::Config,
    compose: &ComposeManager,
    output: &OutputManager,
) -> Result<()> {
    info!("stopping server");

    // Try graceful stop via gRPC (proper slot release + DB cleanup), fall back to Docker
    let port = config.server_port;
    if let Some(channel) = connection::try_connect(port) {
        let mut client = CoreServiceClient::new(channel);
        info!("server reachable — stopping workers via gRPC");
        stop_workers_via_grpc(&mut client, output).await;
    } else {
        info!("server unreachable — stopping workers via Docker");
        kill_all_containers(&config.network.worker_prefix, output)?;
    }

    if !compose.is_running()? {
        info!("server is not running, nothing to stop");
        output.print_text("server is not running");
        return Ok(());
    }
    compose.down()?;
    info!("compose down succeeded");
    output.print_text("server stopped");

    builderd::stop_builderd(config, output)?;
    info!("builderd stopped");
    info!("server stop complete");
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

async fn connect(port: u16) -> Result<CoreServiceClient<Channel>> {
    let channel = connection::connect(port).await?;
    Ok(CoreServiceClient::new(channel))
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

/// Attach to the worker for `--rm`, then stop it once the attach ends.
///
/// Unlike a plain attach, this acts like a shell `trap`: it installs handlers
/// for SIGHUP/SIGINT/SIGTERM so that closing the terminal tab running the
/// command (which delivers SIGHUP) still stops the worker, rather than leaving
/// an orphaned container running. The interactive attach blocks, so it runs on
/// a dedicated blocking thread while the async signal handlers race against it
/// via `select!`. Whichever fires first, we fall through to the same cleanup.
async fn attach_with_rm_cleanup(
    process_id: &str,
    worker_prefix: &str,
    port: u16,
    output: &OutputManager,
) -> Result<i32> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sighup = signal(SignalKind::hangup()).context("install SIGHUP handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("install SIGINT handler")?;
    let mut sigterm = signal(SignalKind::terminate()).context("install SIGTERM handler")?;

    let attach_pid = process_id.to_string();
    let attach_prefix = worker_prefix.to_string();
    let attach = tokio::task::spawn_blocking(move || process_attach(&attach_pid, &attach_prefix));

    let exit_code = tokio::select! {
        res = attach => res.context("attach task panicked")??,
        _ = sighup.recv() => {
            eprintln!("\nTerminal closed; stopping worker (--rm)...");
            // 128 + SIGHUP(1)
            129
        }
        _ = sigint.recv() => {
            eprintln!("\nInterrupted; stopping worker (--rm)...");
            // 128 + SIGINT(2)
            130
        }
        _ = sigterm.recv() => {
            eprintln!("\nTerminated; stopping worker (--rm)...");
            // 128 + SIGTERM(15)
            143
        }
    };

    let mut client = connect(port).await?;
    process_stop(&mut client, process_id, output).await?;
    Ok(exit_code)
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
async fn worker_describe(
    client: &mut CoreServiceClient<Channel>,
    worker_id: Option<&str>,
    output: &OutputManager,
) -> Result<()> {
    info!("describing worker");
    let resp = client.worker_list(WorkerListRequest {}).await?;
    let workers = resp.into_inner().workers;

    let filtered: Vec<_> = if let Some(id) = worker_id {
        workers.into_iter().filter(|w| w.worker_id == id).collect()
    } else {
        workers
    };

    if filtered.is_empty() {
        if let Some(id) = worker_id {
            bail!("unknown worker: {id}");
        }
        output.print_text("No running workers.");
        return Ok(());
    }

    output.print_items(&filtered, |workers| {
        let mut out = String::new();
        for (i, w) in workers.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            format_worker_describe(w, &mut out);
        }
        if out.ends_with('\n') {
            out.pop();
        }
        out
    });
    Ok(())
}

fn format_worker_describe(w: &WorkerSummary, out: &mut String) {
    fn dash(s: &str) -> &str {
        if s.is_empty() { "-" } else { s }
    }
    let mut fields: Vec<(&str, &str)> = vec![
        ("Worker", &w.worker_id),
        ("Container", dash(&w.container_status)),
        ("Agent", dash(&w.agent_status)),
        ("Mode", &w.mode),
        ("Directory", dash(&w.directory)),
        ("Workflow", dash(&w.workflow_id)),
        ("Workflow Status", dash(&w.workflow_status)),
    ];

    if !w.pr_url.is_empty() {
        fields.push(("PR", &w.pr_url));
    }
    let stall_display;
    if w.workflow_stalled {
        stall_display = if w.workflow_stall_reason.is_empty() {
            "yes".to_owned()
        } else {
            format!("yes \u{2014} {}", w.workflow_stall_reason)
        };
        fields.push(("Stalled", &stall_display));
    }

    let label_width = fields.iter().map(|(l, _)| l.len()).max().unwrap_or(0);

    for (label, value) in &fields {
        out.push_str(&format!(
            "{:>width$}: {}\n",
            label,
            value,
            width = label_width
        ));
    }
}

/// Cancel any active workflow for a ticket via the CancelWorkflow RPC.
async fn cancel_workflow(port: u16, ticket_id: &str) -> Result<()> {
    let channel = connection::connect(port).await?;
    let mut ticket_client = TicketServiceClient::new(channel);

    ticket_client
        .cancel_workflow(CancelWorkflowRequest {
            ticket_id: ticket_id.to_owned(),
        })
        .await
        .with_status_context("cancel workflow")?;

    info!(ticket_id, "cancelled active workflow");
    Ok(())
}

/// Create a workflow for a ticket with status=awaiting_dispatch via the CreateWorkflow RPC.
async fn dispatch_ticket(port: u16, ticket_id: &str) -> Result<()> {
    let channel = connection::connect(port).await?;
    let mut ticket_client = TicketServiceClient::new(channel);

    ticket_client
        .create_workflow(CreateWorkflowRequest {
            ticket_id: ticket_id.to_owned(),
            status: ur_rpc::lifecycle::AWAITING_DISPATCH.to_owned(),
        })
        .await
        .with_status_context("create workflow for dispatch")?;

    info!(ticket_id, "created workflow with awaiting_dispatch status");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[instrument(skip(client, output, projects), fields(ticket_id, workspace = ?workspace, project_key = ?project_key, mode, skills = ?skills))]
async fn process_launch(
    client: &mut CoreServiceClient<Channel>,
    ticket_id: &str,
    workspace: Option<PathBuf>,
    project_key: &str,
    worker_prefix: &str,
    mode: &str,
    skills: &[String],
    context_repos: &[String],
    dispatch: bool,
    output: &OutputManager,
    projects: &HashMap<String, ur_config::ProjectConfig>,
    agent_type: String,
) -> Result<String> {
    info!(ticket_id, project_key, "launching worker process");

    // Refresh credentials for every known agent and ensure config exists.
    // Re-seed if the file is older than a day so host re-logins propagate
    // without clobbering fresh container-driven token refreshes.
    //
    // A seeding failure warns rather than aborting the launch: this loops every
    // agent in `AgentType::ALL` (mode resolution is server-side, so the CLI
    // cannot know which one this launch will use), and one agent's broken host
    // source — say an empty `~/.codex/auth.json` — must not block a launch that
    // resolves to a different, perfectly healthy agent. Not silent, and not a
    // swallowed error either: the server's `check_credentials_seeded` rejects
    // the launch with an actionable `MissingCredentials` status if the agent it
    // *does* resolve to has nothing seeded.
    if let Err(e) = credential::ensure_credentials_for_all_agents(Duration::from_secs(60 * 60 * 24))
    {
        warn!(ticket_id, error = %e, "credential seeding failed");
        if !output.is_json() {
            eprintln!("Warning: credential seeding failed — {e:#}");
        }
    }
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

    let default_image;
    let image_id = match projects
        .get(project_key)
        .map(|p| p.container.image.as_str())
        .filter(|s| !s.is_empty())
    {
        Some(image) => image,
        None if !workspace_dir.is_empty() => {
            // Workspace mount without a project — send the base image alias;
            // the server resolves it against the launch's agent.
            default_image = *ur_config::IMAGE_ALIASES
                .first()
                .expect("IMAGE_ALIASES must not be empty");
            default_image
        }
        None => {
            return Err(if project_key.is_empty() {
                anyhow::anyhow!(
                    "no project specified — use -p <project> to select a project with a configured container image"
                )
            } else {
                anyhow::anyhow!(
                    "project '{}' has no container image configured (set container.image in ur.toml)",
                    project_key
                )
            });
        }
    };
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
            mode: mode.to_owned(),
            skills: skills.to_vec(),
            project_key: project_key.to_owned(),
            context_repos: context_repos.to_vec(),
            dispatch,
            // Empty means "let the server resolve it" (mode's agent, then
            // the configured top-level default).
            agent_type,
        })
        .await?;

    let inner = resp.into_inner();
    let container_id = inner.container_id;
    // For manual mode the server generates the process_id; fall back to
    // ticket_id (which the CLI supplied) for code/design modes.
    let process_id = if inner.process_id.is_empty() {
        ticket_id.to_string()
    } else {
        inner.process_id
    };
    info!(
        ticket_id,
        container_name, container_id, image_id, "worker process launched"
    );
    if output.is_json() {
        output.print_success(&WorkerLaunched {
            worker_id: process_id.clone(),
            container_id,
        });
    } else {
        println!("Worker {process_id} running (container {container_id})");
    }
    Ok(process_id)
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

#[instrument(skip(command, projects, output), fields(command_name = command_name(&command)))]
async fn handle_worker(
    command: WorkerCommands,
    port: u16,
    worker_prefix: &str,
    projects: &HashMap<String, ur_config::ProjectConfig>,
    output: &OutputManager,
    default_agent: ur_config::AgentType,
) -> Result<()> {
    match command {
        WorkerCommands::List => {
            let mut client = connect(port).await?;
            process_list(&mut client, output).await
        }
        WorkerCommands::Attach { worker_id, rm } => {
            handle_worker_attach(port, worker_prefix, output, &worker_id, rm).await
        }
        WorkerCommands::Kill { worker_id } => {
            input::validate_id(&worker_id, "worker_id")?;
            let mut client = connect(port).await?;
            process_stop(&mut client, &worker_id, output).await
        }
        WorkerCommands::ReseedCredentials { agent } => {
            handle_worker_reseed_credentials(output, agent.as_deref(), default_agent)
        }
        WorkerCommands::SaveCredentials { worker_id, agent } => handle_worker_save_credentials(
            worker_prefix,
            output,
            &worker_id,
            agent.as_deref(),
            default_agent,
        ),
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
            context_repos,
            agent,
        } => {
            handle_worker_launch(
                port,
                worker_prefix,
                projects,
                output,
                ticket_id,
                workspace,
                project,
                attach,
                rm,
                force,
                mode,
                skills,
                dispatch,
                context_repos,
                agent,
            )
            .await
        }
        WorkerCommands::Describe { worker_id } => {
            debug!(worker_id = ?worker_id, "describing worker");
            let mut client = connect(port).await?;
            worker_describe(&mut client, worker_id.as_deref(), output).await
        }
        WorkerCommands::Send {
            worker_id,
            message,
            no_submit,
        } => handle_worker_send(port, output, worker_id, message, !no_submit).await,
        WorkerCommands::Stop { worker_id } => {
            input::validate_id(&worker_id, "worker_id")?;
            let mut client = connect(port).await?;
            process_stop(&mut client, &worker_id, output).await
        }
        WorkerCommands::Dir { worker_id } => handle_worker_dir(port, output, &worker_id).await,
        WorkerCommands::Code { worker_id } => handle_worker_code(port, &worker_id).await,
    }
}

async fn handle_worker_attach(
    port: u16,
    worker_prefix: &str,
    output: &OutputManager,
    worker_id: &str,
    rm: bool,
) -> Result<()> {
    if output.is_json() {
        let err = StructuredError::new(
            ErrorCode::InteractiveNotSupported,
            "attach is an interactive command and cannot produce JSON output",
        );
        output.print_error(&err);
        process::exit(err.code.exit_code());
    }
    let exit_code = process_attach(worker_id, worker_prefix)?;
    if rm {
        println!("Stopping {worker_id} (--rm)...");
        let mut client = connect(port).await?;
        process_stop(&mut client, worker_id, output).await?;
    }
    process::exit(exit_code);
}

/// Resolve an optional `--agent` flag against the configured top-level
/// default. `None` means the flag was omitted; a bad name is a descriptive
/// error naming the flag, not a panic.
fn resolve_agent_flag(
    agent: Option<&str>,
    default_agent: ur_config::AgentType,
) -> Result<ur_config::AgentType> {
    match agent {
        Some(name) => {
            ur_config::AgentType::parse(name).with_context(|| format!("invalid --agent {name:?}"))
        }
        None => Ok(default_agent),
    }
}

/// Validate an optional `--agent` flag for `worker launch`, returning the
/// resolved agent name to send on the wire, or an empty string when omitted.
/// Empty means "let the server resolve it" (the mode's `agent` field, then
/// the configured top-level default) — this CLI has no dependency on
/// `crates/server` to resolve it itself.
fn resolve_launch_agent_flag(agent: Option<&str>) -> Result<String> {
    match agent {
        Some(name) => Ok(ur_config::AgentType::parse(name)
            .with_context(|| format!("invalid --agent {name:?}"))?
            .name()
            .to_owned()),
        None => Ok(String::new()),
    }
}

fn handle_worker_reseed_credentials(
    output: &OutputManager,
    agent: Option<&str>,
    default_agent: ur_config::AgentType,
) -> Result<()> {
    info!(agent = ?agent, "forcing credential re-seed from host");
    let agent = resolve_agent_flag(agent, default_agent)?;
    if agent
        .auth()
        .is_some_and(|auth| auth.source == ur_config::AuthSource::InContainer)
    {
        anyhow::bail!(
            "agent {} manages credentials in-container — launch a worker and complete the sign-in in the pane",
            agent.name()
        );
    }
    let Some(cred_mgr) = credential::credential_manager_for(agent) else {
        anyhow::bail!("agent {} has no credentials to seed", agent.name());
    };
    cred_mgr.ensure_credentials(Duration::ZERO)?;
    let path = cred_mgr.host_credentials_path()?;
    if !ur_config::credentials_file_is_seeded(&path) {
        anyhow::bail!(
            "no host credentials found to seed for agent {} — log in to that agent on this machine first",
            agent.name()
        );
    }
    if output.is_json() {
        output.print_success(&CredentialsSaved {
            paths: vec![path.display().to_string()],
        });
    } else {
        println!("Reseeded {}", path.display());
    }
    Ok(())
}

fn handle_worker_save_credentials(
    worker_prefix: &str,
    output: &OutputManager,
    worker_id: &str,
    agent: Option<&str>,
    default_agent: ur_config::AgentType,
) -> Result<()> {
    input::validate_id(worker_id, "worker_id")?;
    info!(worker_id = %worker_id, agent = ?agent, "saving credentials from container");
    let runtime = container::runtime_from_env();
    let id = container::ContainerId(format!("{worker_prefix}{worker_id}"));
    let agent = resolve_agent_flag(agent, default_agent)?;
    let Some(cred_mgr) = credential::credential_manager_for(agent) else {
        anyhow::bail!("agent {} has no credentials to save", agent.name());
    };
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

async fn handle_worker_send(
    port: u16,
    output: &OutputManager,
    worker_id: String,
    message: String,
    submit: bool,
) -> Result<()> {
    input::validate_id(&worker_id, "worker_id")?;
    let mut client = connect(port).await?;
    info!(worker_id = %worker_id, submit, "sending message to worker");
    client
        .send_worker_message(SendWorkerMessageRequest {
            worker_id: worker_id.clone(),
            message,
            submit: Some(submit),
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

async fn handle_worker_dir(port: u16, output: &OutputManager, worker_id: &str) -> Result<()> {
    input::validate_id(worker_id, "worker_id")?;
    let dir = process_workspace_dir(port, worker_id).await?;
    if output.is_json() {
        output.print_success(&WorkerDir { path: dir });
    } else {
        println!("{dir}");
    }
    Ok(())
}

async fn handle_worker_code(port: u16, worker_id: &str) -> Result<()> {
    input::validate_id(worker_id, "worker_id")?;
    let dir = process_workspace_dir(port, worker_id).await?;
    let status = process::Command::new("code")
        .arg(&dir)
        .status()
        .context("failed to launch VS Code — is `code` on your PATH?")?;
    if !status.success() {
        bail!("VS Code exited with {status}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_worker_launch(
    port: u16,
    worker_prefix: &str,
    projects: &HashMap<String, ur_config::ProjectConfig>,
    output: &OutputManager,
    ticket_id: Option<String>,
    workspace: Option<PathBuf>,
    project: Option<String>,
    attach: bool,
    rm: bool,
    force: bool,
    mode: String,
    skills: Option<String>,
    dispatch: bool,
    context_repos: Option<String>,
    agent: Option<String>,
) -> Result<()> {
    let is_manual = mode == "manual";

    // Manual mode validations
    if is_manual && dispatch {
        bail!("dispatch is not supported for manual mode");
    }
    if is_manual && project.is_none() && workspace.is_none() {
        bail!("manual mode requires -p project or -w workspace");
    }

    // Validate --agent up front so a bad name fails before any RPC.
    let agent_type = resolve_launch_agent_flag(agent.as_deref())?;

    // Non-manual modes require a ticket_id
    let ticket_id_str: String = if is_manual {
        // Manual mode does not use a ticket_id; send empty string to server
        ticket_id.unwrap_or_default()
    } else {
        match ticket_id {
            Some(ref id) => {
                input::validate_id(id, "ticket_id")?;
                id.clone()
            }
            None => bail!("ticket_id is required for mode '{mode}'"),
        }
    };

    if let Some(ref p) = project {
        input::validate_id(p, "project")?;
    }
    if let Some(ref w) = workspace {
        input::reject_path_traversal(w, "workspace")?;
    }

    let skills_vec: Vec<String> = skills
        .iter()
        .flat_map(|s| s.split(',').map(|s| s.trim().to_owned()))
        .filter(|s| !s.is_empty())
        .collect();

    let context_repos_vec: Vec<String> = context_repos
        .iter()
        .flat_map(|s| s.split(',').map(|s| s.trim().to_owned()))
        .filter(|s| !s.is_empty())
        .collect();

    let project_keys: Vec<String> = projects.keys().cloned().collect();
    let resolved_project = resolve_project_key(
        project,
        &workspace,
        &ticket_id_str,
        &project_keys,
        is_manual,
    )?;

    reject_unsupported_local_launch(projects.get(&resolved_project), &workspace, &mode, dispatch)?;

    let mut client = connect(port).await?;
    if force {
        debug!(ticket_id = %ticket_id_str, "force-stopping existing process before launch");
        let _ = process_stop(&mut client, &ticket_id_str, output).await;
        if !is_manual {
            cancel_workflow(port, &ticket_id_str).await?;
        }
    }
    let is_design = mode == "design";
    if dispatch && !is_design {
        dispatch_ticket(port, &ticket_id_str).await?;
    }
    let process_id = process_launch(
        &mut client,
        &ticket_id_str,
        workspace,
        &resolved_project,
        worker_prefix,
        &mode,
        &skills_vec,
        &context_repos_vec,
        dispatch && is_design,
        output,
        projects,
        agent_type,
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
        wait_for_healthy(&process_id, worker_prefix)?;
        if rm {
            let exit_code =
                attach_with_rm_cleanup(&process_id, worker_prefix, port, output).await?;
            process::exit(exit_code);
        }
        let exit_code = process_attach(&process_id, worker_prefix)?;
        process::exit(exit_code);
    }
    Ok(())
}

/// Reject launch combinations that cannot work for a local (repo-less) project.
///
/// A local project has no git remote, so there is no pool to clone into and no
/// branch to push. That rules out three things, each caught here before the
/// launch RPC so the user gets the reason rather than a pool failure:
///
/// - `--dispatch`: dispatch drives the implement → push → PR workflow.
/// - Any mode other than `manual`: code and design modes own a ticket branch.
/// - Omitting `-w`: without a workspace mount there is nothing to put in the
///   container, and the usual pool fallback does not exist.
///
/// `None` for `project` means the key is unconfigured (workspace-only launch),
/// which is unrelated to locality and always allowed.
fn reject_unsupported_local_launch(
    project: Option<&ur_config::ProjectConfig>,
    workspace: &Option<PathBuf>,
    mode: &str,
    dispatch: bool,
) -> Result<()> {
    let Some(project) = project.filter(|p| p.is_local()) else {
        return Ok(());
    };
    let key = &project.key;
    if dispatch {
        bail!(
            "project '{key}' is local (local = true, no repo) — its tickets cannot be \
             dispatched: there is no repo to clone, branch to push, or PR to open. \
             Launch a manual worker instead: ur worker launch -m manual -w . -p {key}"
        );
    }
    if mode != "manual" {
        bail!(
            "project '{key}' is local (local = true, no repo) — mode '{mode}' needs a repo \
             pool and a ticket branch. Only manual mode is supported: \
             ur worker launch -m manual -w . -p {key}"
        );
    }
    if workspace.is_none() {
        bail!(
            "project '{key}' is local (local = true, no repo) — it has no repo pool to fall \
             back on, so a workspace mount is required: \
             ur worker launch -m manual -w . -p {key}"
        );
    }
    Ok(())
}

fn resolve_project_key(
    project: Option<String>,
    workspace: &Option<PathBuf>,
    ticket_id: &str,
    project_keys: &[String],
    is_manual: bool,
) -> Result<String> {
    if let Some(p) = project {
        return Ok(p);
    }
    // Manual mode with no -p or -w: caller already validated this won't happen
    // (workspace is also None here only when -p was required and missing).
    // When ticket_id is empty (manual mode), skip prefix-based derivation.
    let id_prefix = if ticket_id.is_empty() {
        String::new()
    } else {
        ticket_id
            .split(&['-', '.'][..])
            .next()
            .unwrap_or("")
            .to_owned()
    };
    if !id_prefix.is_empty() && project_keys.contains(&id_prefix) {
        debug!(project_key = %id_prefix, "derived project from ticket ID prefix");
        return Ok(id_prefix);
    }
    let cwd = std::env::current_dir().context("failed to get current working directory")?;
    let dir_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("cannot determine directory name from cwd"))?
        .to_owned();
    if project_keys.contains(&dir_name) {
        debug!(project_key = %dir_name, "derived project from cwd");
        return Ok(dir_name);
    }
    if workspace.is_some() || is_manual {
        return Ok(String::new());
    }
    bail!(
        "could not derive project from ticket ID prefix '{}' or \
         cwd directory name '{}' \
         (neither is a configured project key). Use -p <project> or -w <path>.",
        id_prefix,
        dir_name
    )
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
        WorkerCommands::ReseedCredentials { .. } => "reseed_credentials",
        WorkerCommands::SaveCredentials { .. } => "save_credentials",
        WorkerCommands::Send { .. } => "send",
        WorkerCommands::Launch { .. } => "launch",
        WorkerCommands::Describe { .. } => "describe",
        WorkerCommands::Stop { .. } => "stop",
        WorkerCommands::Dir { .. } => "dir",
        WorkerCommands::Code { .. } => "code",
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

async fn handle_project(
    command: ProjectCommands,
    config: &ur_config::Config,
    output: &OutputManager,
) -> Result<()> {
    match command {
        ProjectCommands::Add {
            path,
            image,
            key,
            name,
            pool_limit,
            local,
        } => {
            if let Some(ref k) = key {
                input::validate_id(k, "key")?;
            }
            if let Some(ref n) = name {
                input::reject_control_chars(n, "name")?;
            }
            input::reject_path_traversal(&path, "path")?;
            let image_resolved: String =
                image.unwrap_or_else(|| ur_config::default_image_alias().to_string());
            ur_config::validate_image_alias(&image_resolved)?;
            let resolved_key = resolve_add_project_key(&path, key.as_deref(), local)?;
            project::add(
                config,
                &project::AddRequest {
                    path: &path,
                    image: &image_resolved,
                    key: key.as_deref(),
                    name: name.as_deref(),
                    pool_limit,
                    local,
                },
                output,
            )?;
            project::try_reload_server(
                config.server_port,
                &config.config_dir,
                &resolved_key,
                "added",
            )
            .await;
        }
        ProjectCommands::List => project::list(config, output)?,
        ProjectCommands::Remove { key, force } => {
            input::validate_id(&key, "key")?;
            project::remove(config, &key, force, output)?;
            project::try_reload_server(config.server_port, &config.config_dir, &key, "removed")
                .await;
        }
    }
    Ok(())
}

/// Resolve the project key that `project::add` will use, without side effects.
/// Used for the post-add server reload.
///
/// A local project has no remote to derive from, so its key comes from the
/// directory basename — matching `project::add`.
fn resolve_add_project_key(path: &Path, explicit_key: Option<&str>, local: bool) -> Result<String> {
    match explicit_key {
        Some(k) => Ok(k.to_string()),
        None => {
            let canonical =
                std::fs::canonicalize(path).context("failed to resolve path for key derivation")?;
            if local {
                return project::derive_key_from_path(&canonical);
            }
            let repo = project::git_remote_origin(&canonical)?;
            project::derive_key_from_repo(&repo)
        }
    }
}

fn handle_proxy(
    command: ProxyCommands,
    config: &ur_config::Config,
    output: &OutputManager,
) -> Result<()> {
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
    Ok(())
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
    let _log_guard = logging::init(&config.logs_dir);

    info!(
        config_dir = %config.config_dir.display(),
        server_port = config.server_port,
        builderd_port = config.builderd_port,
        "ur CLI started"
    );

    let port = config.server_port;
    let compose = compose_manager_from_config(&config);

    match cli.command {
        Commands::Builder { command } => builder::handle(command, config.builderd_port).await?,
        Commands::Db { command } => match command {
            DbCommands::Backup => db::backup(&config, output).await?,
            DbCommands::List => db::list(&config, output)?,
            DbCommands::Restore { path } => {
                input::reject_path_traversal(&path, "path")?;
                db::restore(&config, &path, output).await?
            }
        },
        Commands::Init { .. } => unreachable!(),
        Commands::Project { command } => handle_project(command, &config, output).await?,
        Commands::Proxy { command } => handle_proxy(command, &config, output)?,
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
        Commands::Ticket { command } => {
            ticket::handle(port, command, output, &config.projects).await?
        }
        Commands::Tui { command } => tui::handle(*command, &config, output)?,
        Commands::Flow { command } => flow::handle(port, command, output).await?,
        Commands::Worker { command } => {
            handle_worker(
                command,
                port,
                &config.network.worker_prefix,
                &config.projects,
                output,
                config.agent,
            )
            .await?
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project_keys() -> Vec<String> {
        vec!["ur".to_owned(), "myproj".to_owned()]
    }

    /// A project config with `repo` set or cleared, for the locality guards.
    fn make_project(key: &str, repo: Option<&str>) -> ur_config::ProjectConfig {
        ur_config::ProjectConfig {
            key: key.to_owned(),
            repo: repo.map(str::to_owned),
            name: key.to_owned(),
            pool_limit: 10,
            hostexec: vec![],
            instruction_md: None,
            container: ur_config::ContainerConfig {
                image: "ur-worker".to_owned(),
                mounts: vec![],
                ports: vec![],
            },
            max_fix_attempts: 5,
            max_implement_cycles: None,
            protected_branches: vec![],
            tui: None,
            ignored_workflow_checks: vec![],
            hostexec_scripts: vec![],
            push_again_exit_code: ur_config::DEFAULT_PUSH_AGAIN_EXIT_CODE,
            memory_dir: None,
            brain_dir: None,
        }
    }

    // ── reject_unsupported_local_launch ────────────────────────────────

    #[test]
    fn local_launch_manual_with_workspace_is_allowed() {
        let proj = make_project("myapp", None);
        reject_unsupported_local_launch(
            Some(&proj),
            &Some(PathBuf::from("/Users/me/myapp")),
            "manual",
            false,
        )
        .unwrap();
    }

    #[test]
    fn local_launch_with_dispatch_is_rejected() {
        let proj = make_project("myapp", None);
        let err = reject_unsupported_local_launch(
            Some(&proj),
            &Some(PathBuf::from("/Users/me/myapp")),
            "manual",
            true,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("cannot be dispatched"), "unexpected: {err}");
        assert!(err.contains("myapp"), "unexpected: {err}");
    }

    #[test]
    fn local_launch_non_manual_mode_is_rejected() {
        let proj = make_project("myapp", None);
        for mode in ["code", "design"] {
            let err = reject_unsupported_local_launch(
                Some(&proj),
                &Some(PathBuf::from("/Users/me/myapp")),
                mode,
                false,
            )
            .unwrap_err()
            .to_string();
            assert!(err.contains(mode), "unexpected for {mode}: {err}");
        }
    }

    #[test]
    fn local_launch_without_workspace_is_rejected() {
        let proj = make_project("myapp", None);
        let err = reject_unsupported_local_launch(Some(&proj), &None, "manual", false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("workspace mount"), "unexpected: {err}");
    }

    #[test]
    fn repo_backed_project_is_unaffected_by_local_guards() {
        let proj = make_project("ur", Some("git@github.com:cmaher/ur.git"));
        // Every combination the guard would reject for a local project is fine here.
        reject_unsupported_local_launch(Some(&proj), &None, "code", true).unwrap();
        reject_unsupported_local_launch(Some(&proj), &None, "design", false).unwrap();
        reject_unsupported_local_launch(Some(&proj), &None, "manual", false).unwrap();
    }

    #[test]
    fn unconfigured_project_is_unaffected_by_local_guards() {
        // A workspace-only launch (`-w` with no configured project) is not local.
        reject_unsupported_local_launch(None, &Some(PathBuf::from("/tmp/x")), "manual", false)
            .unwrap();
        reject_unsupported_local_launch(None, &None, "code", true).unwrap();
    }

    #[test]
    fn explicit_project_flag_takes_priority() {
        let result = resolve_project_key(
            Some("explicit".to_owned()),
            &None,
            "ur-abc12",
            &project_keys(),
            false,
        )
        .unwrap();
        assert_eq!(result, "explicit");
    }

    #[test]
    fn explicit_project_flag_with_workspace() {
        let ws = Some(PathBuf::from("/tmp/ws"));
        let result = resolve_project_key(
            Some("explicit".to_owned()),
            &ws,
            "ur-abc12",
            &project_keys(),
            false,
        )
        .unwrap();
        assert_eq!(result, "explicit");
    }

    #[test]
    fn ticket_id_prefix_derivation() {
        let result = resolve_project_key(None, &None, "ur-abc12", &project_keys(), false).unwrap();
        assert_eq!(result, "ur");
    }

    #[test]
    fn ticket_id_prefix_derivation_with_workspace() {
        let ws = Some(PathBuf::from("/tmp/ws"));
        let result = resolve_project_key(None, &ws, "ur-abc12", &project_keys(), false).unwrap();
        assert_eq!(result, "ur");
    }

    #[test]
    fn ticket_id_prefix_dot_separator() {
        let result =
            resolve_project_key(None, &None, "myproj.xyz", &project_keys(), false).unwrap();
        assert_eq!(result, "myproj");
    }

    #[test]
    fn ticket_id_prefix_not_in_keys_falls_through() {
        // Prefix doesn't match a project key; with workspace set and cwd not matching,
        // falls through to the workspace fallback returning empty string.
        let ws = Some(PathBuf::from("/tmp/ws"));
        let result = resolve_project_key(None, &ws, "zzz-abc", &project_keys(), false);
        if let Ok(val) = result {
            assert_ne!(val, "zzz");
        }
    }

    #[test]
    fn workspace_fallback_returns_empty_when_nothing_matches() {
        // Empty ticket ID prefix + workspace set: should return Ok("") fallback
        // (unless cwd happens to match a project key, which is also fine).
        let ws = Some(PathBuf::from("/tmp/ws"));
        let result = resolve_project_key(None, &ws, "", &project_keys(), false);
        match result {
            Ok(val) => assert!(val.is_empty() || project_keys().contains(&val)),
            Err(_) => panic!("should not error when workspace is set"),
        }
    }

    #[test]
    fn no_workspace_no_match_returns_error() {
        // No project flag, non-matching prefix, no workspace, empty project keys
        // so cwd can't match either → should error.
        let result = resolve_project_key(None, &None, "zzz-abc", &[], false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("could not derive project"));
    }

    // ── --agent flag on credential commands ────────────────────────────

    fn text_output() -> OutputManager {
        OutputManager::from_args(Some("text"))
    }

    #[test]
    fn reseed_credentials_unknown_agent_errors_without_panicking() {
        let result = handle_worker_reseed_credentials(
            &text_output(),
            Some("bogus-agent"),
            ur_config::AgentType::Claude,
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("bogus-agent"), "{msg}");
    }

    #[test]
    fn save_credentials_unknown_agent_errors_without_panicking() {
        let result = handle_worker_save_credentials(
            "ur-worker-",
            &text_output(),
            "w1",
            Some("bogus-agent"),
            ur_config::AgentType::Claude,
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("bogus-agent"), "{msg}");
    }

    #[test]
    fn reseed_credentials_rejects_in_container_auth() {
        let result = handle_worker_reseed_credentials(
            &text_output(),
            Some("agy"),
            ur_config::AgentType::Claude,
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("sign-in in the pane"), "{msg}");
    }

    #[test]
    fn save_credentials_rejects_in_container_auth() {
        let result = handle_worker_save_credentials(
            "ur-worker-",
            &text_output(),
            "missing-worker",
            Some("agy"),
            ur_config::AgentType::Claude,
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("already bind-mounted"), "{msg}");
    }

    /// The `--agent` flag on the credential subcommands has no clap
    /// `default_value` any more — omitting it must leave `None` so the
    /// caller applies the configured top-level default, rather than baking
    /// a literal into the CLI definition.
    #[test]
    fn agent_flag_omitted_is_none_on_credential_commands() {
        let cli = Cli::try_parse_from(["ur", "worker", "reseed-credentials"]).unwrap();
        let Commands::Worker {
            command: WorkerCommands::ReseedCredentials { agent },
        } = cli.command
        else {
            panic!("expected worker reseed-credentials");
        };
        assert_eq!(agent, None);
    }

    /// `resolve_agent_flag` must always produce a value `AgentType::parse`
    /// already accepts (it *is* an `AgentType`) — pins the omitted-flag path
    /// for every known agent, so a new agent variant is covered automatically.
    #[test]
    fn resolve_agent_flag_omitted_uses_configured_default() {
        for default_agent in ur_config::AgentType::ALL {
            assert_eq!(
                resolve_agent_flag(None, *default_agent).unwrap(),
                *default_agent
            );
        }
    }

    #[test]
    fn resolve_agent_flag_present_overrides_default() {
        assert_eq!(
            resolve_agent_flag(Some("codex"), ur_config::AgentType::Claude).unwrap(),
            ur_config::AgentType::Codex
        );
    }

    #[test]
    fn resolve_agent_flag_unknown_errors_without_panicking() {
        let err = resolve_agent_flag(Some("bogus"), ur_config::AgentType::Claude).unwrap_err();
        assert!(err.to_string().contains("bogus"));
    }

    // ── --agent flag on worker launch ──────────────────────────────────

    #[test]
    fn launch_parses_agent_flag() {
        let cli =
            Cli::try_parse_from(["ur", "worker", "launch", "ur-x", "--agent", "codex"]).unwrap();
        let Commands::Worker {
            command: WorkerCommands::Launch { agent, .. },
        } = cli.command
        else {
            panic!("expected worker launch");
        };
        assert_eq!(agent, Some("codex".to_string()));
    }

    #[test]
    fn launch_agent_flag_omitted_is_empty() {
        assert_eq!(resolve_launch_agent_flag(None).unwrap(), "");
    }

    #[test]
    fn launch_agent_flag_valid_resolves_name() {
        assert_eq!(resolve_launch_agent_flag(Some("codex")).unwrap(), "codex");
    }

    #[test]
    fn launch_agent_flag_unknown_errors_without_panicking() {
        let err = resolve_launch_agent_flag(Some("bogus")).unwrap_err();
        assert!(err.to_string().contains("bogus"));
    }
}
