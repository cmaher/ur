use std::fmt::Write as _;
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

/// Parameters for generating a compose file, extracted from the ur config.
struct ComposeParams {
    server_container_name: String,
    squid_container_name: String,
    postgres_container_name: String,
    infra_network_name: String,
    worker_network_name: String,
    /// Host-side backup path, if configured. Mounted at `/backup` in the postgres container.
    backup_path: Option<PathBuf>,
    /// Postgres user for env vars and healthcheck.
    db_user: String,
    /// Postgres password for env vars.
    db_password: String,
    /// Postgres database name for env vars.
    db_name: String,
    /// Full DATABASE_URL for the server service.
    database_url: String,
    /// Whether to include the postgres service in the compose file.
    /// False when connecting to a remote database (host != DEFAULT_DB_HOST).
    include_postgres: bool,
    /// Network interface to bind the postgres container port on.
    postgres_bind_address: Option<String>,
    /// Postgres port for bind_address exposure.
    postgres_port: u16,
}

/// Generate the docker compose YAML programmatically.
///
/// Produces the same network topology and service configuration that the old static
/// template provided: ur-squid, ur-qdrant, and ur-server services on infra + workers
/// networks, with the same volumes, env vars, healthchecks, and ports.
#[instrument(fields(network_name = %network.name, worker_network = %network.worker_name))]
pub fn generate_compose(
    network: &ur_config::NetworkConfig,
    proxy: &ur_config::ProxyConfig,
    db: &ur_config::DatabaseConfig,
) -> String {
    // Compose service names are bare hostnames (no dots). External addresses
    // (IPs, FQDNs) always contain dots. Use this to decide whether postgres
    // should be included as a compose-managed service.
    let include_postgres = !db.host.contains('.');
    let params = ComposeParams {
        server_container_name: network.server_hostname.clone(),
        squid_container_name: proxy.hostname.clone(),
        postgres_container_name: db.host.clone(),
        infra_network_name: network.name.clone(),
        worker_network_name: network.worker_name.clone(),
        backup_path: if db.backup.enabled {
            db.backup.path.clone()
        } else {
            None
        },
        db_user: db.user.clone(),
        db_password: db.password.clone(),
        db_name: db.name.clone(),
        database_url: db.database_url(),
        include_postgres,
        postgres_bind_address: db.bind_address.clone(),
        postgres_port: db.port,
    };

    let mut out = String::with_capacity(2048);

    write_header(&mut out);
    writeln!(out, "services:").unwrap();
    write_squid_service(&mut out, &params);
    if params.include_postgres {
        writeln!(out).unwrap();
        write_postgres_service(&mut out, &params);
    }
    writeln!(out).unwrap();
    write_server_service(&mut out, &params);
    writeln!(out).unwrap();
    write_networks(&mut out, &params);

    out
}

fn write_header(out: &mut String) {
    writeln!(
        out,
        "# Auto-generated by `ur start`. Do not edit — changes will be overwritten."
    )
    .unwrap();
    writeln!(out).unwrap();
}

fn write_squid_service(out: &mut String, params: &ComposeParams) {
    writeln!(out, "  ur-squid:").unwrap();
    writeln!(out, "    image: ur-squid:${{UR_IMAGE_TAG:-latest}}").unwrap();
    writeln!(out, "    container_name: {}", params.squid_container_name).unwrap();
    writeln!(out, "    volumes:").unwrap();
    writeln!(
        out,
        "      - ${{UR_CONFIG:-~/.ur}}/squid/allowlist.txt:/etc/squid/allowlist.txt:ro"
    )
    .unwrap();
    writeln!(out, "    networks:").unwrap();
    writeln!(out, "      - infra").unwrap();
    writeln!(out, "      - workers").unwrap();
    writeln!(out, "    restart: unless-stopped").unwrap();
}

