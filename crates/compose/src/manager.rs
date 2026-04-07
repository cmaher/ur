use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tracing::{debug, error, info, instrument, warn};

/// Manages server lifecycle via Docker Compose.
///
/// Wraps `docker compose` CLI commands targeting a programmatically generated compose file.
/// The compose file is written on `up()` and removed on `down()`.
#[derive(Debug, Clone)]
pub struct ComposeManager {
    compose_file: PathBuf,
    /// Environment variables passed to `docker compose` (forwarded to the compose file's
    /// variable interpolation, e.g. `${UR_SERVER_PORT}`, `${UR_CONFIG}`).
    env_vars: Vec<(String, String)>,
    /// Generated compose file content.
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
    /// Generates and writes the compose file before invoking docker compose.
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

    /// Ensure the compose file exists (write it if missing), then force-recreate a single service.
    #[instrument(skip(self), fields(service, compose_file = %self.compose_file.display()))]
    pub fn recreate_service(&self, service: &str) -> Result<()> {
        if !self.compose_file.exists() {
            debug!("compose file missing, writing before recreate");
            fs::write(&self.compose_file, &self.compose_content).with_context(|| {
                format!(
                    "failed to write compose file: {}",
                    self.compose_file.display()
                )
            })?;
        }

        info!(service, "force-recreating compose service");
        let output = self
            .base_command()
            .args(["up", "-d", "--force-recreate", service])
            .output()
            .context("failed to run docker compose up")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(service, stderr = %stderr, "docker compose recreate failed");
            bail!("docker compose recreate {service} failed: {stderr}");
        }

        info!(service, "service recreated");
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

/// Build a `ComposeManager` from the resolved ur config.
///
/// Forwards `UR_CONFIG`, `UR_WORKSPACE`, `UR_SERVER_PORT`, `UR_BUILDERD_PORT`,
/// and optionally `UR_IMAGE_TAG` and `UR_CONTAINER` as environment variables
/// so the compose file's variable interpolation picks them up.
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
        ("UR_SERVER_PORT".to_string(), config.server_port.to_string()),
        (
            "UR_BUILDERD_PORT".to_string(),
            config.builderd_port.to_string(),
        ),
    ];

    // Forward UR_CONTAINER if set so compose can potentially use it
    if let Ok(val) = std::env::var("UR_CONTAINER") {
        env_vars.push(("UR_CONTAINER".to_string(), val));
    }

    // Forward UR_IMAGE_TAG if set so CI-tagged images are used by compose
    if let Ok(val) = std::env::var("UR_IMAGE_TAG") {
        env_vars.push(("UR_IMAGE_TAG".to_string(), val));
    }

    env_vars.push((
        "UR_LOGS_DIR".to_string(),
        config.logs_dir.to_string_lossy().into_owned(),
    ));

    let compose_file = crate::ComposeFile::base(&config.network, &config.proxy, &config.db);
    let compose_content = compose_file.render();

    ComposeManager::new(config.compose_file.clone(), env_vars, compose_content)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            logs_dir: PathBuf::from("/test/config/logs"),
            workspace: PathBuf::from("/test/workspace"),
            server_port: 9999,
            compose_file: PathBuf::from("/test/docker-compose.yml"),
            proxy: ur_config::ProxyConfig {
                hostname: ur_config::DEFAULT_PROXY_HOSTNAME.to_string(),
                allowlist: vec![],
            },
            network: ur_config::NetworkConfig {
                name: "ur".to_string(),
                worker_name: "ur-workers".to_string(),
                server_hostname: "ur-server".to_string(),
                worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.to_string(),
            },
            worker_port: 10000,
            builderd_port: ur_config::DEFAULT_BUILDERD_PORT,
            hostexec: ur_config::HostExecConfig::default(),
            db: ur_config::DatabaseConfig {
                host: ur_config::DEFAULT_DB_HOST.to_string(),
                port: ur_config::DEFAULT_DB_PORT,
                user: ur_config::DEFAULT_DB_USER.to_string(),
                password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
                name: ur_config::DEFAULT_DB_NAME.to_string(),
                backup: ur_config::BackupConfig {
                    path: None,
                    interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                    enabled: true,
                    retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
                },
            },
            git_branch_prefix: String::new(),
            server: ur_config::ServerConfig {
                container_command: "docker".into(),
                stale_worker_ttl_days: 7,
                max_implement_cycles: Some(6),
                poll_interval_ms: 500,
                github_scan_interval_secs: 30,
                builderd_retry_count: ur_config::DEFAULT_BUILDERD_RETRY_COUNT,
                builderd_retry_backoff_ms: ur_config::DEFAULT_BUILDERD_RETRY_BACKOFF_MS,
                ui_event_fallback_interval_ms: ur_config::DEFAULT_UI_EVENT_FALLBACK_INTERVAL_MS,
            },
            projects: std::collections::HashMap::new(),
            tui: ur_config::TuiConfig::default(),
            plugins: std::collections::HashMap::new(),
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
        assert!(
            manager
                .env_vars
                .contains(&("UR_LOGS_DIR".to_string(), "/test/config/logs".to_string()))
        );
    }

    #[test]
    fn up_writes_compose_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let compose_path = tmp.path().join("docker-compose.yml");
        let content = "services: {}".to_string();
        let manager = ComposeManager::new(compose_path.clone(), vec![], content.clone());

        // up() will fail on docker compose, but should still write the file
        let _ = manager.up();
        assert!(compose_path.exists());
        assert_eq!(fs::read_to_string(&compose_path).unwrap(), content);
    }
}
