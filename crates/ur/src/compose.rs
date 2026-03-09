use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Manages urd lifecycle via Docker Compose.
///
/// Wraps `docker compose` CLI commands targeting the project's compose file.
/// The compose file path is resolved from `ur_config::Config::compose_file`.
#[derive(Debug, Clone)]
pub struct ComposeManager {
    compose_file: PathBuf,
    /// Environment variables passed to `docker compose` (forwarded to the compose file's
    /// variable interpolation, e.g. `${URD_PORT}`, `${UR_CONFIG}`).
    env_vars: Vec<(String, String)>,
}

impl ComposeManager {
    pub fn new(compose_file: PathBuf, env_vars: Vec<(String, String)>) -> Self {
        Self {
            compose_file,
            env_vars,
        }
    }

    /// Build the base `docker compose -f <file>` command with environment variables.
    fn base_command(&self) -> Command {
        let mut cmd = Command::new("docker");
        cmd.arg("compose");
        cmd.arg("-f");
        cmd.arg(&self.compose_file);
        for (key, value) in &self.env_vars {
            cmd.env(key, value);
        }
        cmd
    }

    /// Start urd via `docker compose up -d`.
    ///
    /// Validates that the compose file exists before invoking docker compose.
    pub fn up(&self) -> Result<()> {
        self.validate_compose_file()?;

        let output = self
            .base_command()
            .args(["up", "-d", "--wait"])
            .output()
            .context("failed to run docker compose up")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("docker compose up failed: {stderr}");
        }

        Ok(())
    }

    /// Stop and remove urd containers/networks via `docker compose down`.
    pub fn down(&self) -> Result<()> {
        self.validate_compose_file()?;

        let output = self
            .base_command()
            .args(["down"])
            .output()
            .context("failed to run docker compose down")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("docker compose down failed: {stderr}");
        }

        Ok(())
    }

    /// Check if the urd service is running via `docker compose ps`.
    ///
    /// Returns `true` if at least one container for the urd service is running.
    pub fn is_running(&self) -> Result<bool> {
        if !self.compose_file.exists() {
            return Ok(false);
        }

        let output = self
            .base_command()
            .args(["ps", "--status", "running", "--format", "{{.Name}}"])
            .output()
            .context("failed to run docker compose ps")?;

        if !output.status.success() {
            // compose ps can fail if the project was never started; treat as not running
            return Ok(false);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(!stdout.trim().is_empty())
    }

    fn validate_compose_file(&self) -> Result<()> {
        if !self.compose_file.exists() {
            bail!(
                "compose file not found: {}\n\
                 Set compose_file in ur.toml or copy docker-compose.yml to {}",
                self.compose_file.display(),
                self.compose_file.parent().unwrap_or(Path::new("/")).display()
            );
        }
        Ok(())
    }
}

/// Build a `ComposeManager` from the resolved ur config.
///
/// Forwards `UR_CONFIG`, `UR_WORKSPACE`, and `URD_PORT` as environment variables
/// so the compose file's variable interpolation picks them up.
pub fn compose_manager_from_config(config: &ur_config::Config) -> ComposeManager {
    let mut env_vars = vec![
        (
            "UR_CONFIG".to_string(),
            config.config_dir.to_string_lossy().into_owned(),
        ),
        (
            "UR_WORKSPACE".to_string(),
            config.workspace.to_string_lossy().into_owned(),
        ),
        ("URD_PORT".to_string(), config.daemon_port.to_string()),
    ];

    // Forward UR_CONTAINER if set so compose can potentially use it
    if let Ok(val) = std::env::var("UR_CONTAINER") {
        env_vars.push(("UR_CONTAINER".to_string(), val));
    }

    ComposeManager::new(config.compose_file.clone(), env_vars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn validate_compose_file_missing() {
        let manager = ComposeManager::new(PathBuf::from("/nonexistent/docker-compose.yml"), vec![]);
        let err = manager.validate_compose_file().unwrap_err();
        assert!(
            err.to_string().contains("compose file not found"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_compose_file_exists() {
        let tmp = TempDir::new().unwrap();
        let compose_path = tmp.path().join("docker-compose.yml");
        fs::write(&compose_path, "services: {}").unwrap();
        let manager = ComposeManager::new(compose_path, vec![]);
        manager.validate_compose_file().unwrap();
    }

    #[test]
    fn is_running_returns_false_when_file_missing() {
        let manager = ComposeManager::new(PathBuf::from("/nonexistent/docker-compose.yml"), vec![]);
        assert!(!manager.is_running().unwrap());
    }

    #[test]
    fn compose_manager_from_config_sets_env_vars() {
        let config = ur_config::Config {
            config_dir: PathBuf::from("/test/config"),
            workspace: PathBuf::from("/test/workspace"),
            daemon_port: 9999,
            compose_file: PathBuf::from("/test/docker-compose.yml"),
            proxy: ur_config::ProxyConfig {
                port: ur_config::DEFAULT_PROXY_PORT,
                allowlist: vec![],
            },
            network: ur_config::NetworkConfig {
                name: "ur".to_string(),
                urd_hostname: "urd".to_string(),
            },
        };

        let manager = compose_manager_from_config(&config);
        assert_eq!(manager.compose_file, PathBuf::from("/test/docker-compose.yml"));
        assert!(manager.env_vars.contains(&("UR_CONFIG".to_string(), "/test/config".to_string())));
        assert!(manager
            .env_vars
            .contains(&("UR_WORKSPACE".to_string(), "/test/workspace".to_string())));
        assert!(manager.env_vars.contains(&("URD_PORT".to_string(), "9999".to_string())));
    }
}
