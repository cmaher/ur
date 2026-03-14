use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tracing::{debug, error, info, instrument, warn};

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
    #[instrument(skip(self), fields(compose_file = %self.compose_file.display()))]
    pub fn up(&self) -> Result<()> {
        debug!(compose_file = %self.compose_file.display(), "writing compose file");
        fs::write(&self.compose_file, &self.compose_content).with_context(|| {
            format!(
                "failed to write compose file: {}",
                self.compose_file.display()
            )
        })?;

        // Clean up stale networks/containers from a previous run so `up` doesn't
        // fail with "network already exists".
        debug!("running pre-up cleanup (docker compose down)");
        let _ = self.base_command().args(["down"]).output();

        info!("running docker compose up");
        let output = self
            .base_command()
            .args(["up", "-d", "--wait"])
            .output()
            .context("failed to run docker compose up")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, "docker compose up failed");
            bail!("docker compose up failed: {stderr}");
        }

        info!("docker compose up succeeded");
        Ok(())
    }

    /// Stop and remove server containers/networks via `docker compose down`.
    ///
    /// Removes the compose file after a successful teardown.
    #[instrument(skip(self), fields(compose_file = %self.compose_file.display()))]
    pub fn down(&self) -> Result<()> {
        if !self.compose_file.exists() {
            warn!(compose_file = %self.compose_file.display(), "compose file not found");
            bail!(
                "compose file not found: {} — is the server running?",
                self.compose_file.display()
            );
        }

        info!("running docker compose down");
        let output = self
            .base_command()
            .args(["down"])
            .output()
            .context("failed to run docker compose down")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, "docker compose down failed");
            bail!("docker compose down failed: {stderr}");
        }

        // Clean up the rendered compose file
        let _ = fs::remove_file(&self.compose_file);
        info!("docker compose down succeeded");

        Ok(())
    }

    /// Check if the server service is running via `docker compose ps`.
    ///
    /// Returns `true` if at least one container for the server service is running.
    #[instrument(skip(self))]
    pub fn is_running(&self) -> Result<bool> {
        if !self.compose_file.exists() {
            debug!("compose file does not exist, server is not running");
            return Ok(false);
        }

        let output = self
            .base_command()
            .args(["ps", "--status", "running", "--format", "{{.Name}}"])
            .output()
            .context("failed to run docker compose ps")?;

        if !output.status.success() {
            // compose ps can fail if the project was never started; treat as not running
            debug!("docker compose ps failed, treating as not running");
            return Ok(false);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let running = !stdout.trim().is_empty();
        debug!(running, "compose service status");
        Ok(running)
    }
}

/// Render the compose template with resolved network and container names.
#[instrument(fields(network_name = %network.name, worker_network = %network.worker_name))]
pub fn render_compose(
    network: &ur_config::NetworkConfig,
    proxy: &ur_config::ProxyConfig,
    rag: &ur_config::RagConfig,
) -> String {
    COMPOSE_TEMPLATE
        .replace("{{NETWORK_NAME}}", &network.name)
        .replace("{{WORKER_NETWORK_NAME}}", &network.worker_name)
        .replace("{{SERVER_CONTAINER_NAME}}", &network.server_hostname)
        .replace("{{SQUID_CONTAINER_NAME}}", &proxy.hostname)
        .replace("{{QDRANT_CONTAINER_NAME}}", &rag.qdrant_hostname)
}

/// Build a `ComposeManager` from the resolved ur config.
///
/// Forwards `UR_CONFIG`, `UR_WORKSPACE`, `UR_SERVER_PORT`, and `UR_HOSTD_PORT`
/// as environment variables so the compose file's variable interpolation picks them up.
#[instrument(skip(config), fields(compose_file = %config.compose_file.display()))]
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
        ("UR_HOSTD_PORT".to_string(), config.hostd_port.to_string()),
    ];

    // Forward UR_CONTAINER if set so compose can potentially use it
    if let Ok(val) = std::env::var("UR_CONTAINER") {
        env_vars.push(("UR_CONTAINER".to_string(), val));
    }

    let compose_content = render_compose(&config.network, &config.proxy, &config.rag);

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
                agent_prefix: ur_config::DEFAULT_AGENT_PREFIX.to_string(),
            },
            hostd_port: ur_config::DEFAULT_HOSTD_PORT,
            hostexec: ur_config::HostExecConfig::default(),
            rag: ur_config::RagConfig {
                qdrant_hostname: ur_config::DEFAULT_QDRANT_HOSTNAME.to_string(),
                embedding_model: ur_config::DEFAULT_EMBEDDING_MODEL.to_string(),
                docs: ur_config::RagDocsConfig::default(),
            },
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
            projects: std::collections::HashMap::new(),
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
            agent_prefix: "test-agent-".to_string(),
        };
        let proxy = ur_config::ProxyConfig {
            hostname: "test-squid".to_string(),
            allowlist: vec![],
        };
        let rag = ur_config::RagConfig {
            qdrant_hostname: "test-qdrant".to_string(),
            embedding_model: ur_config::DEFAULT_EMBEDDING_MODEL.to_string(),
            docs: ur_config::RagDocsConfig::default(),
        };
        let rendered = render_compose(&network, &proxy, &rag);
        assert!(rendered.contains("name: test-net"));
        assert!(rendered.contains("name: test-workers"));
        assert!(rendered.contains("container_name: test-server"));
        assert!(rendered.contains("container_name: test-squid"));
        assert!(rendered.contains("container_name: test-qdrant"));
        assert!(!rendered.contains("{{NETWORK_NAME}}"));
        assert!(!rendered.contains("{{WORKER_NETWORK_NAME}}"));
        assert!(!rendered.contains("{{SERVER_CONTAINER_NAME}}"));
        assert!(!rendered.contains("{{SQUID_CONTAINER_NAME}}"));
        assert!(!rendered.contains("{{QDRANT_CONTAINER_NAME}}"));
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
