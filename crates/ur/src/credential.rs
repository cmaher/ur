use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use container::{ContainerId, ContainerRuntime, ExecOpts};
use tracing::{debug, info, instrument, warn};
use ur_config::{AgentAuth, AgentType};

fn worker_home() -> &'static Path {
    Path::new(ur_config::WORKER_HOME)
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

    /// Resolve the host-side app config file path.
    fn host_app_config_path(&self) -> Result<PathBuf>;
}

/// Manages Claude Code credentials for container workers.
///
/// Credentials (`.credentials.json`) are stored at `$UR_CONFIG/claude/` on the
/// host and bind-mounted into all worker containers. The app config
/// (`.claude.json`) is baked into the container image.
#[derive(Clone)]
pub struct ClaudeCredentialManager {
    auth: AgentAuth,
}

impl ClaudeCredentialManager {
    /// Read a file from the container and write it to the host path.
    #[instrument(skip(self, runtime), fields(container = %container_id.0))]
    fn save_file_from_container(
        &self,
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
                 login to Claude Code in the container first"
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
}

impl AgentCredentialManager for ClaudeCredentialManager {
    fn agent_type(&self) -> AgentType {
        AgentType::Claude
    }

    /// Re-seeds OAuth credentials from the host Claude Code installation if the
    /// shared credentials file is missing, empty, or older than `max_age`. On
    /// macOS, reads from the Keychain; on Linux, copies from
    /// `~/.claude/.credentials.json`. Pass `Duration::ZERO` to force a re-seed
    /// unconditionally.
    ///
    /// Between re-seeds, containers own their session independently — token
    /// refreshes in containers write back to the shared mount without touching
    /// the host credentials. The age check ensures host re-logins eventually
    /// propagate without clobbering fresh container-driven refreshes on every
    /// launch.
    #[instrument(skip(self))]
    fn ensure_credentials(&self, max_age: Duration) -> Result<()> {
        let creds_path = self.host_credentials_path()?;

        // Re-seed if the file is missing, empty/stub, or older than max_age.
        // An empty/stub file can be left behind by the server's Docker
        // bind-mount setup, so treat it as missing.
        let needs_seed = match std::fs::metadata(&creds_path) {
            Err(_) => true,
            Ok(meta) if meta.len() < 10 => true,
            Ok(meta) => match meta.modified() {
                Ok(mtime) => mtime.elapsed().map(|age| age >= max_age).unwrap_or(true),
                Err(_) => true,
            },
        };
        if needs_seed {
            if let Ok(creds_json) = read_host_credentials(self.agent_type(), self.auth) {
                info!(path = %creds_path.display(), "seeding credentials from host Claude Code");
                write_file(&creds_path, &creds_json)?;
            } else {
                debug!(path = %creds_path.display(), "no host credentials found to seed");
            }
        } else {
            debug!(path = %creds_path.display(), "credentials are fresh");
        }

        Ok(())
    }

    /// Reads both `.credentials.json` and `.claude.json` from the container and
    /// writes them to `$UR_CONFIG/claude/`.
    #[instrument(skip(self, runtime), fields(container = %container_id.0))]
    fn save_from_container(
        &self,
        runtime: &dyn ContainerRuntime,
        container_id: &ContainerId,
    ) -> Result<Vec<PathBuf>> {
        let mut saved = Vec::new();

        let creds_container_path = worker_home()
            .join(self.agent_type().home_subdir())
            .join(self.auth.credentials_filename);
        let creds_path = self.save_file_from_container(
            runtime,
            container_id,
            &creds_container_path.to_string_lossy(),
            &self.host_credentials_path()?,
        )?;
        saved.push(creds_path);

        let config_container_path = worker_home().join(self.auth.app_config_filename);
        let config_path = self.save_file_from_container(
            runtime,
            container_id,
            &config_container_path.to_string_lossy(),
            &self.host_app_config_path()?,
        )?;
        saved.push(config_path);

        info!(count = saved.len(), "credentials saved from container");
        Ok(saved)
    }

