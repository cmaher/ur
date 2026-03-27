use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tonic::transport::Endpoint;
use tracing::{debug, error, info, warn};

use ur_rpc::proto::core::WorkerStopRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::hostexec::HostExecCommandEntry;
use ur_rpc::proto::hostexec::ListHostExecCommandsRequest;
use ur_rpc::proto::hostexec::host_exec_service_client::HostExecServiceClient;
use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonServiceServer;

mod grpc_service;
mod init_git_hooks;
mod init_skill_hooks;
mod init_skills;
mod logging;

const SHIM_DIR: &str = ".local/bin";
const MAX_RETRIES: u32 = 30;
const INITIAL_BACKOFF_MS: u64 = 500;
const MAX_BACKOFF_MS: u64 = 5000;
const WORKERD_GRPC_PORT: u16 = 9120;
const EXIT_WATCHER_POLL_SECS: u64 = 5;

#[derive(Parser)]
#[command(name = "workerd", about = "Ur worker daemon")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run all container initialization: skills, git hooks, hostexec shims
    Init,
    /// Run the daemon without running init first (caller must run `workerd init` beforehand)
    Daemon,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _log_guard = logging::init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init) => run_init().await,
        Some(Commands::Daemon) => run_daemon_only().await,
        None => run_daemon().await,
    }
}

/// Synchronous initialization: skills, git hooks, and hostexec shim creation.
async fn run_init() -> Result<()> {
    info!("workerd init starting");

    // Initialize skills
    let skills_manager = init_skills::InitSkillsManager::from_env();
    let exit_code = skills_manager.run().await;
    if exit_code != 0 {
        anyhow::bail!("skills initialization failed");
    }

    // Initialize git hooks
    let git_hooks_manager = init_git_hooks::InitGitHooksManager;
    git_hooks_manager
        .run()
        .await
        .context("git hooks initialization failed")?;

    // Initialize skill hooks
    let skill_hooks_manager = init_skill_hooks::InitSkillHooksManager;
    skill_hooks_manager
        .run()
        .await
        .context("skill hooks initialization failed")?;

    // Create hostexec shims
    let shim_dir = resolve_shim_dir();
    info!(shim_dir = %shim_dir.display(), "resolved shim directory");

    tokio::fs::create_dir_all(&shim_dir)
        .await
        .with_context(|| format!("creating shim dir {}", shim_dir.display()))?;

    let entries = fetch_commands_with_retry().await?;

    for entry in &entries {
        create_shim(&shim_dir, entry).await?;
    }

    let command_names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    info!(count = entries.len(), ?command_names, "init complete");
    Ok(())
}

/// Background daemon: runs init, creates tmux session, launches Claude Code, serves healthz + gRPC.
async fn run_daemon() -> Result<()> {
    info!("workerd daemon starting");

    // 0. Run initialization (skills, git hooks, hostexec shims)
    run_init().await.context("init phase failed")?;

    run_daemon_only().await
}

