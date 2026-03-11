use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use tonic::transport::Endpoint;
use tracing::{debug, info, warn};

use ur_rpc::proto::hostexec::ListHostExecCommandsRequest;
use ur_rpc::proto::hostexec::host_exec_service_client::HostExecServiceClient;

mod logging;

const SHIM_DIR: &str = ".local/bin";
const MAX_RETRIES: u32 = 30;
const INITIAL_BACKOFF_MS: u64 = 500;
const MAX_BACKOFF_MS: u64 = 5000;

#[tokio::main]
async fn main() -> Result<()> {
    logging::init();

    info!("ur-workerd starting");

    let shim_dir = resolve_shim_dir();
    info!(shim_dir = %shim_dir.display(), "resolved shim directory");

    tokio::fs::create_dir_all(&shim_dir)
        .await
        .with_context(|| format!("creating shim dir {}", shim_dir.display()))?;

    let commands = fetch_commands_with_retry().await?;

    for command in &commands {
        create_shim(&shim_dir, command).await?;
    }

    info!(count = commands.len(), ?commands, "all shims created");

    // Stay alive for future daemon uses
    info!("entering daemon loop");
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

fn resolve_shim_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ur_config::WORKER_HOME.into());
    PathBuf::from(home).join(SHIM_DIR)
}

async fn fetch_commands_with_retry() -> Result<Vec<String>> {
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

async fn try_fetch_commands(addr: &str) -> Result<Vec<String>> {
    let channel = Endpoint::try_from(addr.to_string())?.connect().await?;
    let mut client = HostExecServiceClient::new(channel);

    let mut request = tonic::Request::new(ListHostExecCommandsRequest {});

    // Inject agent ID metadata header if available
    if let Ok(agent_id) = std::env::var(ur_config::UR_AGENT_ID_ENV)
        && let Ok(val) = agent_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::AGENT_ID_HEADER, val);
    }

    let resp = client.list_commands(request).await?;
    Ok(resp.into_inner().commands)
}

async fn create_shim(shim_dir: &Path, command: &str) -> Result<()> {
    let shim_path = shim_dir.join(command);
    let content = format!("#!/bin/sh\nexec ur-tools host-exec {command} \"$@\"\n");

    debug!(command, path = %shim_path.display(), "writing shim");

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
