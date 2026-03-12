mod template_path;

pub use template_path::{ResolvedTemplatePath, resolve_template_path};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

// ---- Environment variable names ----

/// Environment variable: override the config directory (default `~/.ur`).
pub const UR_CONFIG_ENV: &str = "UR_CONFIG";

/// Environment variable: `host:port` address for worker→server gRPC connections.
pub const UR_SERVER_ADDR_ENV: &str = "UR_SERVER_ADDR";

/// Environment variable: unique agent ID injected into containers at launch.
/// Format: `{process_id}-{4 random [a-z0-9]}`, e.g. `deploy-x7q2`.
pub const UR_AGENT_ID_ENV: &str = "UR_AGENT_ID";

/// gRPC metadata header key for the agent ID.
/// Sent by ur-tools and workerd on every request so the server can identify
/// which agent is making the call.
pub const AGENT_ID_HEADER: &str = "ur-agent-id";

/// Environment variable: Claude credentials JSON blob injected into containers.
pub const CLAUDE_CREDENTIALS_ENV: &str = "CLAUDE_CREDENTIALS";

/// Environment variable: host-side config directory path.
///
/// The server container sees its config at `/config` (bind mount), but needs the
/// original host path when constructing volume mounts for agent containers
/// (which go through the Docker socket and use host paths).
pub const UR_HOST_CONFIG_ENV: &str = "UR_HOST_CONFIG";

/// Subdirectory under `config_dir` for Claude-related files.
pub const CLAUDE_DIR: &str = "claude";

/// Credentials filename within `CLAUDE_DIR`.
pub const CLAUDE_CREDENTIALS_FILENAME: &str = ".credentials.json";

/// Claude Code app config filename (lives in the user's home directory, NOT inside `CLAUDE_DIR`).
/// Contains onboarding state, oauthAccount, project trust settings, and feature flags.
/// Without this file, Claude Code prompts for login even when credentials exist.
pub const CLAUDE_CONFIG_FILENAME: &str = ".claude.json";

/// Home directory of the worker user inside agent containers.
pub const WORKER_HOME: &str = "/home/worker";

/// Container-internal mount point for the workspace volume.
/// The compose template mounts `$UR_WORKSPACE` (host path) at this path.
pub const WORKSPACE_MOUNT: &str = "/workspace";

/// Environment variable: host-side workspace directory path.
///
/// Like `UR_HOST_CONFIG`, the server container needs the original host path when
/// constructing paths for ur-hostd (which runs on the host).
pub const UR_HOST_WORKSPACE_ENV: &str = "UR_HOST_WORKSPACE";

// ---- Defaults ----

/// Default TCP port for the server (ur→server communication).
pub const DEFAULT_DAEMON_PORT: u16 = 42069;

/// Default TCP port for the host execution daemon (hostd).
pub const DEFAULT_HOSTD_PORT: u16 = 42070;

/// PID file for the hostd process, stored in the config directory.
pub const HOSTD_PID_FILE: &str = "hostd.pid";

/// Environment variable: `host:port` address for worker→hostd gRPC connections.
pub const HOSTD_ADDR_ENV: &str = "UR_HOSTD_ADDR";

/// Subdirectory under `config_dir` for host execution configuration.
pub const HOSTEXEC_DIR: &str = "hostexec";

/// Allowlist configuration filename within `HOSTEXEC_DIR`.
pub const HOSTEXEC_ALLOWLIST_FILE: &str = "allowlist.toml";

/// Default hostname for the Squid proxy container on the Docker network.
pub const DEFAULT_PROXY_HOSTNAME: &str = "ur-squid";

/// Squid listening port inside the container (standard Squid default).
pub const SQUID_PORT: u16 = 3128;

/// Default Docker network name for infrastructure (server + squid, internet-connected).
pub const DEFAULT_NETWORK_NAME: &str = "ur";

/// Default Docker network name for workers (internal, no internet).
/// Workers reach server + squid via Docker DNS on this network.
pub const DEFAULT_WORKER_NETWORK_NAME: &str = "ur-workers";

/// Default hostname that containers use to reach the server via Docker DNS.
pub const DEFAULT_SERVER_HOSTNAME: &str = "ur-server";