/// Daemon without init — expects `workerd init` to have been called already.
/// Used by image-specific entrypoints that need to launch background processes between init and daemon.
async fn run_daemon_only() -> Result<()> {
    // 1. Create tmux session `agent` (220x55)
    let session = tmux::Session::create(tmux::CreateOptions {
        name: "agent".into(),
        width: Some(220),
        height: Some(55),
        detached: true,
    })
    .await?;

    // 2. Set tmux status line with worker ID
    let worker_id = std::env::var(ur_config::UR_WORKER_ID_ENV).unwrap_or_else(|_| "unknown".into());
    let status_left = format!("[{worker_id}] ");
    session.set_status_left(&status_left).await?;

    // 3. Launch Claude Code via send-keys
    session.send_keys("claude").await?;
    info!("claude launched in tmux session");

    // 3b. Spawn exit watcher that polls tmux pane and triggers shutdown when Claude exits
    {
        let server_addr = std::env::var(ur_config::UR_SERVER_ADDR_ENV)
            .unwrap_or_else(|_| "localhost:50051".into());
        let worker_id =
            std::env::var(ur_config::UR_WORKER_ID_ENV).unwrap_or_else(|_| "unknown".into());
        let worker_secret =
            std::env::var(ur_config::UR_WORKER_SECRET_ENV).unwrap_or_else(|_| String::new());
        tokio::spawn(run_exit_watcher(server_addr, worker_id, worker_secret));
    }

    // 4. Spawn healthz HTTP server (port 9119) in background
    tokio::spawn(serve_healthz());

    // 5. Start gRPC server on port 9120 (this is the long-lived process)
    let addr: SocketAddr = ([0, 0, 0, 0], WORKERD_GRPC_PORT).into();
    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).unwrap_or_else(|_| "localhost:50051".into());
    let worker_id = std::env::var(ur_config::UR_WORKER_ID_ENV).unwrap_or_else(|_| "unknown".into());
    let worker_secret =
        std::env::var(ur_config::UR_WORKER_SECRET_ENV).unwrap_or_else(|_| String::new());
    let service = grpc_service::WorkerDaemonServiceImpl {
        server_addr,
        worker_id,
        worker_secret,
        dispatch_buffer: std::sync::Arc::new(tokio::sync::Mutex::new(
            grpc_service::DispatchBuffer {
                commands: std::collections::VecDeque::new(),
                step_complete: false,
                lifecycle_step: String::new(),
            },
        )),
    };
    info!(port = WORKERD_GRPC_PORT, "starting gRPC server");

    tonic::transport::Server::builder()
        .add_service(WorkerDaemonServiceServer::new(service))
        .serve(addr)
        .await
        .context("gRPC server exited")?;

    Ok(())
}

/// Poll the tmux agent session and initiate container shutdown when Claude Code exits.
async fn run_exit_watcher(server_addr: String, worker_id: String, worker_secret: String) {
    let session = tmux::Session::agent();
    let interval = Duration::from_secs(EXIT_WATCHER_POLL_SECS);
    let is_design_worker = std::env::var("UR_WORKER_CLAUDE").unwrap_or_default() == "design";

    info!(
        is_design_worker,
        "exit watcher started, polling every {EXIT_WATCHER_POLL_SECS}s"
    );

    loop {
        tokio::time::sleep(interval).await;

        let should_stop = match session.is_pane_alive().await {
            Ok(true) => {
                debug!("agent pane is alive");
                // Pane is alive — for design workers, also check if claude process exited
                if is_design_worker && !is_claude_process_running().await {
                    info!("claude process exited in design worker, initiating shutdown");
                    true
                } else {
                    false
                }
            }
            Ok(false) => {
                info!("agent pane is dead, initiating shutdown");
                true
            }
            Err(e) => {
                warn!(error = %e, "failed to check pane status, treating as dead");
                true
            }
        };

        if !should_stop {
            continue;
        }

        // Pane is dead, claude exited (design), or check failed — send WorkerStop RPC
        send_stop_and_wait(&server_addr, &worker_id, &worker_secret).await;
    }
}

/// Send WorkerStop RPC and wait for the server to kill the container.
async fn send_stop_and_wait(server_addr: &str, worker_id: &str, worker_secret: &str) {
    info!(worker_id, server_addr, "sending WorkerStop RPC");
    match self_stop_rpc(server_addr, worker_id, worker_secret).await {
        Ok(()) => {
            info!("WorkerStop RPC succeeded, waiting for server to stop container");
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }
        Err(e) => {
            error!(error = %e, "WorkerStop RPC failed, falling back to process::exit(0)");
            std::process::exit(0);
        }
    }
}

