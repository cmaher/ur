use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use container::{ContainerId, ContainerRuntime};
use ur_config::{AgentAuth, AgentType, AuthSource};

use super::{AgentCredentialManager, home_relative_filename};

/// Manages AGY's in-container OAuth cache.
///
/// The bind-mounted token file is the cache itself. AGY creates and refreshes
/// it in place, so there is no host source to seed and no extraction round trip.
#[derive(Clone)]
pub struct AgyCredentialManager {
    pub(super) auth: AgentAuth,
}

impl AgentCredentialManager for AgyCredentialManager {
    fn agent_type(&self) -> AgentType {
        AgentType::Agy
    }

    fn ensure_credentials(&self, _max_age: Duration) -> Result<()> {
        if self.auth.source != AuthSource::InContainer {
            anyhow::bail!("agy auth source is not in-container");
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_credentials_is_a_noop() {
        let manager = AgyCredentialManager {
            auth: AgentType::Agy.auth().expect("agy has auth"),
        };
        assert!(manager.ensure_credentials(Duration::ZERO).is_ok());
    }
}