/// Default container name prefix for agent containers (e.g., `ur-agent-myticket`).
pub const DEFAULT_AGENT_PREFIX: &str = "ur-agent-";

/// Default maximum number of cached repo clones per project.
pub const DEFAULT_POOL_LIMIT: u32 = 10;

/// Domains required by Claude Code for normal operation.
fn default_proxy_allowlist() -> Vec<String> {
    vec![
        "api.anthropic.com".to_string(),
        "platform.claude.com".to_string(),
        "raw.githubusercontent.com".to_string(),
    ]
}

// ---- Config ----

/// Raw TOML representation — all fields optional so missing keys use defaults.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    workspace: Option<PathBuf>,
    daemon_port: Option<u16>,
    hostd_port: Option<u16>,
    compose_file: Option<PathBuf>,
    proxy: Option<RawProxyConfig>,
    network: Option<RawNetworkConfig>,
    #[serde(default)]
    projects: HashMap<String, RawProjectConfig>,
}

/// Raw TOML representation for a `[projects.<key>]` entry.
#[derive(Debug, Deserialize)]
struct RawProjectConfig {
    repo: String,
    name: Option<String>,
    pool_limit: Option<u32>,
    #[serde(default)]
    hostexec: Vec<String>,
    git_hooks_dir: Option<String>,
    #[serde(default)]
    mounts: Vec<String>,
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
    worker_name: Option<String>,
    server_hostname: Option<String>,
    agent_prefix: Option<String>,
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
    /// Infrastructure network name — internet-connected bridge for server + squid (default: "ur").
    pub name: String,
    /// Worker network name — internal bridge with no internet (default: "ur-workers").
    /// Workers join this network and reach the internet only through the squid proxy.
    pub worker_name: String,
    /// Hostname containers use to reach the server via Docker DNS (default: "ur-server").
    /// This must match the container/service name of the server on the Docker network.
    pub server_hostname: String,
    /// Container name prefix for agent containers (default: "ur-agent-").
    /// Agent containers are named `{agent_prefix}{process_id}`.
    pub agent_prefix: String,
}

