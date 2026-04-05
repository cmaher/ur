use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level Docker Compose file representation.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ComposeFile {
    pub services: IndexMap<String, Service>,
    pub networks: IndexMap<String, Network>,
}

/// A Docker Compose service definition.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Service {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<IndexMap<String, DependsOn>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volumes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_hosts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthcheck: Option<Healthcheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub networks: Option<Vec<String>>,
}

/// Dependency condition for depends_on.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DependsOn {
    pub condition: String,
}

/// Docker Compose healthcheck configuration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Healthcheck {
    pub test: Vec<String>,
    pub interval: String,
    pub timeout: String,
    pub retries: u32,
    pub start_period: String,
}

/// Docker Compose network definition.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Network {
    pub driver: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal: Option<bool>,
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
}

impl ComposeFile {
    /// Produce the base compose configuration for ur infrastructure services.
    ///
    /// Produces the same network topology and service configuration that the old static
    /// template provided: ur-squid, ur-postgres, and ur-server services on infra + workers
    /// networks, with the same volumes, env vars, healthchecks, and ports.
    pub fn base(
        network: &ur_config::NetworkConfig,
        proxy: &ur_config::ProxyConfig,
        db: &ur_config::DatabaseConfig,
    ) -> Self {
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
        };

        let mut services = IndexMap::new();
        services.insert("ur-squid".to_string(), build_squid_service(&params));
        services.insert(
            params.postgres_container_name.clone(),
            build_postgres_service(&params),
        );
        services.insert("ur-server".to_string(), build_server_service(&params));

        let mut networks = IndexMap::new();
        networks.insert(
            "infra".to_string(),
            Network {
                driver: "bridge".to_string(),
                name: params.infra_network_name,
                internal: None,
            },
        );
        networks.insert(
            "workers".to_string(),
            Network {
                driver: "bridge".to_string(),
                name: params.worker_network_name,
                internal: Some(true),
            },
        );

        ComposeFile { services, networks }
    }

    /// Serialize to YAML with the auto-generated header comment.
    pub fn render(&self) -> String {
        let yaml = serde_yaml::to_string(self).expect("ComposeFile should serialize to YAML");
        format!(
            "# Auto-generated by `ur start`. Do not edit — changes will be overwritten.\n\n{yaml}"
        )
    }
}

fn build_squid_service(params: &ComposeParams) -> Service {
    Service {
        image: Some("ur-squid:${UR_IMAGE_TAG:-latest}".to_string()),
        container_name: Some(params.squid_container_name.clone()),
        volumes: Some(vec![
            "${UR_CONFIG:-~/.ur}/squid/allowlist.txt:/etc/squid/allowlist.txt:ro".to_string(),
        ]),
        networks: Some(vec!["infra".to_string(), "workers".to_string()]),
        restart: Some("unless-stopped".to_string()),
        ..Default::default()
    }
}

fn build_postgres_service(params: &ComposeParams) -> Service {
    let mut volumes = vec!["${UR_CONFIG:-~/.ur}/postgres:/var/lib/postgresql/data".to_string()];
    if let Some(backup_path) = &params.backup_path {
        volumes.push(format!(
            "{}:{}",
            backup_path.display(),
            ur_config::BACKUP_CONTAINER_PATH,
        ));
    }

    Service {
        image: Some("postgres:17-alpine".to_string()),
        container_name: Some(params.postgres_container_name.clone()),
        restart: Some("unless-stopped".to_string()),
        volumes: Some(volumes),
        environment: Some(vec![
            format!("POSTGRES_USER={}", params.db_user),
            format!("POSTGRES_PASSWORD={}", params.db_password),
            format!("POSTGRES_DB={}", params.db_name),
        ]),
        healthcheck: Some(Healthcheck {
            test: vec![
                "CMD-SHELL".to_string(),
                format!("pg_isready -U {}", params.db_user),
            ],
            interval: "1s".to_string(),
            timeout: "2s".to_string(),
            retries: 10,
            start_period: "3s".to_string(),
        }),
        networks: Some(vec!["infra".to_string()]),
        ..Default::default()
    }
}

