use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tonic::transport::Endpoint;
use tracing::{debug, info, warn};

use ur_rpc::proto::hostexec::HostExecCommandEntry;
use ur_rpc::proto::hostexec::ListHostExecCommandsRequest;
use ur_rpc::proto::hostexec::host_exec_service_client::HostExecServiceClient;
use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonServiceServer;

use workerd::{
    ContainerExitSink, ExitWatcherManager, InitGitHooksManager, InitInstructionsManager,
    InitSettingsManager, InitSkillHooksManager, InitSkillsManager, SkillLayout,
    StartupHooksManager, WorkerDaemonServiceImpl, WorkerIdentity, WorkerInitInputs, WorkerPaths,
};

mod logging;

const MAX_RETRIES: u32 = 30;
const INITIAL_BACKOFF_MS: u64 = 500;
const MAX_BACKOFF_MS: u64 = 5000;
const WORKERD_GRPC_PORT: u16 = 9120;
const EXIT_WATCHER_POLL_SECS: u64 = 5;
const AGENT_SESSION_NAME: &str = "agent";
const DESIGN_STRATEGY: &str = "design";

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

    // Resolve the agent, container paths and per-worker inputs once and
    // inject them everywhere, so init and daemon agree on one agent rather
    // than each component re-reading the environment for itself.
    let agent = ur_config::AgentType::from_env();
    let paths = WorkerPaths::container(agent);
    let inputs = WorkerInitInputs::from_env(agent);

    match cli.command {
        Some(Commands::Init) => run_init(agent, &paths, &inputs).await,
        Some(Commands::Daemon) => run_daemon_only(agent, &inputs).await,
        None => run_daemon(agent, &paths, &inputs).await,
    }
}

/// Synchronous initialization: skills, git hooks, and hostexec shim creation.
async fn run_init(
    agent: ur_config::AgentType,
    paths: &WorkerPaths,
    inputs: &WorkerInitInputs,
) -> Result<()> {
    info!(agent = agent.name(), "workerd init starting");

    InitSkillsManager::new(paths.clone(), SkillLayout::AgentHome)
        .run(&inputs.skills)
        .await
        .context("skills initialization failed")?;

    InitInstructionsManager::new(paths.clone(), SkillLayout::AgentHome)
        .run(&inputs.instructions)
        .await
        .context("instructions initialization failed")?;

    InitSettingsManager::new(paths.clone())
        .run()
        .await
        .context("settings initialization failed")?;

    InitGitHooksManager::new(paths.clone())
        .run()
        .await
        .context("git hooks initialization failed")?;

    InitSkillHooksManager::new(paths.clone())
        .run()
        .await
        .context("skill hooks initialization failed")?;

    // Create hostexec shims
    let shim_dir = &paths.shim_dir;
    info!(shim_dir = %shim_dir.display(), "resolved shim directory");

    tokio::fs::create_dir_all(shim_dir)
        .await
        .with_context(|| format!("creating shim dir {}", shim_dir.display()))?;

    let entries = fetch_commands_with_retry().await?;

    for entry in &entries {
        create_shim(shim_dir, entry).await?;
    }

    let command_names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    info!(count = entries.len(), ?command_names, "init complete");
    Ok(())
}

/// Background daemon: runs init, creates tmux session, launches the agent, serves healthz + gRPC.
async fn run_daemon(
    agent: ur_config::AgentType,
    paths: &WorkerPaths,
    inputs: &WorkerInitInputs,
) -> Result<()> {
    info!("workerd daemon starting");

    // 0. Run initialization (skills, git hooks, hostexec shims)
    run_init(agent, paths, inputs)
        .await
        .context("init phase failed")?;

    let spawned_hooks = StartupHooksManager::new(paths.clone())
        .run()
        .await
        .context("startup hooks phase failed")?;
    for hook in spawned_hooks {
        info!(hook = %hook.name, pid = hook.pid, "spawned background startup hook");
    }

    run_daemon_only(agent, inputs).await
}