/// Resolved project configuration for a single project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    /// Structural identifier (the TOML table key, e.g. "ur").
    pub key: String,
    /// Git remote URL (required).
    pub repo: String,
    /// Display-friendly label (defaults to the key).
    pub name: String,
    /// Maximum number of cached repo clones in the pool (default: 10).
    pub pool_limit: u32,
    /// Additional passthrough hostexec commands for this project.
    /// These are added to the global allowlist when agents run against this project.
    pub hostexec: Vec<String>,
    /// Optional template path to a directory of git hook scripts.
    /// Supports `%PROJECT%/...` and `%URCONFIG%/...` template variables, or absolute paths.
    /// Resolve with [`resolve_template_path`] at use time.
    pub git_hooks_dir: Option<String>,
    /// Additional volume mount paths for this project.
    /// Each entry is a template path string supporting `%PROJECT%/...`, `%URCONFIG%/...`,
    /// or absolute paths. Validated at config load time; resolved at use time via
    /// [`resolve_template_path`].
    pub mounts: Vec<String>,
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
    /// TCP port the host execution daemon listens on (default: 42070).
    pub hostd_port: u16,
    /// Path to the Docker Compose file for starting the server (default: `<config_dir>/docker-compose.yml`).
    pub compose_file: PathBuf,
    /// Forward proxy settings (always enabled with defaults).
    pub proxy: ProxyConfig,
    /// Docker network settings for container networking.
    pub network: NetworkConfig,
    /// Configured projects, keyed by project key.
    pub projects: HashMap<String, ProjectConfig>,
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

    /// Path to the host execution config directory: `$UR_CONFIG/hostexec/`.
    pub fn hostexec_dir(&self) -> PathBuf {
        self.config_dir.join(HOSTEXEC_DIR)
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
        let hostd_port = raw.hostd_port.unwrap_or(DEFAULT_HOSTD_PORT);
        let compose_file = raw
            .compose_file
            .unwrap_or_else(|| config_dir.join("docker-compose.yml"));
        let proxy = match raw.proxy {
            Some(p) => ProxyConfig {
                hostname: p
                    .hostname
                    .unwrap_or_else(|| DEFAULT_PROXY_HOSTNAME.to_string()),
                allowlist: p.allowlist.unwrap_or_else(default_proxy_allowlist),
            },
            None => ProxyConfig {
                hostname: DEFAULT_PROXY_HOSTNAME.to_string(),
                allowlist: default_proxy_allowlist(),
            },
        };
        let network = match raw.network {
            Some(n) => NetworkConfig {
                name: n.name.unwrap_or_else(|| DEFAULT_NETWORK_NAME.to_string()),
                worker_name: n
                    .worker_name
                    .unwrap_or_else(|| DEFAULT_WORKER_NETWORK_NAME.to_string()),
                server_hostname: n
                    .server_hostname
                    .unwrap_or_else(|| DEFAULT_SERVER_HOSTNAME.to_string()),
                agent_prefix: n
                    .agent_prefix
                    .unwrap_or_else(|| DEFAULT_AGENT_PREFIX.to_string()),
            },
            None => NetworkConfig {
                name: DEFAULT_NETWORK_NAME.to_string(),
                worker_name: DEFAULT_WORKER_NETWORK_NAME.to_string(),
                server_hostname: DEFAULT_SERVER_HOSTNAME.to_string(),
                agent_prefix: DEFAULT_AGENT_PREFIX.to_string(),
            },
        };

        let projects = raw
            .projects
            .into_iter()
            .map(|(key, raw_proj)| {
                validate_project_templates(&key, &raw_proj)?;
                let resolved = ProjectConfig {
                    name: raw_proj.name.unwrap_or_else(|| key.clone()),
                    repo: raw_proj.repo,
                    pool_limit: raw_proj.pool_limit.unwrap_or(DEFAULT_POOL_LIMIT),
                    key: key.clone(),
                    hostexec: raw_proj.hostexec,
                    git_hooks_dir: raw_proj.git_hooks_dir,
                    mounts: raw_proj.mounts,
                };
                Ok((key, resolved))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        Ok(Config {
            config_dir: config_dir.to_path_buf(),
            workspace,
            daemon_port,
            hostd_port,
            compose_file,
            proxy,
            network,
            projects,
        })
    }
}

/// Filename for the server pid file, stored in the config directory.
pub const SERVER_PID_FILE: &str = "server.pid";

/// Determine the config directory from `$UR_CONFIG` or fall back to `~/.ur`.
fn validate_project_templates(key: &str, raw_proj: &RawProjectConfig) -> anyhow::Result<()> {
    if let Some(ref tpl) = raw_proj.git_hooks_dir {
        template_path::validate_template_str(tpl)
            .map_err(|e| anyhow::anyhow!("project '{}': git_hooks_dir: {}", key, e))?;
    }
    for (i, mount) in raw_proj.mounts.iter().enumerate() {
        template_path::validate_template_str(mount)
            .map_err(|e| anyhow::anyhow!("project '{}': mounts[{}]: {}", key, i, e))?;
    }
    Ok(())
}

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
        assert_eq!(cfg.proxy.allowlist, default_proxy_allowlist());
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
        assert_eq!(cfg.proxy.allowlist, default_proxy_allowlist());
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
        assert_eq!(cfg.network.worker_name, DEFAULT_WORKER_NETWORK_NAME);
        assert_eq!(cfg.network.server_hostname, DEFAULT_SERVER_HOSTNAME);
        assert_eq!(cfg.network.agent_prefix, DEFAULT_AGENT_PREFIX);
    }

    #[test]
    fn network_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[network]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, DEFAULT_NETWORK_NAME);
        assert_eq!(cfg.network.worker_name, DEFAULT_WORKER_NETWORK_NAME);
        assert_eq!(cfg.network.server_hostname, DEFAULT_SERVER_HOSTNAME);
        assert_eq!(cfg.network.agent_prefix, DEFAULT_AGENT_PREFIX);
    }

    #[test]
    fn network_reads_custom_values() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[network]\nname = \"custom-net\"\nworker_name = \"custom-workers\"\nserver_hostname = \"my-server\"\nagent_prefix = \"test-agent-\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, "custom-net");
        assert_eq!(cfg.network.worker_name, "custom-workers");
        assert_eq!(cfg.network.server_hostname, "my-server");
        assert_eq!(cfg.network.agent_prefix, "test-agent-");
    }

    #[test]
    fn no_projects_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.projects.is_empty());
    }

    #[test]
    fn parses_single_project_with_defaults() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects.len(), 1);
        let proj = &cfg.projects["ur"];
        assert_eq!(proj.key, "ur");
        assert_eq!(proj.repo, "git@github.com:cmaher/ur.git");
        assert_eq!(proj.name, "ur");
        assert_eq!(proj.pool_limit, DEFAULT_POOL_LIMIT);
    }

    #[test]
    fn parses_project_with_custom_name_and_pool_limit() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.swa]
