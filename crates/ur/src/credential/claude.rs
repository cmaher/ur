use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use container::{ContainerId, ContainerRuntime};
use tracing::{debug, info, instrument};
use ur_config::{AgentAuth, AgentType};

use super::{
    AgentCredentialManager, home_relative_filename, read_host_credentials,
    save_file_from_container, worker_home, write_file,
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
            Ok(meta) if meta.len() < ur_config::MIN_SEEDED_CREDENTIALS_BYTES => true,
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
}

impl ClaudeCredentialManager {
    /// Resolve the host-side `.claude.json` path. Inherent rather than a trait
    /// method: Claude is the only agent that extracts an app config from a
    /// container (codex's `config.toml` is baked into the image), and
    /// `save_from_container` above is its only caller.
    fn host_app_config_path(&self) -> Result<PathBuf> {
        let config_dir = ur_config::resolve_config_dir()?;
        Ok(config_dir
            .join(self.agent_type().name())
            .join(home_relative_filename(self.auth.app_config_path)?))
    }
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
        let mgr = ClaudeCredentialManager {
            auth: AgentType::Claude.auth().expect("claude has auth"),
        };
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
