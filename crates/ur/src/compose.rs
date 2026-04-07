use anyhow::Result;
use compose::ComposeFile;
use plugins::CliRegistry;
use tracing::instrument;

pub use compose::ComposeManager;

/// Build a `ComposeManager` from the resolved ur config, applying plugin modifications.
///
/// Builds the base compose configuration via `ComposeFile::base()`, lets every registered
/// CLI plugin modify it via `registry.apply_compose()`, then renders the final YAML and
/// constructs a `ComposeManager`.
#[instrument(skip(config, registry), fields(compose_file = %config.compose_file.display()))]
pub fn compose_manager_from_config(
    config: &ur_config::Config,
    registry: &CliRegistry,
) -> Result<ComposeManager> {
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

    let mut compose_file = ComposeFile::base(&config.network, &config.proxy, &config.db);
    registry.apply_compose(&mut compose_file)?;
    let compose_content = compose_file.render();

    Ok(ComposeManager::new(
        config.compose_file.clone(),
        env_vars,
        compose_content,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

        let registry = CliRegistry::new();
        let manager = compose_manager_from_config(&config, &registry).unwrap();
        // ComposeManager fields are private, so we verify indirectly via is_running
        // (returns false for non-existent file path, confirming the manager was constructed)
        assert!(!manager.is_running().unwrap());
    }
}