repo = "git@github.com:cmaher/swa.git"
name = "Swa App"
pool_limit = 5
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let proj = &cfg.projects["swa"];
        assert_eq!(proj.key, "swa");
        assert_eq!(proj.repo, "git@github.com:cmaher/swa.git");
        assert_eq!(proj.name, "Swa App");
        assert_eq!(proj.pool_limit, 5);
    }

    #[test]
    fn parses_multiple_projects() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"

[projects.swa]
repo = "git@github.com:cmaher/swa.git"
name = "Swa App"
pool_limit = 5
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects.len(), 2);
        assert!(cfg.projects.contains_key("ur"));
        assert!(cfg.projects.contains_key("swa"));
    }

    #[test]
    fn parses_project_with_hostexec_commands() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
hostexec = ["tk", "make", "cargo"]
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let proj = &cfg.projects["ur"];
        assert_eq!(proj.hostexec, vec!["tk", "make", "cargo"]);
    }

    #[test]
    fn hostexec_defaults_to_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let proj = &cfg.projects["ur"];
        assert!(proj.hostexec.is_empty());
    }

    #[test]
    fn project_missing_repo_is_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.bad]
name = "Missing Repo"
"#,
        )
        .unwrap();
        assert!(Config::load_from(tmp.path()).is_err());
    }

    #[test]
    fn git_hooks_dir_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["ur"].git_hooks_dir, None);
    }

    #[test]
    fn git_hooks_dir_stores_template_string() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
git_hooks_dir = "%PROJECT%/.git-hooks"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.projects["ur"].git_hooks_dir.as_deref(),
            Some("%PROJECT%/.git-hooks")
        );
    }

    #[test]
    fn git_hooks_dir_rejects_unrecognized_variable() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
git_hooks_dir = "%BADVAR%/hooks"
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unrecognized template variable"), "{msg}");
        assert!(msg.contains("project 'ur'"), "{msg}");
    }

    #[test]
    fn git_hooks_dir_accepts_absolute_path() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
git_hooks_dir = "/opt/hooks/ur"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.projects["ur"].git_hooks_dir.as_deref(),
            Some("/opt/hooks/ur")
        );
    }

    #[test]
    fn git_hooks_dir_accepts_urconfig_template() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
git_hooks_dir = "%URCONFIG%/hooks/ur"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.projects["ur"].git_hooks_dir.as_deref(),
            Some("%URCONFIG%/hooks/ur")
        );
    }

    #[test]
    fn mounts_defaults_to_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.projects["ur"].mounts.is_empty());
    }

    #[test]
    fn mounts_parses_multiple_entries() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
mounts = ["%PROJECT%/.cache", "%URCONFIG%/shared-data", "/opt/tools"]
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.projects["ur"].mounts,
            vec!["%PROJECT%/.cache", "%URCONFIG%/shared-data", "/opt/tools"]
        );
    }

    #[test]
    fn mounts_rejects_invalid_variable() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
mounts = ["%PROJECT%/.cache", "%INVALID%/bad"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mounts[1]"), "{msg}");
        assert!(msg.contains("unrecognized template variable"), "{msg}");
        assert!(msg.contains("project 'ur'"), "{msg}");
    }
}
