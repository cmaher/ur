use std::process::Command;

use anyhow::{Context, Result, bail};

/// Manages the lifecycle of a Docker network used by ur-managed containers.
///
/// All containers (server + workers) join this shared network so they can
/// communicate via Docker internal DNS.
#[derive(Clone, Debug)]
pub struct NetworkManager {
    /// Docker-compatible CLI command (`docker` or `nerdctl`).
    docker_command: String,
    /// Name of the Docker network to manage.
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

    /// Ensure the Docker network exists, creating it if necessary.
    ///
    /// Uses `docker network inspect` to check existence, then
    /// `docker network create` with the `bridge` driver if missing.
    /// Returns `Ok(true)` if the network was created, `Ok(false)` if it
    /// already existed.
    pub fn ensure(&self) -> Result<bool> {
        if self.exists()? {
            return Ok(false);
        }
        self.create()?;
        Ok(true)
    }

    /// Check whether the managed network already exists.
    pub fn exists(&self) -> Result<bool> {
        let args = Self::inspect_args(&self.network_name);
        let output = Command::new(&self.docker_command)
            .args(&args)
            .output()
            .with_context(|| {
                format!("failed to execute {} network inspect", self.docker_command)
            })?;
        Ok(output.status.success())
    }

    /// Create the managed network with the bridge driver.
    pub fn create(&self) -> Result<()> {
        let args = Self::create_args(&self.network_name);
        let output = Command::new(&self.docker_command)
            .args(&args)
            .output()
            .with_context(|| format!("failed to execute {} network create", self.docker_command))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "{} network create failed: {}",
                self.docker_command,
                stderr.trim()
            );
        }
        Ok(())
    }

    /// Remove the managed network.
    ///
    /// Fails if containers are still connected. The caller should stop/disconnect
    /// all containers before calling this.
    pub fn remove(&self) -> Result<()> {
        let args = Self::remove_args(&self.network_name);
        let output = Command::new(&self.docker_command)
            .args(&args)
            .output()
            .with_context(|| format!("failed to execute {} network rm", self.docker_command))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "{} network rm failed: {}",
                self.docker_command,
                stderr.trim()
            );
        }
        Ok(())
    }

    // -- Arg builders (public for unit testing) --

    pub fn inspect_args(network_name: &str) -> Vec<String> {
        vec!["network".into(), "inspect".into(), network_name.into()]
    }

    pub fn create_args(network_name: &str) -> Vec<String> {
        vec![
            "network".into(),
            "create".into(),
            "--driver".into(),
            "bridge".into(),
            network_name.into(),
        ]
    }

    pub fn remove_args(network_name: &str) -> Vec<String> {
        vec!["network".into(), "rm".into(), network_name.into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    fn test_manager() -> NetworkManager {
        NetworkManager::new("docker".into(), "ur".into())
    }

    #[test]
    fn inspect_args_correct() {
        assert_eq!(
            NetworkManager::inspect_args("ur"),
            vec![s("network"), s("inspect"), s("ur")]
        );
    }

    #[test]
    fn create_args_correct() {
        assert_eq!(
            NetworkManager::create_args("ur"),
            vec![
                s("network"),
                s("create"),
                s("--driver"),
                s("bridge"),
                s("ur"),
            ]
        );
    }

    #[test]
    fn remove_args_correct() {
        assert_eq!(
            NetworkManager::remove_args("my-net"),
            vec![s("network"), s("rm"), s("my-net")]
        );
    }

    #[test]
    fn custom_network_name() {
        let mgr = NetworkManager::new("docker".into(), "custom-net".into());
        assert_eq!(mgr.network_name(), "custom-net");
    }

    #[test]
    fn custom_docker_command() {
        let mgr = NetworkManager::new("nerdctl".into(), "ur".into());
        assert_eq!(mgr.docker_command, "nerdctl");
    }

    #[test]
    fn clone_works() {
        let mgr = test_manager();
        let cloned = mgr.clone();
        assert_eq!(cloned.network_name(), mgr.network_name());
        assert_eq!(cloned.docker_command, mgr.docker_command);
    }

    #[test]
    fn create_args_with_special_chars() {
        let args = NetworkManager::create_args("ur-test-123");
        assert_eq!(args[4], "ur-test-123");
    }
}
