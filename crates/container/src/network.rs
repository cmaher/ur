use std::process::Command;

use anyhow::{Context, Result, bail};

/// Verifies the Docker network that ur worker containers join.
///
/// Networks are owned by docker-compose (the worker network uses `internal: true`
/// for isolation). This manager only checks existence; it never creates or removes
/// networks.
#[derive(Clone, Debug)]
pub struct NetworkManager {
    /// Docker-compatible CLI command (`docker` or `nerdctl`).
    docker_command: String,
    /// Name of the Docker network to verify.
    network_name: String,
}

impl NetworkManager {
    pub fn new(docker_command: String, network_name: String) -> Self {
        Self {
            docker_command,
            network_name,
        }
    }

    /// Return the network name managed by this instance.
    pub fn network_name(&self) -> &str {
        &self.network_name
    }

    /// Verify the Docker network exists (created by docker compose).
    ///
    /// Networks are owned by docker-compose — the worker network uses
    /// `internal: true` for isolation, which `docker network create` cannot
    /// express. This method only checks; it never creates.
    pub fn ensure(&self) -> Result<()> {
        if !self.exists()? {
            bail!(
                "Docker network '{}' does not exist — is docker compose running?",
                self.network_name
            );
        }
        Ok(())
    }

    /// Check whether the managed network already exists.
    pub fn exists(&self) -> Result<bool> {
        let output = Command::new(&self.docker_command)
            .args(["network", "inspect", &self.network_name])
            .output()
            .with_context(|| {
                format!("failed to execute {} network inspect", self.docker_command)
            })?;
        Ok(output.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager() -> NetworkManager {
        NetworkManager::new("docker".into(), "ur-workers".into())
    }

    #[test]
    fn custom_network_name() {
        let mgr = NetworkManager::new("docker".into(), "custom-net".into());
        assert_eq!(mgr.network_name(), "custom-net");
    }

    #[test]
    fn custom_docker_command() {
        let mgr = NetworkManager::new("nerdctl".into(), "ur-workers".into());
        assert_eq!(mgr.docker_command, "nerdctl");
    }

    #[test]
    fn clone_works() {
        let mgr = test_manager();
        let cloned = mgr.clone();
        assert_eq!(cloned.network_name(), mgr.network_name());
        assert_eq!(cloned.docker_command, mgr.docker_command);
    }
}
