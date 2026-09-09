use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use container::{ContainerId, ContainerRuntime};
use tracing::{debug, info, instrument, warn};
use ur_config::{AgentAuth, AgentType, AuthSource};

use super::{
    AgentCredentialManager, home_relative_filename, save_file_from_container, worker_home,
    write_file,
};

/// Manages OpenAI Codex CLI credentials for container workers.
///
/// Credentials (`auth.json`) are stored at `$UR_CONFIG/codex/` on the host and
/// bind-mounted into all worker containers, mirroring the Claude flow but
/// always as a plain host file — codex has no keychain integration on any
/// platform. The app config (`config.toml`) is baked into the container image.
#[derive(Clone)]
pub struct CodexCredentialManager {
    pub(super) auth: AgentAuth,
}

impl CodexCredentialManager {
    /// Host path to the codex CLI's own `auth.json`, under the host user's home
    /// directory (e.g. `~/.codex/auth.json`) — never a keychain for this agent.
    fn host_source_path(&self) -> Result<PathBuf> {
        let AuthSource::HostFile { path_from_home } = self.auth.source else {
            anyhow::bail!("codex auth source is not a host file");
        };
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home).join(path_from_home))
    }
}

impl AgentCredentialManager for CodexCredentialManager {
    fn agent_type(&self) -> AgentType {
        AgentType::Codex
    }

    /// Re-seeds `auth.json` from the host `~/.codex/auth.json` if the shared
    /// file is missing, empty, or older than `max_age`. Pass `Duration::ZERO`
    /// to force a re-seed unconditionally. See [`seed_from_host_file`] for the
    /// skip-vs-fail distinction.
    #[instrument(skip(self))]
    fn ensure_credentials(&self, max_age: Duration) -> Result<()> {
        let creds_path = self.host_credentials_path()?;
        let source_path = self.host_source_path()?;
        seed_from_host_file(&creds_path, &source_path, max_age)
    }

    /// Reads `auth.json` from the container and writes it to `$UR_CONFIG/codex/`.
    #[instrument(skip(self, runtime), fields(container = %container_id.0))]
    fn save_from_container(
        &self,
        runtime: &dyn ContainerRuntime,
        container_id: &ContainerId,
    ) -> Result<Vec<PathBuf>> {
        let creds_container_path = worker_home().join(self.auth.credentials_path);
        let creds_path = save_file_from_container(
            runtime,
            container_id,
            &creds_container_path.to_string_lossy(),
            &self.host_credentials_path()?,
        )?;
        Ok(vec![creds_path])
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

/// Re-seeds `creds_path` from `source_path` if `creds_path` is missing, empty,
/// or older than `max_age`.
///
/// Unlike Claude's keychain fallback, an *absent* `source_path` is not an
/// error — it just means this host has never run `codex login`, which must
/// not break `ur start` for Claude-only users (a warning names the agent and
/// the remediation instead). A `source_path` that exists but can't be read or
/// is empty, though, is a real failure: unlike "never logged in", that state
/// should never happen and hiding it would mask a broken login.
fn seed_from_host_file(creds_path: &Path, source_path: &Path, max_age: Duration) -> Result<()> {
    // Re-seed if the file is missing, empty/stub, or older than max_age.
    // An empty/stub file can be left behind by the server's Docker
    // bind-mount setup, so treat it as missing.
    let needs_seed = match std::fs::metadata(creds_path) {
        Err(_) => true,
        Ok(meta) if meta.len() < 10 => true,
        Ok(meta) => match meta.modified() {
            Ok(mtime) => mtime.elapsed().map(|age| age >= max_age).unwrap_or(true),
            Err(_) => true,
        },
    };
    if !needs_seed {
        debug!(path = %creds_path.display(), "credentials are fresh");
        return Ok(());
    }

    if !source_path.exists() {
        warn!(
            path = %source_path.display(),
            "no host codex credentials found — run `codex login` on this machine, \
             then `ur worker reseed-credentials --agent codex`"
        );
        return Ok(());
    }

    let contents = std::fs::read_to_string(source_path)
        .with_context(|| format!("failed to read {}", source_path.display()))?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{} is empty", source_path.display());
    }

    info!(path = %creds_path.display(), "seeding credentials from host codex CLI");
    write_file(creds_path, trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::credential_manager_for;

    #[test]
    fn host_credentials_path_is_under_config_dir() {
        let mgr = credential_manager_for(AgentType::Codex).expect("codex has a manager");
        if let Ok(path) = mgr.host_credentials_path() {
            assert!(path.ends_with("auth.json"));
        }
    }

    #[test]
    fn host_config_path_is_under_config_dir() {
        let mgr = credential_manager_for(AgentType::Codex).expect("codex has a manager");
        if let Ok(path) = mgr.host_app_config_path() {
            assert!(path.ends_with("config.toml"));
        }
    }

    #[test]
    fn codex_container_paths_are_unchanged() {
        let auth = AgentType::Codex.auth().expect("codex has auth");
        assert_eq!(
            worker_home().join(auth.credentials_path),
            PathBuf::from("/home/worker/.codex/auth.json")
        );
        assert_eq!(
            worker_home().join(auth.app_config_path),
            PathBuf::from("/home/worker/.codex/config.toml")
        );
    }

    /// A missing host source must warn and succeed, not fail — otherwise
    /// adding codex support would break `ur start` for anyone who has never
    /// run `codex login`.
    #[test]
    fn seed_skips_when_source_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let creds_path = tmp.path().join("creds").join("auth.json");
        let source_path = tmp.path().join("home").join(".codex").join("auth.json");

        assert!(seed_from_host_file(&creds_path, &source_path, Duration::ZERO).is_ok());
        assert!(!creds_path.exists(), "nothing should be written");
    }

    /// A host source that exists but is empty must fail loudly rather than
    /// being treated the same as "never logged in".
    #[test]
    fn seed_fails_when_source_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let creds_path = tmp.path().join("creds").join("auth.json");
        let source_path = tmp.path().join("home").join(".codex").join("auth.json");
        std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        std::fs::write(&source_path, "").unwrap();

        assert!(seed_from_host_file(&creds_path, &source_path, Duration::ZERO).is_err());
    }

    /// A present, non-empty source seeds the destination.
    #[test]
    fn seed_writes_when_source_present() {
        let tmp = tempfile::tempdir().unwrap();
        let creds_path = tmp.path().join("creds").join("auth.json");
        let source_path = tmp.path().join("home").join(".codex").join("auth.json");
        std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        std::fs::write(&source_path, "{\"token\":\"abc\"}").unwrap();

        assert!(seed_from_host_file(&creds_path, &source_path, Duration::ZERO).is_ok());
        assert_eq!(
            std::fs::read_to_string(&creds_path).unwrap(),
            "{\"token\":\"abc\"}"
        );
    }

    /// A fresh destination (within `max_age`) is left alone even if the
    /// source has since changed.
    #[test]
    fn seed_leaves_fresh_destination_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let creds_path = tmp.path().join("creds").join("auth.json");
        let source_path = tmp.path().join("home").join(".codex").join("auth.json");
        std::fs::create_dir_all(creds_path.parent().unwrap()).unwrap();
        std::fs::write(&creds_path, "existing-credentials-blob").unwrap();

        assert!(seed_from_host_file(&creds_path, &source_path, Duration::from_secs(3600)).is_ok());
        assert_eq!(
            std::fs::read_to_string(&creds_path).unwrap(),
            "existing-credentials-blob"
        );
    }
}