fn write_postgres_service(out: &mut String, params: &ComposeParams) {
    writeln!(out, "  {}:", params.postgres_container_name).unwrap();
    writeln!(out, "    image: postgres:17-alpine").unwrap();
    writeln!(
        out,
        "    container_name: {}",
        params.postgres_container_name
    )
    .unwrap();
    writeln!(out, "    restart: unless-stopped").unwrap();

    // Volumes
    writeln!(out, "    volumes:").unwrap();
    writeln!(
        out,
        "      - ${{UR_CONFIG:-~/.ur}}/postgres:/var/lib/postgresql/data"
    )
    .unwrap();
    if let Some(backup_path) = &params.backup_path {
        writeln!(
            out,
            "      - {}:{}",
            backup_path.display(),
            ur_config::BACKUP_CONTAINER_PATH,
        )
        .unwrap();
    }

    // Environment
    writeln!(out, "    environment:").unwrap();
    writeln!(out, "      - POSTGRES_USER={}", params.db_user).unwrap();
    writeln!(out, "      - POSTGRES_PASSWORD={}", params.db_password).unwrap();
    writeln!(out, "      - POSTGRES_DB={}", params.db_name).unwrap();

    // Healthcheck
    writeln!(out, "    healthcheck:").unwrap();
    writeln!(
        out,
        "      test: [\"CMD-SHELL\", \"pg_isready -U {}\"]",
        params.db_user
    )
    .unwrap();
    writeln!(out, "      interval: 1s").unwrap();
    writeln!(out, "      timeout: 2s").unwrap();
    writeln!(out, "      retries: 10").unwrap();
    writeln!(out, "      start_period: 3s").unwrap();

    // Ports (only when bind_address is configured)
    if let Some(addr) = &params.postgres_bind_address {
        writeln!(out, "    ports:").unwrap();
        writeln!(
            out,
            "      - \"{addr}:{port}:{port}\"",
            addr = addr,
            port = params.postgres_port,
        )
        .unwrap();
    }

    // Networks
    writeln!(out, "    networks:").unwrap();
    writeln!(out, "      - infra").unwrap();
}

fn write_server_service(out: &mut String, params: &ComposeParams) {
    writeln!(out, "  ur-server:").unwrap();
    writeln!(out, "    image: ur-server:${{UR_IMAGE_TAG:-latest}}").unwrap();
    writeln!(out, "    container_name: {}", params.server_container_name).unwrap();
    writeln!(out, "    restart: unless-stopped").unwrap();

    // Depends on (only when local postgres is included)
    if params.include_postgres {
        writeln!(out, "    depends_on:").unwrap();
        writeln!(out, "      {}:", params.postgres_container_name).unwrap();
        writeln!(out, "        condition: service_healthy").unwrap();
    }

    // Volumes
    writeln!(out, "    volumes:").unwrap();
    writeln!(out, "      - /var/run/docker.sock:/var/run/docker.sock").unwrap();
    writeln!(out, "      - ${{UR_CONFIG:-~/.ur}}:/config").unwrap();
    writeln!(out, "      - ${{UR_WORKSPACE:-~/.ur/workspace}}:/workspace").unwrap();
    writeln!(out, "      - ${{UR_LOGS_DIR:-~/.ur/logs}}:/logs").unwrap();

    // Environment
    writeln!(out, "    environment:").unwrap();
    writeln!(out, "      - UR_CONFIG=/config").unwrap();
    writeln!(out, "      - UR_HOST_CONFIG=${{UR_CONFIG:-${{HOME}}/.ur}}").unwrap();
    writeln!(
        out,
        "      - UR_HOST_WORKSPACE=${{UR_WORKSPACE:-${{HOME}}/.ur/workspace}}"
    )
    .unwrap();
    writeln!(
        out,
        "      - UR_HOST_LOGS_DIR=${{UR_LOGS_DIR:-${{HOME}}/.ur/logs}}"
    )
    .unwrap();
    writeln!(out, "      - DATABASE_URL={}", params.database_url).unwrap();
    if params.backup_path.is_some() {
        writeln!(
            out,
            "      - UR_BACKUP_PATH={}",
            ur_config::BACKUP_CONTAINER_PATH,
        )
        .unwrap();
    }
    writeln!(
        out,
        "      - UR_BUILDERD_ADDR=http://host.docker.internal:${{UR_BUILDERD_PORT:-12323}}"
    )
    .unwrap();
    writeln!(out, "      - GH_TOKEN=${{GH_TOKEN:-}}").unwrap();
    writeln!(out, "      - GITHUB_TOKEN=${{GITHUB_TOKEN:-}}").unwrap();

    // Extra hosts
    writeln!(out, "    extra_hosts:").unwrap();
    writeln!(out, "      - \"host.docker.internal:host-gateway\"").unwrap();

    // Healthcheck
    writeln!(out, "    healthcheck:").unwrap();
    writeln!(
        out,
        "      test: [\"CMD-SHELL\", \"nc -z 127.0.0.1 ${{UR_SERVER_PORT:-12321}} || exit 1\"]"
    )
    .unwrap();
    writeln!(out, "      interval: 1s").unwrap();
    writeln!(out, "      timeout: 2s").unwrap();
    writeln!(out, "      retries: 10").unwrap();
    writeln!(out, "      start_period: 3s").unwrap();

    // Ports
    writeln!(out, "    ports:").unwrap();
    writeln!(
        out,
        "      - \"${{UR_SERVER_PORT:-12321}}:${{UR_SERVER_PORT:-12321}}\""
    )
    .unwrap();

    // Networks
    writeln!(out, "    networks:").unwrap();
    writeln!(out, "      - infra").unwrap();
    writeln!(out, "      - workers").unwrap();
}

