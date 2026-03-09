use std::path::{Path, PathBuf};

use serde::Deserialize;

// ---- Environment variable names ----

/// Environment variable: override the config directory (default `~/.ur`).
pub const UR_CONFIG_ENV: &str = "UR_CONFIG";

/// Environment variable: `host:port` address for worker→server gRPC connections.
pub const UR_SERVER_ADDR_ENV: &str = "UR_SERVER_ADDR";

/// Environment variable: Claude credentials JSON blob injected into containers.
pub const CLAUDE_CREDENTIALS_ENV: &str = "CLAUDE_CREDENTIALS";

// ---- Defaults ----

/// Default TCP port for the server (ur→server communication).
pub const DEFAULT_DAEMON_PORT: u16 = 42069;

/// Default hostname for the Squid proxy container on the Docker network.
pub const DEFAULT_PROXY_HOSTNAME: &str = "ur-squid";

/// Squid listening port inside the container (standard Squid default).
pub const SQUID_PORT: u16 = 3128;

/// Default Docker network name for ur-managed containers.
pub const DEFAULT_NETWORK_NAME: &str = "ur";

/// Static squid.conf written to `$UR_CONFIG/squid/squid.conf`.
pub const SQUID_CONF: &str = "\
# Ur forward proxy — managed by urd. Do not edit manually.
http_port 3128

acl allowed_domains dstdomain \"/etc/squid/allowlist.txt\"
acl CONNECT method CONNECT

http_access allow CONNECT allowed_domains
http_access allow allowed_domains
http_access deny all

access_log stdio:/dev/stdout
cache_log stdio:/dev/stderr
cache deny all
via off
forwarded_for delete
";

/// Default hostname that containers use to reach the server via Docker DNS.
pub const DEFAULT_SERVER_HOSTNAME: &str = "ur-server";

// ---- Config ----

/// Raw TOML representation — all fields optional so missing keys use defaults.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    workspace: Option<PathBuf>,
    daemon_port: Option<u16>,
    compose_file: Option<PathBuf>,
    proxy: Option<RawProxyConfig>,
    network: Option<RawNetworkConfig>,
}

/// Raw TOML representation for the `[proxy]` section.
#[derive(Debug, Deserialize)]
struct RawProxyConfig {
    hostname: Option<String>,
    allowlist: Option<Vec<String>>,
}

/// Raw TOML representation for the `[network]` section.
#[derive(Debug, Deserialize)]
struct RawNetworkConfig {
    name: Option<String>,
    server_hostname: Option<String>,
}

/// Forward proxy configuration for restricting container network access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    /// Hostname containers use to reach the proxy via Docker DNS (default: "ur-squid").
    pub hostname: String,
    /// Domain allowlist — only these hosts may be reached through the proxy.
    pub allowlist: Vec<String>,
}

/// Docker network configuration for container networking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConfig {
    /// Docker network name that ur-managed containers join (default: "ur").
    pub name: String,
    /// Hostname containers use to reach the server via Docker DNS (default: "ur-server").
    /// This must match the container/service name of the server on the Docker network.
    pub server_hostname: String,
}

/// Resolved, ready-to-use daemon configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Root config directory (`$UR_CONFIG` or `~/.ur`).
    pub config_dir: PathBuf,
    /// Agent workspace directory.
    pub workspace: PathBuf,
    /// TCP port the server listens on (default: 42069).
    pub daemon_port: u16,
    /// Path to the Docker Compose file for starting the server (default: `<config_dir>/docker-compose.yml`).
    pub compose_file: PathBuf,
    /// Forward proxy settings (always enabled with defaults).
    pub proxy: ProxyConfig,
    /// Docker network settings for container networking.
    pub network: NetworkConfig,
}

impl Config {
    /// Load configuration from `$UR_CONFIG/ur.toml`.
    ///
    /// Resolution order:
    /// 1. `$UR_CONFIG` env var → config directory
    /// 2. Falls back to `~/.ur`
    /// 3. Reads `ur.toml` inside that directory (missing file → defaults)
    /// 4. Missing keys in the file → defaults
    pub fn load() -> anyhow::Result<Self> {
        let config_dir = resolve_config_dir()?;
        Self::load_from(&config_dir)
    }

    /// Path to the Squid config directory: `$UR_CONFIG/squid/`.
    pub fn squid_dir(&self) -> PathBuf {
        self.config_dir.join("squid")
    }

    /// Load configuration using an explicit config directory.
    /// Useful for testing.
    pub fn load_from(config_dir: &Path) -> anyhow::Result<Self> {
        let toml_path = config_dir.join("ur.toml");
        let raw = match std::fs::read_to_string(&toml_path) {
            Ok(contents) => toml::from_str::<RawConfig>(&contents)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!(
                    "ur.toml not found in {} — run 'ur init'",
                    config_dir.display()
                );
            }
            Err(e) => return Err(e.into()),
        };

