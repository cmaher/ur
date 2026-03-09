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

// ---- Config ----

/// Raw TOML representation — all fields optional so missing keys use defaults.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    workspace: Option<PathBuf>,
    daemon_port: Option<u16>,
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

        Ok(Config {
            config_dir: config_dir.to_path_buf(),
            workspace,
            daemon_port,
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
    }

    #[test]
    fn defaults_when_empty_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.workspace, tmp.path().join("workspace"));
        assert_eq!(cfg.daemon_port, DEFAULT_DAEMON_PORT);
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
}