/// Worker identity from the container environment.
fn identity_from_env() -> WorkerIdentity {
    WorkerIdentity {
        server_addr: std::env::var(ur_config::UR_SERVER_ADDR_ENV)
            .unwrap_or_else(|_| "localhost:50051".into()),
        worker_id: std::env::var(ur_config::UR_WORKER_ID_ENV).unwrap_or_else(|_| "unknown".into()),
        worker_secret: std::env::var(ur_config::UR_WORKER_SECRET_ENV).unwrap_or_default(),
    }
}

/// Daemon without init — expects `workerd init` to have been called already.
async fn run_daemon_only(agent: ur_config::AgentType, inputs: &WorkerInitInputs) -> Result<()> {
    // 0. Bind the gRPC listener before launching the agent. The agent's
    // SessionStart hook calls NotifyIdle on this port as soon as it boots;
    // binding first lets that connection queue in the backlog instead of
    // being refused, which would drop the idle signal that triggers dispatch.
    let addr: SocketAddr = ([0, 0, 0, 0], WORKERD_GRPC_PORT).into();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding gRPC listener on {addr}"))?;

    let identity = identity_from_env();

    // 1. Create tmux session `agent` (220x55)
    let session = tmux::Session::create(tmux::CreateOptions {
        name: AGENT_SESSION_NAME.into(),
        socket: None,
        start_dir: None,
        env: Vec::new(),
        width: Some(220),
        height: Some(55),
        detached: true,
    })
    .await?;

    // 2. Set tmux status line with worker ID
    let status_left = format!("[{}] ", identity.worker_id);
    session.set_option("status-left-length", "50").await?;
    session.set_status_left(&status_left).await?;

    // 3. Launch the agent via send-keys. Pass `--model <name>` when
    // UR_WORKER_MODEL is set so the model survives Claude Code's own
    // settings.json rewrites on startup (settings.json injection is
    // unreliable — Claude Code recomposes the file from .claude.json and
    // drops unrecognized keys).
    let model = std::env::var(ur_config::UR_WORKER_MODEL_ENV).ok();
    let effort = std::env::var(ur_config::UR_WORKER_EFFORT_ENV)
        .unwrap_or_else(|_| agent.default_effort().to_owned());
    let spawn_cmd = agent.spawn_command(model.as_deref(), &effort);
    session.send_keys(&spawn_cmd).await?;
    info!(cmd = %spawn_cmd, agent = agent.name(), "agent launched in tmux session");

    // 3b. Watch for the agent exiting; the container sink stops the container.
    let is_design_worker = inputs.instructions.strategy.as_deref() == Some(DESIGN_STRATEGY);
    let exit_watcher = ExitWatcherManager::new(
        Arc::new(session.clone()),
        Arc::new(ContainerExitSink::new(identity.clone())),
        is_design_worker,
        Duration::from_secs(EXIT_WATCHER_POLL_SECS),
    )
    .spawn();

    let service = WorkerDaemonServiceImpl::new(identity, agent, SkillLayout::AgentHome, session);

    // 4. Spawn healthz HTTP server (port 9119) in background
    tokio::spawn(serve_healthz());

    // 5. Serve gRPC on the listener bound in step 0 (this is the long-lived
    // process). The container sink never returns, so the watcher arm only
    // fires if it surfaces an error.
    info!(port = WORKERD_GRPC_PORT, "starting gRPC server");

    let server = tonic::transport::Server::builder()
        .add_service(WorkerDaemonServiceServer::new(service))
        .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener));

    tokio::select! {
        result = server => result.context("gRPC server exited")?,
        result = exit_watcher => {
            let reason = result.context("exit watcher task failed")??;
            info!(?reason, "agent exit handled");
        }
    }

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
    let content = format!("#!/bin/sh\nexec ur-workertools host-exec{bidi_flag} {command} \"$@\"\n");

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