        let workspace = raw
            .workspace
            .unwrap_or_else(|| config_dir.join("workspace"));
        let daemon_port = raw.daemon_port.unwrap_or(DEFAULT_DAEMON_PORT);
        let compose_file = raw
            .compose_file
            .unwrap_or_else(|| config_dir.join("docker-compose.yml"));
        let proxy = match raw.proxy {
            Some(p) => ProxyConfig {
                hostname: p
                    .hostname
                    .unwrap_or_else(|| DEFAULT_PROXY_HOSTNAME.to_string()),
                allowlist: p
                    .allowlist
                    .unwrap_or_else(|| vec!["api.anthropic.com".to_string()]),
            },
            None => ProxyConfig {
                hostname: DEFAULT_PROXY_HOSTNAME.to_string(),
                allowlist: vec!["api.anthropic.com".to_string()],
            },
        };
        let network = match raw.network {
            Some(n) => NetworkConfig {
                name: n.name.unwrap_or_else(|| DEFAULT_NETWORK_NAME.to_string()),
                server_hostname: n
                    .server_hostname
                    .unwrap_or_else(|| DEFAULT_SERVER_HOSTNAME.to_string()),
            },
            None => NetworkConfig {
                name: DEFAULT_NETWORK_NAME.to_string(),
                server_hostname: DEFAULT_SERVER_HOSTNAME.to_string(),
            },
        };

        Ok(Config {
            config_dir: config_dir.to_path_buf(),
            workspace,
            daemon_port,
            compose_file,
            proxy,
            network,
        })
    }
}

/// Filename for the server pid file, stored in the config directory.
pub const SERVER_PID_FILE: &str = "server.pid";

/// Determine the config directory from `$UR_CONFIG` or fall back to `~/.ur`.
pub fn resolve_config_dir() -> anyhow::Result<PathBuf> {
    if let Ok(val) = std::env::var(UR_CONFIG_ENV) {
        return Ok(PathBuf::from(val));
    }
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    Ok(home.join(".ur"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn errors_when_no_toml_file() {
        let tmp = TempDir::new().unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ur.toml not found"), "unexpected error: {msg}");
        assert!(msg.contains("run 'ur init'"), "unexpected error: {msg}");
    }

    #[test]
    fn defaults_when_empty_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.workspace, tmp.path().join("workspace"));
        assert_eq!(cfg.daemon_port, DEFAULT_DAEMON_PORT);
        assert_eq!(cfg.proxy.hostname, DEFAULT_PROXY_HOSTNAME);
    }

    #[test]
    fn reads_workspace_from_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "workspace = \"/custom/workspace\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.workspace, PathBuf::from("/custom/workspace"));
    }

    #[test]
    fn bad_toml_returns_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "not valid [[[ toml").unwrap();
        assert!(Config::load_from(tmp.path()).is_err());
    }

    #[test]
    fn reads_daemon_port_from_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "daemon_port = 9000\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.daemon_port, 9000);
    }

    #[test]
    fn ur_config_env_overrides_default() {
        let tmp = TempDir::new().unwrap();
        // SAFETY: test-only; single-threaded test runner for this module.
        unsafe { std::env::set_var(UR_CONFIG_ENV, tmp.path()) };
        let dir = resolve_config_dir().unwrap();
        // Clean up before asserting
        // SAFETY: test-only; single-threaded test runner for this module.
        unsafe { std::env::remove_var(UR_CONFIG_ENV) };
        assert_eq!(dir, tmp.path());
    }

    #[test]
    fn proxy_section_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[proxy]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.proxy.hostname, DEFAULT_PROXY_HOSTNAME);
        assert_eq!(cfg.proxy.allowlist, vec!["api.anthropic.com"]);
    }

    #[test]
    fn proxy_section_reads_custom_values() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[proxy]\nhostname = \"my-proxy\"\nallowlist = [\"example.com\", \"other.com\"]\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let proxy = &cfg.proxy;
        assert_eq!(proxy.hostname, "my-proxy");
        assert_eq!(proxy.allowlist, vec!["example.com", "other.com"]);
    }

    #[test]
    fn proxy_defaults_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "daemon_port = 5000\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.proxy.hostname, DEFAULT_PROXY_HOSTNAME);
        assert_eq!(cfg.proxy.allowlist, vec!["api.anthropic.com"]);
    }

    #[test]
    fn squid_dir_returns_correct_path() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.squid_dir(), tmp.path().join("squid"));
    }

    #[test]
    fn network_defaults_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "daemon_port = 5000\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, DEFAULT_NETWORK_NAME);
        assert_eq!(cfg.network.server_hostname, DEFAULT_SERVER_HOSTNAME);
    }

    #[test]
    fn network_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[network]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, DEFAULT_NETWORK_NAME);
        assert_eq!(cfg.network.server_hostname, DEFAULT_SERVER_HOSTNAME);
    }

    #[test]
    fn network_reads_custom_values() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[network]\nname = \"custom-net\"\nserver_hostname = \"my-server\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, "custom-net");
        assert_eq!(cfg.network.server_hostname, "my-server");
    }
}