    fn host_credentials_path(&self) -> Result<PathBuf> {
        let config_dir = ur_config::resolve_config_dir()?;
        Ok(config_dir
            .join(self.agent_type().name())
            .join(self.auth.credentials_filename))
    }

    fn host_app_config_path(&self) -> Result<PathBuf> {
        let config_dir = ur_config::resolve_config_dir()?;
        Ok(config_dir
            .join(self.agent_type().name())
            .join(self.auth.app_config_filename))
    }
}

/// Build the credential manager for `agent`, or `None` if the agent has no
/// auth profile.
pub fn credential_manager_for(agent: AgentType) -> Option<Box<dyn AgentCredentialManager>> {
    let auth = agent.auth()?;
    match agent {
        AgentType::Claude => Some(Box::new(ClaudeCredentialManager { auth })),
    }
}

/// Read the agent's own OAuth credentials from the host system.
///
/// On macOS, reads from the Keychain. On Linux, reads directly from the
/// agent's native credentials file in its own home directory (e.g.
/// `~/.claude/.credentials.json` for Claude).
///
/// Takes the resolved [`AgentAuth`] rather than re-deriving it from `agent`:
/// only a manager built by [`credential_manager_for`] can reach here, and that
/// manager already holds the profile, so "this agent has no auth" is
/// unrepresentable instead of being an unwrap.
#[instrument(skip(agent, auth))]
fn read_host_credentials(agent: AgentType, auth: AgentAuth) -> Result<String> {
    read_platform_credentials(agent, auth)
}

#[cfg(target_os = "macos")]
fn read_platform_credentials(agent: AgentType, auth: AgentAuth) -> Result<String> {
    use std::process::Command;
    debug!(
        agent = agent.name(),
        "reading credentials from macOS Keychain"
    );
    let output = Command::new("security")
        .args(["find-generic-password", "-s", auth.keychain_service, "-w"])
        .output()
        .context("failed to run `security` command")?;
    if !output.status.success() {
        warn!("no credentials found in macOS Keychain");
        anyhow::bail!(
            "no credentials in macOS Keychain for service {:?} — \
             log in to Claude Code on this machine first",
            auth.keychain_service
        );
    }
    let json =
        String::from_utf8(output.stdout).context("keychain credentials are not valid UTF-8")?;
    let trimmed = json.trim().to_string();
    if trimmed.is_empty() {
        warn!("keychain credentials are empty");
        anyhow::bail!("keychain credentials are empty");
    }
    info!("credentials read from macOS Keychain");
    Ok(trimmed)
}

#[cfg(not(target_os = "macos"))]
fn read_platform_credentials(agent: AgentType, auth: AgentAuth) -> Result<String> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = PathBuf::from(home)
        .join(agent.home_subdir())
        .join(auth.credentials_filename);
    debug!(path = %path.display(), "reading credentials from agent's native config");
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("{} is empty", path.display());
    }
    info!(path = %path.display(), "credentials read from agent's native config");
    Ok(trimmed)
}

/// Write content to a file, creating parent directories as needed.
#[instrument(skip(contents), fields(path = %path.display()))]
fn write_file(path: &Path, contents: &str) -> Result<()> {
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
    fn host_credentials_path_is_under_config_dir() {
        let mgr = credential_manager_for(AgentType::Claude).expect("claude has a manager");
        if let Ok(path) = mgr.host_credentials_path() {
            assert!(
                path.ends_with(
                    AgentType::Claude
                        .auth()
                        .expect("claude has auth")
                        .credentials_filename
                )
            );
        }
    }

    #[test]
    fn host_config_path_is_under_config_dir() {
        let mgr = credential_manager_for(AgentType::Claude).expect("claude has a manager");
        if let Ok(path) = mgr.host_app_config_path() {
            assert!(
                path.ends_with(
                    AgentType::Claude
                        .auth()
                        .expect("claude has auth")
                        .app_config_filename
                )
            );
        }
    }
}