fn build_server_service(params: &ComposeParams) -> Service {
    let mut depends_on = IndexMap::new();
    depends_on.insert(
        params.postgres_container_name.clone(),
        DependsOn {
            condition: "service_healthy".to_string(),
        },
    );

    let mut environment = vec![
        "UR_CONFIG=/config".to_string(),
        "UR_HOST_CONFIG=${UR_CONFIG:-${HOME}/.ur}".to_string(),
        "UR_HOST_WORKSPACE=${UR_WORKSPACE:-${HOME}/.ur/workspace}".to_string(),
        "UR_HOST_LOGS_DIR=${UR_LOGS_DIR:-${HOME}/.ur/logs}".to_string(),
        format!("DATABASE_URL={}", params.database_url),
    ];
    if params.backup_path.is_some() {
        environment.push(format!(
            "UR_BACKUP_PATH={}",
            ur_config::BACKUP_CONTAINER_PATH,
        ));
    }
    environment.push(
        "UR_BUILDERD_ADDR=http://host.docker.internal:${UR_BUILDERD_PORT:-42070}".to_string(),
    );
    environment.push("GH_TOKEN=${GH_TOKEN:-}".to_string());
    environment.push("GITHUB_TOKEN=${GITHUB_TOKEN:-}".to_string());

    Service {
        image: Some("ur-server:${UR_IMAGE_TAG:-latest}".to_string()),
        container_name: Some(params.server_container_name.clone()),
        restart: Some("unless-stopped".to_string()),
        depends_on: Some(depends_on),
        volumes: Some(vec![
            "/var/run/docker.sock:/var/run/docker.sock".to_string(),
            "${UR_CONFIG:-~/.ur}:/config".to_string(),
            "${UR_WORKSPACE:-~/.ur/workspace}:/workspace".to_string(),
            "${UR_LOGS_DIR:-~/.ur/logs}:/logs".to_string(),
        ]),
        environment: Some(environment),
        extra_hosts: Some(vec!["host.docker.internal:host-gateway".to_string()]),
        healthcheck: Some(Healthcheck {
            test: vec![
                "CMD-SHELL".to_string(),
                "nc -z 127.0.0.1 ${UR_SERVER_PORT:-42069} || exit 1".to_string(),
            ],
            interval: "1s".to_string(),
            timeout: "2s".to_string(),
            retries: 10,
            start_period: "3s".to_string(),
        }),
        ports: Some(vec![
            "${UR_SERVER_PORT:-42069}:${UR_SERVER_PORT:-42069}".to_string(),
        ]),
        networks: Some(vec!["infra".to_string(), "workers".to_string()]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_network() -> ur_config::NetworkConfig {
        ur_config::NetworkConfig {
            name: "test-net".to_string(),
            worker_name: "test-workers".to_string(),
            server_hostname: "test-server".to_string(),
            worker_prefix: "test-worker-".to_string(),
        }
    }

    fn test_proxy() -> ur_config::ProxyConfig {
        ur_config::ProxyConfig {
            hostname: "test-squid".to_string(),
            allowlist: vec![],
        }
    }

    fn test_db() -> ur_config::DatabaseConfig {
        ur_config::DatabaseConfig {
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
        }
    }

    #[test]
    fn base_produces_all_services() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        assert!(compose.services.contains_key("ur-squid"));
        assert!(compose.services.contains_key("ur-server"));
        assert!(compose.services.contains_key("ur-postgres"));
    }

    #[test]
    fn render_contains_all_services() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let rendered = compose.render();

        assert!(rendered.contains("ur-squid:"));
        assert!(rendered.contains("ur-server:"));
        assert!(rendered.contains("ur-postgres:"));
    }

    #[test]
    fn render_starts_with_header() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let rendered = compose.render();
        assert!(rendered.starts_with("# Auto-generated"));
    }

    #[test]
    fn render_contains_container_names() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let rendered = compose.render();

        assert!(rendered.contains("container_name: test-server"));
        assert!(rendered.contains("container_name: test-squid"));
        assert!(rendered.contains("container_name: ur-postgres"));
    }

    #[test]
    fn render_contains_postgres_config() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let rendered = compose.render();

        assert!(rendered.contains("image: postgres:17-alpine"));
        assert!(rendered.contains("/postgres:/var/lib/postgresql/data"));
        assert!(rendered.contains("POSTGRES_USER=ur"));
        assert!(rendered.contains("POSTGRES_PASSWORD=ur"));
        assert!(rendered.contains("POSTGRES_DB=ur"));
        assert!(rendered.contains("pg_isready -U ur"));
    }

    #[test]
    fn render_contains_server_config() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let rendered = compose.render();

        assert!(rendered.contains("condition: service_healthy"));
        assert!(rendered.contains("DATABASE_URL=postgres://ur:ur@ur-postgres:5432/ur"));
        assert!(rendered.contains("/var/run/docker.sock:/var/run/docker.sock"));
        assert!(rendered.contains("UR_CONFIG=/config"));
        assert!(rendered.contains("host.docker.internal:host-gateway"));
        assert!(rendered.contains("nc -z 127.0.0.1"));
        assert!(rendered.contains("interval: 1s"));
        assert!(rendered.contains("retries: 10"));
    }

    #[test]
    fn render_contains_network_names() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let rendered = compose.render();

        assert!(rendered.contains("name: test-net"));
        assert!(rendered.contains("name: test-workers"));
        assert!(rendered.contains("internal: true"));
        assert!(rendered.contains("driver: bridge"));
    }

    #[test]
    fn render_contains_squid_volume() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let rendered = compose.render();
        assert!(rendered.contains("allowlist.txt:/etc/squid/allowlist.txt:ro"));
    }

    #[test]
    fn render_contains_logs_volume() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let rendered = compose.render();
        assert!(rendered.contains("${UR_LOGS_DIR:-~/.ur/logs}:/logs"));
        assert!(rendered.contains("UR_HOST_LOGS_DIR="));
    }

    #[test]
    fn backup_on_postgres_not_server() {
        let db = ur_config::DatabaseConfig {
            backup: ur_config::BackupConfig {
                path: Some(PathBuf::from("/home/user/.ur/backup")),
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
            ..test_db()
        };
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &db);

        // Backup volume should be on postgres
        let postgres = compose.services.get("ur-postgres").unwrap();
        let pg_volumes = postgres.volumes.as_ref().unwrap();
        assert!(
            pg_volumes
                .iter()
                .any(|v| v.contains("/home/user/.ur/backup:/backup"))
        );

        // Backup volume should NOT be on server
        let server = compose.services.get("ur-server").unwrap();
        let srv_volumes = server.volumes.as_ref().unwrap();
        assert!(!srv_volumes.iter().any(|v| v.contains("/backup")));
    }

    #[test]
    fn postgres_on_infra_only() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let postgres = compose.services.get("ur-postgres").unwrap();
        let networks = postgres.networks.as_ref().unwrap();
        assert!(networks.contains(&"infra".to_string()));
        assert!(!networks.contains(&"workers".to_string()));
    }

    #[test]
    fn services_order_is_deterministic() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let keys: Vec<&String> = compose.services.keys().collect();
        assert_eq!(keys, vec!["ur-squid", "ur-postgres", "ur-server"]);
    }

    #[test]
    fn render_is_valid_yaml() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let rendered = compose.render();
        // Strip the comment header and parse
        let yaml_part = rendered
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&yaml_part).expect("rendered YAML should be parseable");
        assert!(parsed.get("services").is_some());
        assert!(parsed.get("networks").is_some());
    }

    #[test]
    fn roundtrip_serde() {
        let compose = ComposeFile::base(&test_network(), &test_proxy(), &test_db());
        let yaml = serde_yaml::to_string(&compose).unwrap();
        let parsed: ComposeFile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.services.len(), compose.services.len());
        assert_eq!(parsed.networks.len(), compose.networks.len());
    }
}