/// Check if a `claude` process is currently running using `pgrep`.
/// Returns `true` if at least one claude process is found, `false` otherwise.
async fn is_claude_process_running() -> bool {
    match tokio::process::Command::new("pgrep")
        .args(["-x", "claude"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
    {
        Ok(status) => {
            let running = status.success();
            debug!(running, "claude process check");
            running
        }
        Err(e) => {
            warn!(error = %e, "failed to run pgrep, assuming claude is running");
            true
        }
    }
}

/// Send a one-shot WorkerStop RPC to the ur-server to request container shutdown.
async fn self_stop_rpc(server_addr: &str, worker_id: &str, worker_secret: &str) -> Result<()> {
    let addr = format!("http://{server_addr}");
    let channel = Endpoint::try_from(addr)?.connect().await?;
    let mut client = CoreServiceClient::new(channel);

    let mut request = tonic::Request::new(WorkerStopRequest {
        worker_id: worker_id.to_owned(),
    });

    if let Ok(val) = worker_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>() {
        request
            .metadata_mut()
            .insert(ur_config::WORKER_ID_HEADER, val);
    }
    if let Ok(val) = worker_secret.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::WORKER_SECRET_HEADER, val);
    }

    client.worker_stop(request).await?;
    Ok(())
}

/// Serve /healthz on port 9119 for Docker HEALTHCHECK.
async fn serve_healthz() -> Result<()> {
    let listener =
        tokio::net::TcpListener::bind(("0.0.0.0", ur_config::WORKERD_HEALTHZ_PORT)).await?;
    info!(
        port = ur_config::WORKERD_HEALTHZ_PORT,
        "healthz endpoint ready"
    );

    loop {
        let (mut stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf).await;
            let body = "ok";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        });
    }
}

fn resolve_shim_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ur_config::WORKER_HOME.into());
    PathBuf::from(home).join(SHIM_DIR)
}

async fn fetch_commands_with_retry() -> Result<Vec<HostExecCommandEntry>> {
    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).context("UR_SERVER_ADDR must be set")?;
    let addr = format!("http://{server_addr}");

    info!(server_addr = %addr, "fetching host-exec commands");

    let mut backoff_ms = INITIAL_BACKOFF_MS;

    for attempt in 1..=MAX_RETRIES {
        debug!(attempt, max_retries = MAX_RETRIES, "fetch attempt");
        match try_fetch_commands(&addr).await {
            Ok(commands) => {
                info!(
                    attempt,
                    count = commands.len(),
                    "successfully fetched commands"
                );
                return Ok(commands);
            }
            Err(e) => {
                warn!(
                    attempt,
                    max_retries = MAX_RETRIES,
                    backoff_ms,
                    error = %e,
                    "failed to fetch commands"
                );
                if attempt == MAX_RETRIES {
                    return Err(e).context("exhausted retries fetching command list");
                }
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            }
        }
    }

    unreachable!()
}

async fn try_fetch_commands(addr: &str) -> Result<Vec<HostExecCommandEntry>> {
    let channel = Endpoint::try_from(addr.to_string())?.connect().await?;
    let mut client = HostExecServiceClient::new(channel);

    let mut request = tonic::Request::new(ListHostExecCommandsRequest {});

    // Inject worker ID and secret metadata headers if available
    if let Ok(worker_id) = std::env::var(ur_config::UR_WORKER_ID_ENV)
        && let Ok(val) = worker_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::WORKER_ID_HEADER, val);
    }
    if let Ok(worker_secret) = std::env::var(ur_config::UR_WORKER_SECRET_ENV)
        && let Ok(val) =
            worker_secret.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::WORKER_SECRET_HEADER, val);
    }

    let resp = client.list_commands(request).await?;
    Ok(resp.into_inner().entries)
}

async fn create_shim(shim_dir: &Path, entry: &HostExecCommandEntry) -> Result<()> {
    let command = &entry.name;
    let shim_path = shim_dir.join(command);
    let bidi_flag = if entry.bidi { " --bidi" } else { "" };
    let content = format!("#!/bin/sh\nexec workertools host-exec{bidi_flag} {command} \"$@\"\n");

    debug!(command, bidi = entry.bidi, path = %shim_path.display(), "writing shim");

    tokio::fs::write(&shim_path, &content)
        .await
        .with_context(|| format!("writing shim {}", shim_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(&shim_path, perms)
            .await
            .with_context(|| format!("chmod shim {}", shim_path.display()))?;
    }

    info!(command, path = %shim_path.display(), "shim created");
    Ok(())
}
