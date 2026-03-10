use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// Embedded compose template with `{{NETWORK_NAME}}` and `{{WORKER_NETWORK_NAME}}` placeholders.
const COMPOSE_TEMPLATE: &str = include_str!("../../../containers/docker-compose.yml");

/// Manages server lifecycle via Docker Compose.
///
/// Wraps `docker compose` CLI commands targeting a rendered compose file.
/// The compose file is written on `up()` and removed on `down()`.
#[derive(Debug, Clone)]
pub struct ComposeManager {
    compose_file: PathBuf,
    /// Environment variables passed to `docker compose` (forwarded to the compose file's
    /// variable interpolation, e.g. `${UR_SERVER_PORT}`, `${UR_CONFIG}`).
    env_vars: Vec<(String, String)>,
    /// Rendered compose file content (template with network names filled in).
    compose_content: String,
}

impl ComposeManager {
    pub fn new(
        compose_file: PathBuf,
        env_vars: Vec<(String, String)>,
        compose_content: String,
    ) -> Self {
        Self {
            compose_file,
            env_vars,
            compose_content,
        }
    }

    /// Build the base `docker compose -f <file>` command with environment variables.
    fn base_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new("docker");
        cmd.arg("compose");
        cmd.arg("-f");
        cmd.arg(&self.compose_file);
        for (key, value) in &self.env_vars {
            cmd.env(key, value);
        }
        cmd
    }

    /// Start the server via `docker compose up -d`.
    ///
    /// Renders and writes the compose file before invoking docker compose.
    /// Runs `docker compose down` first to clean up stale networks/containers
    /// from a previous run that wasn't shut down cleanly.
    pub fn up(&self) -> Result<()> {
        fs::write(&self.compose_file, &self.compose_content).with_context(|| {
            format!(
                "failed to write compose file: {}",
                self.compose_file.display()
            )
        })?;

        // Clean up stale networks/containers from a previous run so `up` doesn't
        // fail with "network already exists".
        let _ = self.base_command().args(["down"]).output();

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

    /// Stop and remove server containers/networks via `docker compose down`.
    ///
    /// Removes the compose file after a successful teardown.
    pub fn down(&self) -> Result<()> {
        if !self.compose_file.exists() {
            bail!(
                "compose file not found: {} — is the server running?",
                self.compose_file.display()
            );
        }

        let output = self
            .base_command()
            .args(["down"])
            .output()
            .context("failed to run docker compose down")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("docker compose down failed: {stderr}");
        }

        // Clean up the rendered compose file
        let _ = fs::remove_file(&self.compose_file);

        Ok(())
    }

    /// Check if the server service is running via `docker compose ps`.
    ///
    /// Returns `true` if at least one container for the server service is running.
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
}

/// Render the compose template with resolved network names.
pub fn render_compose(network: &ur_config::NetworkConfig) -> String {
    COMPOSE_TEMPLATE
        .replace("{{NETWORK_NAME}}", &network.name)
        .replace("{{WORKER_NETWORK_NAME}}", &network.worker_name)
}

/// Build a `ComposeManager` from the resolved ur config.
///
/// Forwards `UR_CONFIG`, `UR_WORKSPACE`, and `UR_SERVER_PORT` as environment variables
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
        ("UR_SERVER_PORT".to_string(), config.daemon_port.to_string()),
    ];

    // Forward UR_CONTAINER if set so compose can potentially use it
    if let Ok(val) = std::env::var("UR_CONTAINER") {
        env_vars.push(("UR_CONTAINER".to_string(), val));
    }

    let compose_content = render_compose(&config.network);

    ComposeManager::new(config.compose_file.clone(), env_vars, compose_content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn is_running_returns_false_when_file_missing() {
        let manager = ComposeManager::new(
            PathBuf::from("/nonexistent/docker-compose.yml"),
            vec![],
            String::new(),
        );
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
                hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
                allowlist: vec![],
            },
            network: ur_config::NetworkConfig {
                name: "ur".to_string(),
                worker_name: "ur-workers".to_string(),
                server_hostname: "ur-server".to_string(),
            },
            hostd_port: ur_config::DEFAULT_HOSTD_PORT,
        };

        let manager = compose_manager_from_config(&config);
        assert_eq!(
            manager.compose_file,
            PathBuf::from("/test/docker-compose.yml")
        );
        assert!(
            manager
                .env_vars
                .contains(&("UR_CONFIG".to_string(), "/test/config".to_string()))
        );
        assert!(
            manager
                .env_vars
                .contains(&("UR_WORKSPACE".to_string(), "/test/workspace".to_string()))
        );
        assert!(
            manager
                .env_vars
                .contains(&("UR_SERVER_PORT".to_string(), "9999".to_string()))
        );
    }

    #[test]
    fn render_compose_replaces_placeholders() {
        let network = ur_config::NetworkConfig {
            name: "test-net".to_string(),
            worker_name: "test-workers".to_string(),
            server_hostname: "test-server".to_string(),
        };
        let rendered = render_compose(&network);
        assert!(rendered.contains("name: test-net"));
        assert!(rendered.contains("name: test-workers"));
        assert!(!rendered.contains("{{NETWORK_NAME}}"));
        assert!(!rendered.contains("{{WORKER_NETWORK_NAME}}"));
    }

    #[test]
    fn up_writes_compose_file() {
        let tmp = TempDir::new().unwrap();
        let compose_path = tmp.path().join("docker-compose.yml");
        let content = "services: {}".to_string();
        let manager = ComposeManager::new(compose_path.clone(), vec![], content.clone());

        // up() will fail on docker compose, but should still write the file
        let _ = manager.up();
        assert!(compose_path.exists());
        assert_eq!(fs::read_to_string(&compose_path).unwrap(), content);
    }
}
