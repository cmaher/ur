use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Environment variable that overrides the config directory (default: `~/.ur`).
const UR_CONFIG_ENV: &str = "UR_CONFIG";

/// Default gRPC port for per-agent servers inside the container.
const DEFAULT_AGENT_GRPC_PORT: u16 = 42069;

/// Raw TOML representation — all fields optional so missing keys use defaults.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    workspace: Option<PathBuf>,
    agent_grpc_port: Option<u16>,
}

/// Resolved, ready-to-use daemon configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Root config directory (`$UR_CONFIG` or `~/.ur`).
    pub config_dir: PathBuf,
    /// Agent workspace directory.
    pub workspace: PathBuf,
    /// Fixed container-side gRPC port for per-agent servers (default: 42069).
    /// Host-side ports are dynamically assigned via `127.0.0.1:0`.
    pub agent_grpc_port: u16,
}

/// Name of the UDS socket file within `config_dir`.
const SOCKET_FILENAME: &str = "ur.sock";

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

    /// Path to the UDS socket file within this config directory.
    pub fn socket_path(&self) -> PathBuf {
        self.config_dir.join(SOCKET_FILENAME)
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
        let agent_grpc_port = raw.agent_grpc_port.unwrap_or(DEFAULT_AGENT_GRPC_PORT);

        Ok(Config {
            config_dir: config_dir.to_path_buf(),
            workspace,
            agent_grpc_port,
        })
    }
}

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
        assert_eq!(cfg.agent_grpc_port, 42069);
    }

    #[test]
    fn defaults_when_empty_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.workspace, tmp.path().join("workspace"));
        assert_eq!(cfg.agent_grpc_port, 42069);
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
    fn reads_agent_grpc_port_from_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "agent_grpc_port = 9999\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.agent_grpc_port, 9999);
    }

    #[test]
    fn bad_toml_returns_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "not valid [[[ toml").unwrap();
        assert!(Config::load_from(tmp.path()).is_err());
    }

    #[test]
    fn socket_path_derived_from_config_dir() {
        let tmp = TempDir::new().unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.socket_path(), tmp.path().join("ur.sock"));
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
