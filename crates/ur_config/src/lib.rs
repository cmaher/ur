use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

// ---- Environment variable names ----

/// Environment variable: override the config directory (default `~/.ur`).
pub const UR_CONFIG_ENV: &str = "UR_CONFIG";

/// Environment variable: `host:port` address for worker→urd gRPC connections.
pub const URD_ADDR_ENV: &str = "URD_ADDR";

/// Environment variable: Claude credentials JSON blob injected into containers.
pub const CLAUDE_CREDENTIALS_ENV: &str = "CLAUDE_CREDENTIALS";

// ---- Defaults ----

/// Default TCP port for the main urd daemon (ur→urd communication).
pub const DEFAULT_DAEMON_PORT: u16 = 42069;

/// Default TCP port for the forward proxy (container→internet via urd).
pub const DEFAULT_PROXY_PORT: u16 = 42070;

// ---- Config ----

/// Raw TOML representation — all fields optional so missing keys use defaults.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    workspace: Option<PathBuf>,
    daemon_port: Option<u16>,
    proxy: Option<RawProxyConfig>,
}

/// Raw TOML representation for the `[proxy]` section.
#[derive(Debug, Deserialize)]
struct RawProxyConfig {
    port: Option<u16>,
    allowlist: Option<Vec<String>>,
}

/// Forward proxy configuration for restricting container network access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    /// TCP port the proxy listens on (default: 42070).
    pub port: u16,
    /// Domain allowlist — only these hosts may be reached through the proxy.
    pub allowlist: Vec<String>,
}

impl ProxyConfig {
    /// Return the allowlist as a `HashSet` for efficient lookup.
    pub fn allowlist_set(&self) -> HashSet<String> {
        self.allowlist.iter().cloned().collect()
    }
}

/// Resolved, ready-to-use daemon configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Root config directory (`$UR_CONFIG` or `~/.ur`).
    pub config_dir: PathBuf,
    /// Agent workspace directory.
    pub workspace: PathBuf,
    /// TCP port the main urd daemon listens on (default: 42069).
    pub daemon_port: u16,
    /// Forward proxy settings. `None` means proxy is disabled.
    pub proxy: Option<ProxyConfig>,
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

    /// Load configuration using an explicit config directory.
    /// Useful for testing.
    pub fn load_from(config_dir: &Path) -> anyhow::Result<Self> {
        let toml_path = config_dir.join("ur.toml");
        let raw = match std::fs::read_to_string(&toml_path) {
            Ok(contents) => toml::from_str::<RawConfig>(&contents)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RawConfig::default(),
            Err(e) => return Err(e.into()),
        };

        let workspace = raw
            .workspace
            .unwrap_or_else(|| config_dir.join("workspace"));
        let daemon_port = raw.daemon_port.unwrap_or(DEFAULT_DAEMON_PORT);
        let proxy = raw.proxy.map(|p| ProxyConfig {
            port: p.port.unwrap_or(DEFAULT_PROXY_PORT),
            allowlist: p.allowlist.unwrap_or_else(|| vec!["api.anthropic.com".to_string()]),
        });

        Ok(Config {
            config_dir: config_dir.to_path_buf(),
            workspace,
            daemon_port,
            proxy,
        })
    }
}

/// Filename for the urd daemon pid file, stored in the config directory.
pub const URD_PID_FILE: &str = "urd.pid";

/// Determine the config directory from `$UR_CONFIG` or fall back to `~/.ur`.
fn resolve_config_dir() -> anyhow::Result<PathBuf> {
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
    fn defaults_when_no_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.config_dir, tmp.path());
        assert_eq!(cfg.workspace, tmp.path().join("workspace"));
        assert_eq!(cfg.daemon_port, DEFAULT_DAEMON_PORT);
        assert!(cfg.proxy.is_none());
    }

    #[test]
    fn defaults_when_empty_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.workspace, tmp.path().join("workspace"));
        assert_eq!(cfg.daemon_port, DEFAULT_DAEMON_PORT);
        assert!(cfg.proxy.is_none());
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
        let proxy = cfg.proxy.unwrap();
        assert_eq!(proxy.port, DEFAULT_PROXY_PORT);
        assert_eq!(proxy.allowlist, vec!["api.anthropic.com"]);
    }

    #[test]
    fn proxy_section_reads_custom_values() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[proxy]\nport = 9999\nallowlist = [\"example.com\", \"other.com\"]\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let proxy = cfg.proxy.unwrap();
        assert_eq!(proxy.port, 9999);
        assert_eq!(proxy.allowlist, vec!["example.com", "other.com"]);
    }

    #[test]
    fn proxy_allowlist_set_returns_hashset() {
        let proxy = ProxyConfig {
            port: DEFAULT_PROXY_PORT,
            allowlist: vec![
                "api.anthropic.com".to_string(),
                "example.com".to_string(),
            ],
        };
        let set = proxy.allowlist_set();
        assert_eq!(set.len(), 2);
        assert!(set.contains("api.anthropic.com"));
        assert!(set.contains("example.com"));
        assert!(!set.contains("blocked.com"));
    }

    #[test]
    fn proxy_none_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "daemon_port = 5000\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.proxy.is_none());
    }
}
