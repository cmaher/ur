mod claude;
mod codex;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use container::{ContainerId, ContainerRuntime, ExecOpts};
use tracing::{debug, info, instrument};
use ur_config::AgentType;

pub use claude::ClaudeCredentialManager;
pub use codex::CodexCredentialManager;

pub(crate) fn worker_home() -> &'static Path {
    Path::new(ur_config::WORKER_HOME)
}

/// Extract the filename component of a home-relative auth path (e.g.
/// `.claude/.credentials.json` -> `.credentials.json`), for host-side storage
/// under `$UR_CONFIG/<agent>/`, which flattens every agent's files into one
/// directory regardless of their subdirectory nesting in the worker home.
pub(crate) fn home_relative_filename(path: &str) -> Result<&std::ffi::OsStr> {
    Path::new(path)
        .file_name()
        .with_context(|| format!("auth path '{path}' has no filename"))
}

/// Per-agent credential management: seeding, extraction, and host path
/// resolution. `None` from [`credential_manager_for`] means the agent has no
/// auth profile — every caller acknowledges that rather than the trait
/// silently no-op'ing.
pub trait AgentCredentialManager: Send + Sync {
    fn agent_type(&self) -> AgentType;

    /// Ensure credentials exist on disk for container mounting.
    fn ensure_credentials(&self, max_age: Duration) -> Result<()>;

    /// Save credentials and config extracted from a running container to the host config dir.
    fn save_from_container(
        &self,
        runtime: &dyn ContainerRuntime,
        container_id: &ContainerId,
    ) -> Result<Vec<PathBuf>>;

    /// Resolve the host-side credentials file path.
    fn host_credentials_path(&self) -> Result<PathBuf>;
}

// No `host_app_config_path` on the trait: only Claude extracts an app config
// from a container, and only its own `save_from_container` ever needs the host
// path. It lives as an inherent method on `ClaudeCredentialManager` so a new
// agent isn't made to implement something nothing calls through the trait.

/// Build the credential manager for `agent`, or `None` if the agent has no
/// auth profile.
pub fn credential_manager_for(agent: AgentType) -> Option<Box<dyn AgentCredentialManager>> {
    let auth = agent.auth()?;
    match agent {
        AgentType::Claude => Some(Box::new(ClaudeCredentialManager { auth })),
        AgentType::Codex => Some(Box::new(CodexCredentialManager { auth })),
    }
}

/// Re-seed credentials for every agent that has an auth profile.
///
/// The CLI cannot know which agent(s) a launch will actually use — `[worker_modes]`
/// resolution is server-side and `crates/ur` has no `server` dependency — so this
/// iterates `AgentType::ALL` rather than a single agent.
///
/// Errors are real I/O failures (unresolvable config dir, unwritable credentials
/// path, or a host source that exists but can't be read) and propagate to the
/// caller. "The host has never logged into this agent" is *not* an error: each
/// manager's `ensure_credentials` warns and leaves the existing file alone rather
/// than failing — otherwise adding a second agent would break `ur start` for
/// every user who only uses the first one.
///
/// Every agent is attempted before returning: one agent's broken credentials must
/// not leave a healthy agent unseeded, since the caller cannot know which agent
/// the launch will resolve to. Failures are then reported together.
pub fn ensure_credentials_for_all_agents(max_age: Duration) -> Result<()> {
    let mut failures = Vec::new();
    for agent in AgentType::ALL {
        let Some(cred_mgr) = credential_manager_for(*agent) else {
            continue;
        };
        if let Err(e) = cred_mgr.ensure_credentials(max_age) {
            failures.push(format!("{}: {e:#}", agent.name()));
        }
    }
    if !failures.is_empty() {
        anyhow::bail!("failed to seed credentials — {}", failures.join("; "));
    }
    Ok(())
}

/// Read a file from a running container and write it to a host path.
///
/// Shared by every [`AgentCredentialManager`] implementation's
/// `save_from_container` — extracting a file from a container is agent-independent;
/// only the container-side path differs.
#[instrument(skip(runtime), fields(container = %container_id.0))]
pub(crate) fn save_file_from_container(
    runtime: &dyn ContainerRuntime,
    container_id: &ContainerId,
    container_path: &str,
    host_path: &Path,
) -> Result<PathBuf> {
    debug!(container_path, host_path = %host_path.display(), "reading file from container");
    let opts = ExecOpts {
        command: vec!["cat".into(), container_path.into()],
        workdir: None,
    };
    let output = runtime
        .exec(container_id, &opts)
        .with_context(|| format!("failed to read {container_path} from container"))?;
    if output.exit_code != 0 {
        anyhow::bail!(
            "container has no file at {container_path} — \
             log in to the agent in the container first"
        );
    }
    let contents = output.stdout.trim();
    if contents.is_empty() {
        anyhow::bail!("{container_path} in container is empty");
    }
    write_file(host_path, contents)?;
    info!(host_path = %host_path.display(), "saved file from container");
    Ok(host_path.to_path_buf())
}

/// Write content to a file, creating parent directories as needed.
#[instrument(skip(contents), fields(path = %path.display()))]
pub(crate) fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_manager_for_claude_returns_claude_manager() {
        let mgr = credential_manager_for(AgentType::Claude).expect("claude has a manager");
        assert_eq!(mgr.agent_type(), AgentType::Claude);
    }

    #[test]
    fn credential_manager_for_codex_returns_codex_manager() {
        let mgr = credential_manager_for(AgentType::Codex).expect("codex has a manager");
        assert_eq!(mgr.agent_type(), AgentType::Codex);
    }
}