fn write_networks(out: &mut String, params: &ComposeParams) {
    writeln!(out, "networks:").unwrap();
    writeln!(out, "  infra:").unwrap();
    writeln!(out, "    driver: bridge").unwrap();
    writeln!(out, "    name: {}", params.infra_network_name).unwrap();
    writeln!(out, "  workers:").unwrap();
    writeln!(out, "    driver: bridge").unwrap();
    writeln!(out, "    name: {}", params.worker_network_name).unwrap();
    writeln!(out, "    internal: true").unwrap();
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

    let compose_content = generate_compose(&config.network, &config.proxy, &config.db);

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
            node_id: "test-node".to_string(),
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
                bind_address: None,
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
    fn generate_compose_contains_all_services() {
        let network = ur_config::NetworkConfig {
            name: "test-net".to_string(),
            worker_name: "test-workers".to_string(),
            server_hostname: "test-server".to_string(),
            worker_prefix: "test-worker-".to_string(),
        };
        let proxy = ur_config::ProxyConfig {
            hostname: "test-squid".to_string(),
            allowlist: vec![],
        };
        let db = ur_config::DatabaseConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_DB_NAME.to_string(),
            bind_address: None,
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
        };
        let generated = generate_compose(&network, &proxy, &db);

        // Verify services are present
        assert!(generated.contains("  ur-squid:"));
        assert!(generated.contains("  ur-server:"));
        assert!(generated.contains("  ur-postgres:"));

        // Verify container names
        assert!(generated.contains("container_name: test-server"));
        assert!(generated.contains("container_name: test-squid"));
        assert!(
            generated.contains("container_name: ur-postgres"),
            "postgres container_name should match db.host"
        );

        // Verify postgres image
        assert!(generated.contains("image: postgres:17-alpine"));

        // Verify postgres data volume
        assert!(generated.contains("/postgres:/var/lib/postgresql/data"));

        // Verify postgres env vars
        assert!(generated.contains("POSTGRES_USER=ur"));
        assert!(generated.contains("POSTGRES_PASSWORD=ur"));
        assert!(generated.contains("POSTGRES_DB=ur"));

        // Verify postgres healthcheck
        assert!(generated.contains("pg_isready -U ur"));

        // Verify server depends_on postgres
        assert!(generated.contains("depends_on:"));
        assert!(generated.contains("condition: service_healthy"));

        // Verify server has DATABASE_URL
        assert!(generated.contains("DATABASE_URL=postgres://ur:ur@ur-postgres:5432/ur"));

        // Verify network names
        assert!(generated.contains("name: test-net"));
        assert!(generated.contains("name: test-workers"));
        assert!(generated.contains("internal: true"));

        // Verify key server configuration
        assert!(generated.contains("/var/run/docker.sock:/var/run/docker.sock"));
        assert!(generated.contains("UR_CONFIG=/config"));
        assert!(generated.contains("host.docker.internal:host-gateway"));
        assert!(generated.contains("nc -z 127.0.0.1"));
        assert!(generated.contains("interval: 1s"));
        assert!(generated.contains("retries: 10"));

        // Verify logs volume mount and env var
        assert!(generated.contains("${UR_LOGS_DIR:-~/.ur/logs}:/logs"));
        assert!(generated.contains("UR_HOST_LOGS_DIR="));

        // Verify squid volume
        assert!(generated.contains("allowlist.txt:/etc/squid/allowlist.txt:ro"));

        // Verify networks section
        assert!(generated.contains("networks:"));
        assert!(generated.contains("driver: bridge"));

        // Verify server does NOT mount backup volume
        // (backup is on postgres container, not server)
        let server_section = generated
            .split("  ur-server:")
            .nth(1)
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        assert!(!server_section.contains("/backup"));
    }

    #[test]
    fn generate_compose_is_valid_yaml_structure() {
        let network = ur_config::NetworkConfig {
            name: "ur".to_string(),
            worker_name: "ur-workers".to_string(),
            server_hostname: "ur-server".to_string(),
            worker_prefix: "ur-worker-".to_string(),
        };
        let proxy = ur_config::ProxyConfig {
            hostname: "ur-squid".to_string(),
            allowlist: vec![],
        };
        let db = ur_config::DatabaseConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_DB_NAME.to_string(),
            bind_address: None,
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
        };
        let generated = generate_compose(&network, &proxy, &db);

        // Verify top-level structure: starts with comment, then services, then networks
        assert!(generated.starts_with("# Auto-generated"));
        assert!(generated.contains("\nservices:\n"));
        assert!(generated.contains("\nnetworks:\n"));

        // Verify services come before networks
        let services_pos = generated.find("services:").unwrap();
        let networks_pos = generated.rfind("networks:").unwrap();
        assert!(services_pos < networks_pos);
    }

    #[test]
    fn generate_compose_backup_on_postgres_not_server() {
        let network = ur_config::NetworkConfig {
            name: "ur".to_string(),
            worker_name: "ur-workers".to_string(),
            server_hostname: "ur-server".to_string(),
            worker_prefix: "ur-worker-".to_string(),
        };
        let proxy = ur_config::ProxyConfig {
            hostname: "ur-squid".to_string(),
            allowlist: vec![],
        };
        let db = ur_config::DatabaseConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_DB_NAME.to_string(),
            bind_address: None,
            backup: ur_config::BackupConfig {
                path: Some(PathBuf::from("/home/user/.ur/backup")),
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
        };
        let generated = generate_compose(&network, &proxy, &db);

        // Backup volume should be on postgres container
        let postgres_section = generated
            .split("  ur-postgres:")
            .nth(1)
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        assert!(postgres_section.contains("/home/user/.ur/backup:/backup"));

        // Backup volume mount should NOT be on server container
        let server_section = generated
            .split("  ur-server:")
            .nth(1)
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        assert!(!server_section.contains("/home/user/.ur/backup:/backup"));

        // Postgres should only be on infra network
        assert!(postgres_section.contains("- infra"));
        assert!(!postgres_section.contains("- workers"));
    }

    #[test]
    fn generate_compose_postgres_on_infra_only() {
        let network = ur_config::NetworkConfig {
            name: "ur".to_string(),
            worker_name: "ur-workers".to_string(),
            server_hostname: "ur-server".to_string(),
            worker_prefix: "ur-worker-".to_string(),
        };
        let proxy = ur_config::ProxyConfig {
            hostname: "ur-squid".to_string(),
            allowlist: vec![],
        };
        let db = ur_config::DatabaseConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_DB_NAME.to_string(),
            bind_address: None,
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
        };
        let generated = generate_compose(&network, &proxy, &db);

        // Extract postgres section and verify it only has infra network
        let postgres_section = generated
            .split("  ur-postgres:")
            .nth(1)
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        assert!(postgres_section.contains("- infra"));
        // Postgres should not be on workers network
        assert!(!postgres_section.contains("- workers"));
    }

    #[test]
    fn generate_compose_bind_address_exposes_postgres_port() {
        let network = ur_config::NetworkConfig {
            name: "ur".to_string(),
            worker_name: "ur-workers".to_string(),
            server_hostname: "ur-server".to_string(),
            worker_prefix: "ur-worker-".to_string(),
        };
        let proxy = ur_config::ProxyConfig {
            hostname: "ur-squid".to_string(),
            allowlist: vec![],
        };
        let db = ur_config::DatabaseConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_DB_NAME.to_string(),
            bind_address: Some("100.64.1.5".to_string()),
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
        };
        let generated = generate_compose(&network, &proxy, &db);

        // Postgres should be present with ports exposed
        assert!(generated.contains("  ur-postgres:"));
        assert!(generated.contains("\"100.64.1.5:5432:5432\""));
        // Server should depend on postgres
        assert!(generated.contains("depends_on:"));
    }

    #[test]
    fn generate_compose_remote_host_skips_postgres() {
        let network = ur_config::NetworkConfig {
            name: "ur".to_string(),
            worker_name: "ur-workers".to_string(),
            server_hostname: "ur-server".to_string(),
            worker_prefix: "ur-worker-".to_string(),
        };
        let proxy = ur_config::ProxyConfig {
            hostname: "ur-squid".to_string(),
            allowlist: vec![],
        };
        let db = ur_config::DatabaseConfig {
            host: "192.168.1.50".to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_DB_NAME.to_string(),
            bind_address: None,
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
        };
        let generated = generate_compose(&network, &proxy, &db);

        // No postgres service
        assert!(!generated.contains("  192.168.1.50:"));
        assert!(!generated.contains("image: postgres"));
        // No depends_on on server
        assert!(!generated.contains("depends_on:"));
        // DATABASE_URL uses remote host
        assert!(generated.contains("DATABASE_URL=postgres://ur:ur@192.168.1.50:5432/ur"));
        // Server and squid still present
        assert!(generated.contains("  ur-server:"));
        assert!(generated.contains("  ur-squid:"));
    }

    #[test]
    fn generate_compose_default_no_ports_on_postgres() {
        let network = ur_config::NetworkConfig {
            name: "ur".to_string(),
            worker_name: "ur-workers".to_string(),
            server_hostname: "ur-server".to_string(),
            worker_prefix: "ur-worker-".to_string(),
        };
        let proxy = ur_config::ProxyConfig {
            hostname: "ur-squid".to_string(),
            allowlist: vec![],
        };
        let db = ur_config::DatabaseConfig {
            host: ur_config::DEFAULT_DB_HOST.to_string(),
            port: ur_config::DEFAULT_DB_PORT,
            user: ur_config::DEFAULT_DB_USER.to_string(),
            password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
            name: ur_config::DEFAULT_DB_NAME.to_string(),
            bind_address: None,
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
        };
        let generated = generate_compose(&network, &proxy, &db);

        // Postgres present, no ports section on it
        assert!(generated.contains("  ur-postgres:"));
        let postgres_section = generated
            .split("  ur-postgres:")
            .nth(1)
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        assert!(!postgres_section.contains("ports:"));
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
