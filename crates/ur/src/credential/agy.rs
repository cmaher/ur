use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use container::{ContainerId, ContainerRuntime};
use tracing::{debug, info, instrument};
use ur_config::{AgentAuth, AgentType, AuthSource};

use super::{AgentCredentialManager, home_relative_filename, read_host_credentials, write_file};

/// Manages AGY's shared OAuth cache.
///
/// A host AGY login is seeded from the OS keyring (or AGY's file fallback on
/// Linux). The bind-mounted file remains writable so a worker can also perform
/// first-time interactive sign-in and refresh the bundle in place.
#[derive(Clone)]
pub struct AgyCredentialManager {
    pub(super) auth: AgentAuth,
}

impl AgentCredentialManager for AgyCredentialManager {
    fn agent_type(&self) -> AgentType {
        AgentType::Agy
    }

    #[instrument(skip(self))]
    fn ensure_credentials(&self, max_age: Duration) -> Result<()> {
        if !matches!(self.auth.source, AuthSource::Keychain { .. }) {
            anyhow::bail!("agy auth source is not a keychain");
        }

        let creds_path = self.host_credentials_path()?;
        seed_from_host(&creds_path, max_age, || {
            read_host_credentials(self.agent_type(), self.auth)
        })
    }

    fn save_from_container(
        &self,
        _runtime: &dyn ContainerRuntime,
        _container_id: &ContainerId,
    ) -> Result<Vec<PathBuf>> {
        anyhow::bail!(
            "agy credentials are already bind-mounted from the host; no save operation is needed"
        )
    }

    fn host_credentials_path(&self) -> Result<PathBuf> {
        let config_dir = ur_config::resolve_config_dir()?;
        Ok(config_dir
            .join(self.agent_type().name())
            .join(home_relative_filename(self.auth.credentials_path)?))
    }
}

fn seed_from_host(
    creds_path: &std::path::Path,
    max_age: Duration,
    read_source: impl FnOnce() -> Result<String>,
) -> Result<()> {
    let needs_seed = match std::fs::metadata(creds_path) {
        Err(_) => true,
        Ok(meta) if meta.len() < ur_config::MIN_SEEDED_CREDENTIALS_BYTES => true,
        Ok(meta) => match meta.modified() {
            Ok(mtime) => mtime.elapsed().map(|age| age >= max_age).unwrap_or(true),
            Err(_) => true,
        },
    };
    if !needs_seed {
        debug!(path = %creds_path.display(), "credentials are fresh");
        return Ok(());
    }

    match read_source() {
        Ok(credentials) => {
            info!(path = %creds_path.display(), "seeding credentials from host AGY");
            write_file(creds_path, &credentials)?;
        }
        Err(error) => {
            debug!(path = %creds_path.display(), %error, "no host AGY credentials found to seed");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_source_is_host_keychain_with_worker_bootstrap_fallback() {
        let manager = AgyCredentialManager {
            auth: AgentType::Agy.auth().expect("agy has auth"),
        };
        assert!(matches!(manager.auth.source, AuthSource::Keychain { .. }));
        assert!(manager.auth.allows_in_container_bootstrap);
    }

    #[test]
    fn seed_writes_host_keyring_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().join("agy/antigravity-oauth-token");
        let payload = r#"{"token":{"access_token":"abc","refresh_token":"def"}}"#;

        seed_from_host(&destination, Duration::ZERO, || Ok(payload.to_owned())).unwrap();

        assert_eq!(std::fs::read_to_string(destination).unwrap(), payload);
    }

    #[test]
    fn seed_preserves_worker_credentials_when_host_has_no_login() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().join("agy/antigravity-oauth-token");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(&destination, "worker-refreshed-credentials").unwrap();

        seed_from_host(&destination, Duration::ZERO, || anyhow::bail!("not found")).unwrap();

        assert_eq!(
            std::fs::read_to_string(destination).unwrap(),
            "worker-refreshed-credentials"
        );
    }
}
