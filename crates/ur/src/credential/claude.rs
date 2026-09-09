use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use container::{ContainerId, ContainerRuntime};
use tracing::{debug, info, instrument, warn};
use ur_config::{AgentAuth, AgentType, AuthSource};

use super::{
    AgentCredentialManager, home_relative_filename, save_file_from_container, worker_home,
    write_file,
};

/// Manages Claude Code credentials for container workers.
///
/// Credentials (`.credentials.json`) are stored at `$UR_CONFIG/claude/` on the
/// host and bind-mounted into all worker containers. The app config
/// (`.claude.json`) is baked into the container image.
#[derive(Clone)]
pub struct ClaudeCredentialManager {
    pub(super) auth: AgentAuth,
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

        let creds_container_path = worker_home().join(self.auth.credentials_path);
        let creds_path = save_file_from_container(
            runtime,
            container_id,
            &creds_container_path.to_string_lossy(),
            &self.host_credentials_path()?,
        )?;
        saved.push(creds_path);

        let config_container_path = worker_home().join(self.auth.app_config_path);
        let config_path = save_file_from_container(
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
            .join(home_relative_filename(self.auth.credentials_path)?))
    }

    fn host_app_config_path(&self) -> Result<PathBuf> {
        let config_dir = ur_config::resolve_config_dir()?;
        Ok(config_dir
            .join(self.agent_type().name())
            .join(home_relative_filename(self.auth.app_config_path)?))
    }
}

/// Read the agent's own OAuth credentials from the host system.
///
/// On macOS, reads from the Keychain. On Linux, reads directly from the
/// agent's native credentials file in its own home directory (e.g.
/// `~/.claude/.credentials.json` for Claude).
///
/// Takes the resolved [`AgentAuth`] rather than re-deriving it from `agent`:
/// only a manager built by [`super::credential_manager_for`] can reach here, and
/// that manager already holds the profile, so "this agent has no auth" is
/// unrepresentable instead of being an unwrap.
#[instrument(skip(agent, auth))]
fn read_host_credentials(agent: AgentType, auth: AgentAuth) -> Result<String> {
    read_platform_credentials(agent, auth)
}

#[cfg(target_os = "macos")]
fn read_platform_credentials(agent: AgentType, auth: AgentAuth) -> Result<String> {
    use std::process::Command;
    let service = match auth.source {
        AuthSource::Keychain { service, .. } => service,
        AuthSource::HostFile { path_from_home } => {
            return read_host_file_credentials(agent, path_from_home);
        }
    };
    debug!(
        agent = agent.name(),
        "reading credentials from macOS Keychain"
    );
    let output = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .context("failed to run `security` command")?;
    if !output.status.success() {
        warn!("no credentials found in macOS Keychain");
        anyhow::bail!(
            "no credentials in macOS Keychain for service {service:?} — \
             log in to Claude Code on this machine first"
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
    let path_from_home = match auth.source {
        AuthSource::Keychain { linux_fallback, .. } => linux_fallback,
        AuthSource::HostFile { path_from_home } => path_from_home,
    };
    read_host_file_credentials(agent, path_from_home)
}

/// Read credentials from a plain file under the host user's home directory,
/// used both for [`AuthSource::HostFile`] agents and as the Linux fallback
/// for [`AuthSource::Keychain`] agents.
fn read_host_file_credentials(agent: AgentType, path_from_home: &str) -> Result<String> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = PathBuf::from(home).join(path_from_home);
    debug!(agent = agent.name(), path = %path.display(), "reading credentials from agent's native config");
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("{} is empty", path.display());
    }
    info!(path = %path.display(), "credentials read from agent's native config");
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::credential_manager_for;

    #[test]
    fn host_credentials_path_is_under_config_dir() {
        let mgr = credential_manager_for(AgentType::Claude).expect("claude has a manager");
        if let Ok(path) = mgr.host_credentials_path() {
            assert!(path.ends_with(".credentials.json"));
        }
    }

    #[test]
    fn host_config_path_is_under_config_dir() {
        let mgr = credential_manager_for(AgentType::Claude).expect("claude has a manager");
        if let Ok(path) = mgr.host_app_config_path() {
            assert!(path.ends_with(".claude.json"));
        }
    }

    /// Pins Claude's resolved container-side auth paths to their pre-refactor
    /// values, since nothing else in the test suite exercises the exact
    /// strings baked into `AgentAuth`.
    #[test]
    fn claude_container_paths_are_unchanged() {
        let auth = AgentType::Claude.auth().expect("claude has auth");
        assert_eq!(
            worker_home().join(auth.credentials_path),
            PathBuf::from("/home/worker/.claude/.credentials.json")
        );
        assert_eq!(
            worker_home().join(auth.app_config_path),
            PathBuf::from("/home/worker/.claude.json")
        );
    }
}
