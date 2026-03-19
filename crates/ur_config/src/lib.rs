mod template_path;

pub use template_path::{ResolvedTemplatePath, resolve_template_path};

/// A parsed mount configuration entry: host source -> container destination.
///
/// Source supports `%URCONFIG%/...` template variables or absolute paths.
/// `%PROJECT%` is not supported for mount sources — mounts are for paths
/// outside the project repo (project-relative paths are already accessible
/// via the workspace mount).
///
/// Destination must be an absolute container path (e.g., `/workspace/.tickets`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountConfig {
    /// Template path for the host-side source directory.
    pub source: String,
    /// Absolute container-side destination path.
    pub destination: String,
}

/// A parsed port mapping entry: host port -> container port.
///
/// Maps to Docker's `-p host_port:container_port` flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortMapping {
    /// TCP port on the host.
    pub host_port: u16,
    /// TCP port inside the container.
    pub container_port: u16,
}

/// Parse a mount string in `"source:destination"` format.
///
/// Splits on the first `:` character. Validates that:
/// - The source is a valid template path (but not `%PROJECT%`)
/// - The destination is an absolute path (starts with `/`)
fn parse_mount_entry(project_key: &str, index: usize, raw: &str) -> anyhow::Result<MountConfig> {
    let colon_pos = raw.find(':').ok_or_else(|| {
        anyhow::anyhow!(
            "project '{project_key}': mounts[{index}]: expected 'source:destination' format, got: {raw}"
        )
    })?;

    let source = &raw[..colon_pos];
    let destination = &raw[colon_pos + 1..];

    if source.is_empty() {
        anyhow::bail!("project '{project_key}': mounts[{index}]: source must not be empty");
    }
    if destination.is_empty() {
        anyhow::bail!("project '{project_key}': mounts[{index}]: destination must not be empty");
    }

    // Validate source template — must not use %PROJECT%
    template_path::validate_template_str(source)
        .map_err(|e| anyhow::anyhow!("project '{project_key}': mounts[{index}]: source: {e}"))?;
    if source.starts_with("%PROJECT%") {
        anyhow::bail!(
            "project '{project_key}': mounts[{index}]: source must not use %PROJECT% \
             (project-relative paths are already accessible via the workspace mount)"
        );
    }

    // Validate destination is absolute
    if !destination.starts_with('/') {
        anyhow::bail!(
            "project '{project_key}': mounts[{index}]: destination must be an absolute path, got: {destination}"
        );
    }

    Ok(MountConfig {
        source: source.to_string(),
        destination: destination.to_string(),
    })
}

/// Parse a port mapping string in `"host_port:container_port"` format.
///
/// Both ports must be valid u16 values.
fn parse_port_entry(project_key: &str, index: usize, raw: &str) -> anyhow::Result<PortMapping> {
    let colon_pos = raw.find(':').ok_or_else(|| {
        anyhow::anyhow!(
            "project '{project_key}': ports[{index}]: expected 'host_port:container_port' format, got: {raw}"
        )
    })?;

    let host_str = &raw[..colon_pos];
    let container_str = &raw[colon_pos + 1..];

    let host_port: u16 = host_str.parse().map_err(|_| {
        anyhow::anyhow!(
            "project '{project_key}': ports[{index}]: invalid host port '{host_str}', expected a number 0-65535"
        )
    })?;
    let container_port: u16 = container_str.parse().map_err(|_| {
        anyhow::anyhow!(
            "project '{project_key}': ports[{index}]: invalid container port '{container_str}', expected a number 0-65535"
        )
    })?;

    Ok(PortMapping {
        host_port,
        container_port,
    })
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

// ---- Environment variable names ----

/// Environment variable: override the config directory (default `~/.ur`).
pub const UR_CONFIG_ENV: &str = "UR_CONFIG";

/// Environment variable: `host:port` address for worker→server gRPC connections.
pub const UR_SERVER_ADDR_ENV: &str = "UR_SERVER_ADDR";

/// Environment variable: unique worker ID injected into containers at launch.
/// Format: `{process_id}-{4 random [a-z0-9]}`, e.g. `deploy-x7q2`.
pub const UR_WORKER_ID_ENV: &str = "UR_WORKER_ID";

/// gRPC metadata header key for the worker ID.
/// Sent by workertools and workerd on every request so the server can identify
/// which worker is making the call.
pub const WORKER_ID_HEADER: &str = "ur-worker-id";

/// Environment variable: per-worker secret (UUID v4) injected into containers at launch.
/// Used alongside `UR_WORKER_ID` to authenticate worker requests to the shared worker server.
pub const UR_WORKER_SECRET_ENV: &str = "UR_WORKER_SECRET";

/// gRPC metadata header key for the worker secret.
/// Sent by workertools on every request to the worker server for authentication.
pub const WORKER_SECRET_HEADER: &str = "ur-worker-secret";

/// Environment variable: Claude credentials JSON blob injected into containers.
pub const CLAUDE_CREDENTIALS_ENV: &str = "CLAUDE_CREDENTIALS";

/// Environment variable: host-side config directory path.
///
/// The server container sees its config at `/config` (bind mount), but needs the
/// original host path when constructing volume mounts for worker containers
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

/// Home directory of the worker user inside worker containers.
pub const WORKER_HOME: &str = "/home/worker";

/// Container-internal mount point for the workspace volume.
/// The compose template mounts `$UR_WORKSPACE` (host path) at this path.
pub const WORKSPACE_MOUNT: &str = "/workspace";

/// Environment variable: host-side workspace directory path.
///
/// Like `UR_HOST_CONFIG`, the server container needs the original host path when
/// constructing paths for builderd (which runs on the host).
pub const UR_HOST_WORKSPACE_ENV: &str = "UR_HOST_WORKSPACE";

/// Container-side mount point for the backup directory.
pub const BACKUP_CONTAINER_PATH: &str = "/backup";

// ---- Defaults ----

/// Default TCP port for the server (ur→server communication).
pub const DEFAULT_SERVER_PORT: u16 = 42069;

/// Default TCP port for the builder daemon (builderd).
/// Kept for documentation; the actual default is derived as `server_port + 2`.
pub const DEFAULT_BUILDERD_PORT: u16 = 42070;

/// PID file for the builderd process, stored in the config directory.
pub const BUILDERD_PID_FILE: &str = "builderd.pid";

/// Environment variable: `host:port` address for worker→builderd gRPC connections.
pub const BUILDERD_ADDR_ENV: &str = "UR_BUILDERD_ADDR";

/// Subdirectory under `config_dir` for host execution configuration.
pub const HOSTEXEC_DIR: &str = "hostexec";

/// Allowlist configuration filename within `HOSTEXEC_DIR`.
/// Deprecated: hostexec commands are now configured in `ur.toml` under `[hostexec.commands]`.
/// This constant is retained only for migration detection.
pub const HOSTEXEC_ALLOWLIST_FILE: &str = "allowlist.toml";

/// Default hostname for the Squid proxy container on the Docker network.
pub const DEFAULT_PROXY_HOSTNAME: &str = "ur-squid";

/// Squid listening port inside the container (standard Squid default).
pub const SQUID_PORT: u16 = 3128;

/// Port for workerd healthz HTTP endpoint inside worker containers.
pub const WORKERD_HEALTHZ_PORT: u16 = 9119;

/// Port for workerd gRPC server inside worker containers.
pub const WORKERD_GRPC_PORT: u16 = 9120;

/// Default Docker network name for infrastructure (server + squid, internet-connected).
pub const DEFAULT_NETWORK_NAME: &str = "ur";

/// Default Docker network name for workers (internal, no internet).
/// Workers reach server + squid via Docker DNS on this network.
pub const DEFAULT_WORKER_NETWORK_NAME: &str = "ur-workers";

/// Default hostname that containers use to reach the server via Docker DNS.
pub const DEFAULT_SERVER_HOSTNAME: &str = "ur-server";

/// Default container name prefix for worker containers (e.g., `ur-worker-myticket`).
pub const DEFAULT_WORKER_PREFIX: &str = "ur-worker-";

/// Default maximum number of cached repo clones per project.
pub const DEFAULT_POOL_LIMIT: u32 = 10;

/// Default hostname for the Qdrant vector database on the Docker network.
pub const DEFAULT_QDRANT_HOSTNAME: &str = "ur-qdrant";

/// Default gRPC port for Qdrant.
pub const DEFAULT_QDRANT_PORT: u16 = 6334;

/// Default embedding model name for RAG.
pub const DEFAULT_EMBEDDING_MODEL: &str = "all-MiniLM-L6-v2";

/// HuggingFace download metadata for a supported embedding model.
///
/// Contains only the information needed to download model files — no fastembed
/// or other heavy dependencies. Used by the CLI (`ur rag model download`) and
/// the install script logic.
pub struct ModelDownloadInfo {
    /// HuggingFace org (e.g. "Qdrant").
    pub hf_org: &'static str,
    /// HuggingFace repo name (e.g. "all-MiniLM-L6-v2-onnx").
    pub hf_repo: &'static str,
    /// Git commit hash for the snapshot.
    pub hf_commit: &'static str,
    /// Files to download from HuggingFace.
    pub hf_files: &'static [&'static str],
    /// Vector dimensionality (e.g. 384 for MiniLM).
    pub vector_size: u64,
}

const MINI_LM_FILES: &[&str] = &[
    "model.onnx",
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

static SUPPORTED_MODELS: &[(&str, ModelDownloadInfo)] = &[(
    "all-MiniLM-L6-v2",
    ModelDownloadInfo {
        hf_org: "Qdrant",
        hf_repo: "all-MiniLM-L6-v2-onnx",
        hf_commit: "5f1b8cd78bc4fb444dd171e59b18f3a3af89a079",
        hf_files: MINI_LM_FILES,
        vector_size: 384,
    },
)];

/// Look up model download info by config name (e.g. "all-MiniLM-L6-v2").
///
/// Returns `None` for unknown model names.
pub fn model_download_info(name: &str) -> Option<&'static ModelDownloadInfo> {
    SUPPORTED_MODELS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, info)| info)
}

/// List all supported model names.
pub fn supported_model_names() -> Vec<&'static str> {
    SUPPORTED_MODELS.iter().map(|(n, _)| *n).collect()
}

/// Domains required by Claude Code for normal operation.
fn default_proxy_allowlist() -> Vec<String> {
    vec![
        "api.anthropic.com".to_string(),
        "platform.claude.com".to_string(),
    ]
}

// ---- Config ----

/// Default backup interval in minutes.
pub const DEFAULT_BACKUP_INTERVAL_MINUTES: u64 = 30;

/// Default number of backup files to retain.
pub const DEFAULT_BACKUP_RETAIN_COUNT: u64 = 3;

/// Raw TOML representation — all fields optional so missing keys use defaults.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    workspace: Option<PathBuf>,
    server_port: Option<u16>,
    worker_port: Option<u16>,
    builderd_port: Option<u16>,
    compose_file: Option<PathBuf>,
    git_branch_prefix: Option<String>,
    proxy: Option<RawProxyConfig>,
    network: Option<RawNetworkConfig>,
    hostexec: Option<RawHostExecConfig>,
    rag: Option<RawRagConfig>,
    backup: Option<RawBackupConfig>,
    server: Option<RawServerConfig>,
    #[serde(default)]
    projects: HashMap<String, RawProjectConfig>,
}

/// Raw TOML representation for the `[hostexec]` section.
#[derive(Debug, Default, Deserialize)]
struct RawHostExecConfig {
    #[serde(default)]
    commands: HashMap<String, RawHostExecCommandConfig>,
}

/// Raw TOML representation for a single hostexec command entry.
#[derive(Debug, Default, Deserialize)]
struct RawHostExecCommandConfig {
    /// Path to a Lua script (relative to `$UR_CONFIG/hostexec/`).
    lua: Option<String>,
    /// Use the built-in default Lua script for this command (if one exists).
    default_script: Option<bool>,
    /// When true, the process is expected to run indefinitely (e.g. a daemon).
    long_lived: Option<bool>,
    /// When true, the command uses bidirectional streaming (requires long_lived = true).
    bidi: Option<bool>,
}

/// Resolved configuration for a single hostexec command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostExecCommandConfig {
    /// Path to a Lua script (relative to `$UR_CONFIG/hostexec/`).
    pub lua: Option<String>,
    /// Use the built-in default Lua script for this command (if one exists).
    pub default_script: bool,
    /// When true, the process is expected to run indefinitely (e.g. a daemon).
    pub long_lived: bool,
    /// When true, the command uses bidirectional streaming (requires long_lived = true).
    pub bidi: bool,
}

/// Resolved hostexec configuration from the `[hostexec]` section of `ur.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostExecConfig {
    /// Named commands with optional Lua transform configuration.
    pub commands: HashMap<String, HostExecCommandConfig>,
}

/// Raw TOML representation for a `[projects.<key>.container]` section.
#[derive(Debug, Deserialize)]
struct RawContainerConfig {
    image: String,
    #[serde(default)]
    mounts: Vec<String>,
    #[serde(default)]
    ports: Vec<String>,
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
    container: Option<RawContainerConfig>,
    /// Reject mounts at the project root level with a helpful error.
    #[serde(default)]
    mounts: Option<serde::de::IgnoredAny>,
    /// Template path to a directory of workflow hook scripts.
    workflow_hooks_dir: Option<String>,
    /// Maximum fix loop iterations before stalling agent.
    max_fix_attempts: Option<u32>,
    /// Branches that cannot be force-pushed. Supports glob patterns.
    protected_branches: Option<Vec<String>>,
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
    worker_prefix: Option<String>,
}

/// Raw TOML representation for the `[rag]` section.
#[derive(Debug, Deserialize)]
struct RawRagConfig {
    qdrant_hostname: Option<String>,
    embedding_model: Option<String>,
    docs: Option<RawRagDocsConfig>,
}

/// Raw TOML representation for the `[rag.docs]` section.
#[derive(Debug, Deserialize)]
struct RawRagDocsConfig {
    #[serde(default)]
    exclude: Vec<String>,
}

/// Raw TOML representation for the `[backup]` section.
#[derive(Debug, Deserialize)]
struct RawBackupConfig {
    path: Option<PathBuf>,
    interval_minutes: Option<u64>,
    enabled: Option<bool>,
    retain_count: Option<u64>,
}

/// Raw TOML representation for the `[server]` section.
#[derive(Debug, Default, Deserialize)]
struct RawServerConfig {
    container_command: Option<String>,
    stale_worker_ttl_days: Option<u64>,
    max_transition_attempts: Option<i32>,
    poll_interval_ms: Option<u64>,
    github_scan_interval_secs: Option<u64>,
}

/// Environment variable: container runtime command override (e.g. "nerdctl").
/// Checked as a fallback when `[server].container_command` is not set in ur.toml.
pub const UR_CONTAINER_ENV: &str = "UR_CONTAINER";

/// Default container runtime command.
pub const DEFAULT_CONTAINER_COMMAND: &str = "docker";

/// Default number of days before a stale worker is cleaned up.
pub const DEFAULT_STALE_WORKER_TTL_DAYS: u64 = 7;

/// Default maximum number of lifecycle transition attempts before giving up.
pub const DEFAULT_MAX_TRANSITION_ATTEMPTS: i32 = 3;

/// Default poll interval in milliseconds for background polling loops.
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 500;

/// Default GitHub scan interval in seconds for the poller.
pub const DEFAULT_GITHUB_SCAN_INTERVAL_SECS: u64 = 30;

/// Server runtime configuration from the `[server]` section of `ur.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Container runtime command (default: "docker").
    /// Resolution order: ur.toml > `UR_CONTAINER` env var > "docker".
    pub container_command: String,
    /// Number of days before stale workers are cleaned up (default: 7).
    pub stale_worker_ttl_days: u64,
    /// Maximum number of lifecycle transition attempts (default: 3).
    pub max_transition_attempts: i32,
    /// Poll interval in milliseconds for background loops (default: 500).
    pub poll_interval_ms: u64,
    /// GitHub scan interval in seconds for the poller (default: 30).
    pub github_scan_interval_secs: u64,
}

/// Default maximum number of fix loop iterations before stalling an agent.
pub const DEFAULT_MAX_FIX_ATTEMPTS: u32 = 10;

/// Default protected branch patterns (branches that cannot be force-pushed).
pub fn default_protected_branches() -> Vec<String> {
    vec!["main".to_string(), "master".to_string()]
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
    /// Container name prefix for worker containers (default: "ur-worker-").
    /// Worker containers are named `{worker_prefix}{process_id}`.
    pub worker_prefix: String,
}

/// RAG (Retrieval-Augmented Generation) configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RagConfig {
    /// Hostname containers use to reach Qdrant via Docker DNS (default: "ur-qdrant").
    pub qdrant_hostname: String,
    /// Embedding model name (default: "all-MiniLM-L6-v2").
    pub embedding_model: String,
    /// Documentation generation settings.
    pub docs: RagDocsConfig,
}

/// Configuration for RAG documentation generation (`[rag.docs]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RagDocsConfig {
    /// Direct dependency crate names to exclude from generated docs.
    /// Useful for filtering out noisy deps that add bulk without value.
    pub exclude: Vec<String>,
}

/// Backup configuration for periodic database snapshots.
///
/// When `path` is `None`, periodic backup is disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupConfig {
    /// Directory to write backup files to. `None` means backup is disabled.
    pub path: Option<PathBuf>,
    /// Interval between backups in minutes (default: 30).
    pub interval_minutes: u64,
    /// Whether periodic backups are enabled (default: true). When false,
    /// disables periodic backups even if a path is configured. Manual
    /// `ur db backup` still works.
    pub enabled: bool,
    /// Number of backup files to retain (default: 3). Older backups beyond
    /// this count are deleted after each successful backup.
    pub retain_count: u64,
}

/// Known image aliases and their full tags.
pub const IMAGE_ALIASES: &[(&str, &str)] = &[
    ("base", "ur-worker:latest"),
    ("rust", "ur-worker-rust:latest"),
];

/// Validate that the given string is a known image alias or a full image reference.
/// Returns `Ok(())` if valid, or an error describing the valid aliases.
pub fn validate_image_alias(raw: &str) -> anyhow::Result<()> {
    // Full image references are always valid
    if raw.contains(':') || raw.contains('/') {
        return Ok(());
    }
    for (alias, _) in IMAGE_ALIASES {
        if raw == *alias {
            return Ok(());
        }
    }
    let valid: Vec<&str> = IMAGE_ALIASES.iter().map(|(a, _)| *a).collect();
    anyhow::bail!(
        "unknown image alias '{raw}'. Valid aliases: {valid:?}. \
         Use a full image reference (e.g. 'myimage:tag') for custom images."
    )
}

/// Resolve an image alias to its full tag.
/// Returns the full tag if the input is a known alias, or an error if the alias is unknown.
/// If the input contains `:` or `/`, it is treated as a full image reference and returned as-is.
fn resolve_image_alias(project_key: &str, raw: &str) -> anyhow::Result<String> {
    // If it looks like a full image reference, pass through
    if raw.contains(':') || raw.contains('/') {
        return Ok(raw.to_string());
    }
    // Try alias lookup
    for (alias, tag) in IMAGE_ALIASES {
        if raw == *alias {
            return Ok(tag.to_string());
        }
    }
    let valid: Vec<&str> = IMAGE_ALIASES.iter().map(|(a, _)| *a).collect();
    anyhow::bail!(
        "project '{project_key}': container.image: unknown alias '{raw}'. \
         Valid aliases: {valid:?}. Use a full image reference (e.g. 'myimage:tag') for custom images."
    )
}

/// Resolved container configuration for a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerConfig {
    /// Full image tag (after alias resolution).
    pub image: String,
    /// Additional volume mounts for this project.
    /// Each entry maps a host-side source to a container-side destination.
    /// Source supports `%URCONFIG%/...` or absolute paths (not `%PROJECT%`).
    /// Parsed from `"source:destination"` strings in TOML.
    pub mounts: Vec<MountConfig>,
    /// Port mappings for this project's containers.
    /// Each entry maps a host TCP port to a container TCP port (`-p host:container`).
    /// Parsed from `"host_port:container_port"` strings in TOML.
    pub ports: Vec<PortMapping>,
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
    /// Container configuration (image, mounts).
    pub container: ContainerConfig,
    /// Optional template path to a directory of workflow hook scripts.
    /// Supports `%PROJECT%/...`, `%URCONFIG%/...` template variables, or absolute paths.
    /// Resolve with [`resolve_template_path`] at use time.
    pub workflow_hooks_dir: Option<String>,
    /// Maximum fix loop iterations before stalling the agent (default: 5).
    pub max_fix_attempts: u32,
    /// Branch patterns that cannot be force-pushed (default: `["main", "master"]`).
    /// Supports glob patterns.
    pub protected_branches: Vec<String>,
}

/// Resolved, ready-to-use daemon configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Root config directory (`$UR_CONFIG` or `~/.ur`).
    pub config_dir: PathBuf,
    /// Worker workspace directory.
    pub workspace: PathBuf,
    /// TCP port the server listens on (default: 42069).
    pub server_port: u16,
    /// TCP port the shared worker gRPC server listens on (default: `server_port + 1`).
    pub worker_port: u16,
    /// TCP port the builder daemon listens on (default: `server_port + 2`).
    pub builderd_port: u16,
    /// Path to the Docker Compose file for starting the server (default: `<config_dir>/docker-compose.yml`).
    pub compose_file: PathBuf,
    /// Forward proxy settings (always enabled with defaults).
    pub proxy: ProxyConfig,
    /// Docker network settings for container networking.
    pub network: NetworkConfig,
    /// Global hostexec command configuration (from `[hostexec]` section).
    pub hostexec: HostExecConfig,
    /// RAG system settings (Qdrant vector database).
    pub rag: RagConfig,
    /// Periodic backup settings for the database.
    pub backup: BackupConfig,
    /// Server runtime settings (container command, polling intervals, etc.).
    pub server: ServerConfig,
    /// Prefix prepended to worker-ID branch names (e.g. `"feature/"` → `feature/myproc-a1b2`).
    /// Empty string means no prefix.
    pub git_branch_prefix: String,
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
        let server_port = raw.server_port.unwrap_or(DEFAULT_SERVER_PORT);
        let worker_port = raw.worker_port.unwrap_or(server_port + 1);
        let builderd_port = raw.builderd_port.unwrap_or(server_port + 2);
        let compose_file = raw
            .compose_file
            .unwrap_or_else(|| config_dir.join("docker-compose.yml"));
        let proxy = resolve_proxy(raw.proxy);
        let network = resolve_network(raw.network);

        let hostexec = match raw.hostexec {
            Some(h) => resolve_hostexec_config(h)?,
            None => HostExecConfig::default(),
        };

        let rag = resolve_rag(raw.rag);
        let backup = resolve_backup(raw.backup);
        let server = resolve_server(raw.server);

        let projects = raw
            .projects
            .into_iter()
            .map(|(key, raw_proj)| {
                // Reject mounts at project root level
                if raw_proj.mounts.is_some() {
                    anyhow::bail!(
                        "project '{key}': 'mounts' must be inside [projects.{key}.container], \
                         not at the project root level"
                    );
                }

                validate_project_templates(&key, &raw_proj)?;

                let raw_container = raw_proj.container.ok_or_else(|| {
                    anyhow::anyhow!(
                        "project '{key}': missing required [projects.{key}.container] section"
                    )
                })?;

                let image = resolve_image_alias(&key, &raw_container.image)?;
                let mounts = raw_container
                    .mounts
                    .iter()
                    .enumerate()
                    .map(|(i, m)| parse_mount_entry(&key, i, m))
                    .collect::<anyhow::Result<Vec<_>>>()?;

                let ports = raw_container
                    .ports
                    .iter()
                    .enumerate()
                    .map(|(i, p)| parse_port_entry(&key, i, p))
                    .collect::<anyhow::Result<Vec<_>>>()?;

                let container = ContainerConfig {
                    image,
                    mounts,
                    ports,
                };

                let resolved = ProjectConfig {
                    name: raw_proj.name.unwrap_or_else(|| key.clone()),
                    repo: raw_proj.repo,
                    pool_limit: raw_proj.pool_limit.unwrap_or(DEFAULT_POOL_LIMIT),
                    key: key.clone(),
                    hostexec: raw_proj.hostexec,
                    git_hooks_dir: raw_proj.git_hooks_dir,
                    container,
                    workflow_hooks_dir: raw_proj.workflow_hooks_dir,
                    max_fix_attempts: raw_proj
                        .max_fix_attempts
                        .unwrap_or(DEFAULT_MAX_FIX_ATTEMPTS),
                    protected_branches: raw_proj
                        .protected_branches
                        .unwrap_or_else(default_protected_branches),
                };
                Ok((key, resolved))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        let git_branch_prefix = raw.git_branch_prefix.unwrap_or_default();

        Ok(Config {
            config_dir: config_dir.to_path_buf(),
            workspace,
            server_port,
            worker_port,
            builderd_port,
            compose_file,
            proxy,
            network,
            hostexec,
            rag,
            backup,
            server,
            git_branch_prefix,
            projects,
        })
    }
}

/// Filename for the server pid file, stored in the config directory.
pub const SERVER_PID_FILE: &str = "server.pid";

fn resolve_hostexec_config(raw: RawHostExecConfig) -> anyhow::Result<HostExecConfig> {
    let mut commands = HashMap::new();
    for (name, raw_cmd) in raw.commands {
        let long_lived = raw_cmd.long_lived.unwrap_or(false);
        let bidi = raw_cmd.bidi.unwrap_or(false);
        if bidi && !long_lived {
            anyhow::bail!("hostexec command '{name}': bidi = true requires long_lived = true");
        }
        let cmd = HostExecCommandConfig {
            lua: raw_cmd.lua,
            default_script: raw_cmd.default_script.unwrap_or(false),
            long_lived,
            bidi,
        };
        commands.insert(name, cmd);
    }
    Ok(HostExecConfig { commands })
}

fn resolve_proxy(raw: Option<RawProxyConfig>) -> ProxyConfig {
    match raw {
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
    }
}

fn resolve_network(raw: Option<RawNetworkConfig>) -> NetworkConfig {
    match raw {
        Some(n) => NetworkConfig {
            name: n.name.unwrap_or_else(|| DEFAULT_NETWORK_NAME.to_string()),
            worker_name: n
                .worker_name
                .unwrap_or_else(|| DEFAULT_WORKER_NETWORK_NAME.to_string()),
            server_hostname: n
                .server_hostname
                .unwrap_or_else(|| DEFAULT_SERVER_HOSTNAME.to_string()),
            worker_prefix: n
                .worker_prefix
                .unwrap_or_else(|| DEFAULT_WORKER_PREFIX.to_string()),
        },
        None => NetworkConfig {
            name: DEFAULT_NETWORK_NAME.to_string(),
            worker_name: DEFAULT_WORKER_NETWORK_NAME.to_string(),
            server_hostname: DEFAULT_SERVER_HOSTNAME.to_string(),
            worker_prefix: DEFAULT_WORKER_PREFIX.to_string(),
        },
    }
}

fn resolve_rag(raw: Option<RawRagConfig>) -> RagConfig {
    match raw {
        Some(r) => RagConfig {
            qdrant_hostname: r
                .qdrant_hostname
                .unwrap_or_else(|| DEFAULT_QDRANT_HOSTNAME.to_string()),
            embedding_model: r
                .embedding_model
                .unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string()),
            docs: r
                .docs
                .map(|d| RagDocsConfig { exclude: d.exclude })
                .unwrap_or_default(),
        },
        None => RagConfig {
            qdrant_hostname: DEFAULT_QDRANT_HOSTNAME.to_string(),
            embedding_model: DEFAULT_EMBEDDING_MODEL.to_string(),
            docs: RagDocsConfig::default(),
        },
    }
}

fn resolve_server(raw: Option<RawServerConfig>) -> ServerConfig {
    let container_command = match raw.as_ref().and_then(|s| s.container_command.clone()) {
        Some(cmd) => cmd,
        None => std::env::var(UR_CONTAINER_ENV)
            .unwrap_or_else(|_| DEFAULT_CONTAINER_COMMAND.to_string()),
    };
    match raw {
        Some(s) => ServerConfig {
            container_command,
            stale_worker_ttl_days: s
                .stale_worker_ttl_days
                .unwrap_or(DEFAULT_STALE_WORKER_TTL_DAYS),
            max_transition_attempts: s
                .max_transition_attempts
                .unwrap_or(DEFAULT_MAX_TRANSITION_ATTEMPTS),
            poll_interval_ms: s.poll_interval_ms.unwrap_or(DEFAULT_POLL_INTERVAL_MS),
            github_scan_interval_secs: s
                .github_scan_interval_secs
                .unwrap_or(DEFAULT_GITHUB_SCAN_INTERVAL_SECS),
        },
        None => ServerConfig {
            container_command,
            stale_worker_ttl_days: DEFAULT_STALE_WORKER_TTL_DAYS,
            max_transition_attempts: DEFAULT_MAX_TRANSITION_ATTEMPTS,
            poll_interval_ms: DEFAULT_POLL_INTERVAL_MS,
            github_scan_interval_secs: DEFAULT_GITHUB_SCAN_INTERVAL_SECS,
        },
    }
}

fn resolve_backup(raw: Option<RawBackupConfig>) -> BackupConfig {
    match raw {
        Some(b) => BackupConfig {
            path: b.path,
            interval_minutes: b
                .interval_minutes
                .unwrap_or(DEFAULT_BACKUP_INTERVAL_MINUTES),
            enabled: b.enabled.unwrap_or(true),
            retain_count: b.retain_count.unwrap_or(DEFAULT_BACKUP_RETAIN_COUNT),
        },
        None => BackupConfig {
            path: None,
            interval_minutes: DEFAULT_BACKUP_INTERVAL_MINUTES,
            enabled: true,
            retain_count: DEFAULT_BACKUP_RETAIN_COUNT,
        },
    }
}

/// Determine the config directory from `$UR_CONFIG` or fall back to `~/.ur`.
fn validate_project_templates(key: &str, raw_proj: &RawProjectConfig) -> anyhow::Result<()> {
    if let Some(ref tpl) = raw_proj.git_hooks_dir {
        template_path::validate_template_str(tpl)
            .map_err(|e| anyhow::anyhow!("project '{}': git_hooks_dir: {}", key, e))?;
    }
    if let Some(ref tpl) = raw_proj.workflow_hooks_dir {
        template_path::validate_template_str(tpl)
            .map_err(|e| anyhow::anyhow!("project '{}': workflow_hooks_dir: {}", key, e))?;
    }
    // Mount validation is handled by parse_mount_entry during config loading.
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
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialize tests that mutate process-wide env vars.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

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
        assert_eq!(cfg.server_port, DEFAULT_SERVER_PORT);
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
    fn reads_server_port_from_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "server_port = 9000\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.server_port, 9000);
    }

    #[test]
    fn ur_config_env_overrides_default() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        // SAFETY: serialized by ENV_MUTEX; no other test mutates this var concurrently.
        unsafe { std::env::set_var(UR_CONFIG_ENV, tmp.path()) };
        let dir = resolve_config_dir().unwrap();
        // SAFETY: serialized by ENV_MUTEX.
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
        std::fs::write(tmp.path().join("ur.toml"), "server_port = 5000\n").unwrap();
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
        std::fs::write(tmp.path().join("ur.toml"), "server_port = 5000\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, DEFAULT_NETWORK_NAME);
        assert_eq!(cfg.network.worker_name, DEFAULT_WORKER_NETWORK_NAME);
        assert_eq!(cfg.network.server_hostname, DEFAULT_SERVER_HOSTNAME);
        assert_eq!(cfg.network.worker_prefix, DEFAULT_WORKER_PREFIX);
    }

    #[test]
    fn network_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[network]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, DEFAULT_NETWORK_NAME);
        assert_eq!(cfg.network.worker_name, DEFAULT_WORKER_NETWORK_NAME);
        assert_eq!(cfg.network.server_hostname, DEFAULT_SERVER_HOSTNAME);
        assert_eq!(cfg.network.worker_prefix, DEFAULT_WORKER_PREFIX);
    }

    #[test]
    fn network_reads_custom_values() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[network]\nname = \"custom-net\"\nworker_name = \"custom-workers\"\nserver_hostname = \"my-server\"\nworker_prefix = \"test-worker-\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, "custom-net");
        assert_eq!(cfg.network.worker_name, "custom-workers");
        assert_eq!(cfg.network.server_hostname, "my-server");
        assert_eq!(cfg.network.worker_prefix, "test-worker-");
    }

    #[test]
    fn rag_defaults_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "server_port = 5000\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.rag.qdrant_hostname, DEFAULT_QDRANT_HOSTNAME);
    }

    #[test]
    fn rag_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[rag]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.rag.qdrant_hostname, DEFAULT_QDRANT_HOSTNAME);
    }

    #[test]
    fn rag_reads_custom_qdrant_hostname() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[rag]\nqdrant_hostname = \"my-qdrant\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.rag.qdrant_hostname, "my-qdrant");
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
[projects.ur.container]
image = "base"
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
        assert_eq!(proj.container.image, "ur-worker:latest");
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
[projects.swa.container]
image = "base"
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
[projects.ur.container]
image = "base"

[projects.swa]
repo = "git@github.com:cmaher/swa.git"
name = "Swa App"
pool_limit = 5
[projects.swa.container]
image = "rust"
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
hostexec = ["jq", "rg", "cargo"]
[projects.ur.container]
image = "base"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let proj = &cfg.projects["ur"];
        assert_eq!(proj.hostexec, vec!["jq", "rg", "cargo"]);
    }

    #[test]
    fn hostexec_defaults_to_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
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
[projects.ur.container]
image = "base"
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
[projects.ur.container]
image = "base"
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
[projects.ur.container]
image = "base"
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
[projects.ur.container]
image = "base"
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
[projects.ur.container]
image = "base"
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
[projects.ur.container]
image = "base"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.projects["ur"].container.mounts.is_empty());
    }

    #[test]
    fn mounts_parses_source_destination_format() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
mounts = ["%URCONFIG%/shared-data:/var/data", "/opt/tools:/workspace/.tools"]
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["ur"].container.mounts.len(), 2);
        assert_eq!(
            cfg.projects["ur"].container.mounts[0],
            MountConfig {
                source: "%URCONFIG%/shared-data".into(),
                destination: "/var/data".into(),
            }
        );
        assert_eq!(
            cfg.projects["ur"].container.mounts[1],
            MountConfig {
                source: "/opt/tools".into(),
                destination: "/workspace/.tools".into(),
            }
        );
    }

    #[test]
    fn mounts_rejects_missing_colon() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
mounts = ["/opt/tools"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mounts[0]"), "{msg}");
        assert!(msg.contains("source:destination"), "{msg}");
    }

    #[test]
    fn mounts_rejects_project_relative_source() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
mounts = ["%PROJECT%/.cache:/workspace/.cache"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mounts[0]"), "{msg}");
        assert!(msg.contains("%PROJECT%"), "{msg}");
    }

    #[test]
    fn mounts_rejects_relative_destination() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
mounts = ["/opt/tools:relative/path"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mounts[0]"), "{msg}");
        assert!(msg.contains("absolute path"), "{msg}");
    }

    #[test]
    fn rag_embedding_model_defaults_when_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.rag.embedding_model, DEFAULT_EMBEDDING_MODEL);
    }

    #[test]
    fn rag_reads_custom_embedding_model() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[rag]\nembedding_model = \"custom-model\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.rag.embedding_model, "custom-model");
    }

    #[test]
    fn rag_embedding_model_defaults_when_rag_section_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[rag]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.rag.embedding_model, DEFAULT_EMBEDDING_MODEL);
    }

    #[test]
    fn rag_docs_exclude_defaults_to_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.rag.docs.exclude.is_empty());
    }

    #[test]
    fn rag_docs_exclude_defaults_when_rag_section_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[rag]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.rag.docs.exclude.is_empty());
    }

    #[test]
    fn rag_docs_exclude_reads_values() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[rag.docs]\nexclude = [\"tokio\", \"serde\"]\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.rag.docs.exclude, vec!["tokio", "serde"]);
    }

    #[test]
    fn rag_docs_section_empty_exclude_defaults() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[rag.docs]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.rag.docs.exclude.is_empty());
    }

    #[test]
    fn mounts_rejects_invalid_source_variable() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
mounts = ["%INVALID%/bad:/workspace/bad"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mounts[0]"), "{msg}");
        assert!(msg.contains("unrecognized template variable"), "{msg}");
    }

    #[test]
    fn hostexec_defaults_to_empty_when_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.hostexec.commands.is_empty());
    }

    #[test]
    fn hostexec_parses_passthrough_command() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[hostexec.commands]
cargo = {}
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.hostexec.commands.len(), 1);
        let cargo = &cfg.hostexec.commands["cargo"];
        assert_eq!(cargo.lua, None);
        assert!(!cargo.default_script);
    }

    #[test]
    fn hostexec_parses_command_with_lua() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[hostexec.commands]
git = { lua = "my-git.lua" }
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let git = &cfg.hostexec.commands["git"];
        assert_eq!(git.lua.as_deref(), Some("my-git.lua"));
        assert!(!git.default_script);
    }

    #[test]
    fn hostexec_parses_command_with_default_script() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[hostexec.commands]
git = { default_script = true }
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let git = &cfg.hostexec.commands["git"];
        assert!(git.default_script);
        assert_eq!(git.lua, None);
    }

    #[test]
    fn hostexec_parses_multiple_commands() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[hostexec.commands]
cargo = {}
jq = {}
rg = { lua = "rg-safe.lua" }
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.hostexec.commands.len(), 3);
        assert!(cfg.hostexec.commands.contains_key("cargo"));
        assert!(cfg.hostexec.commands.contains_key("jq"));
        assert!(cfg.hostexec.commands.contains_key("rg"));
    }

    #[test]
    fn backup_defaults_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.backup.path, None);
        assert_eq!(cfg.backup.interval_minutes, DEFAULT_BACKUP_INTERVAL_MINUTES);
        assert!(cfg.backup.enabled);
        assert_eq!(cfg.backup.retain_count, DEFAULT_BACKUP_RETAIN_COUNT);
    }

    #[test]
    fn backup_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[backup]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.backup.path, None);
        assert_eq!(cfg.backup.interval_minutes, DEFAULT_BACKUP_INTERVAL_MINUTES);
        assert!(cfg.backup.enabled);
        assert_eq!(cfg.backup.retain_count, DEFAULT_BACKUP_RETAIN_COUNT);
    }

    #[test]
    fn backup_reads_path_and_interval() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[backup]\npath = \"/backups/ur\"\ninterval_minutes = 60\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.backup.path,
            Some(std::path::PathBuf::from("/backups/ur"))
        );
        assert_eq!(cfg.backup.interval_minutes, 60);
    }

    #[test]
    fn backup_reads_path_with_default_interval() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[backup]\npath = \"/backups/ur\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.backup.path,
            Some(std::path::PathBuf::from("/backups/ur"))
        );
        assert_eq!(cfg.backup.interval_minutes, DEFAULT_BACKUP_INTERVAL_MINUTES);
    }

    #[test]
    fn worker_port_defaults_to_server_port_plus_one() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.worker_port, DEFAULT_SERVER_PORT + 1);
    }

    #[test]
    fn worker_port_follows_custom_server_port() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "server_port = 9000\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.worker_port, 9001);
    }

    #[test]
    fn worker_port_reads_explicit_value() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "server_port = 9000\nworker_port = 8000\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.worker_port, 8000);
    }

    #[test]
    fn backup_enabled_false_disables() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[backup]\npath = \"/backups/ur\"\nenabled = false\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(!cfg.backup.enabled);
    }

    #[test]
    fn backup_retain_count_configurable() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[backup]\npath = \"/backups/ur\"\nretain_count = 7\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.backup.retain_count, 7);
    }

    #[test]
    fn hostexec_long_lived_defaults_false() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[hostexec.commands]
cargo = {}
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let cargo = &cfg.hostexec.commands["cargo"];
        assert!(!cargo.long_lived);
        assert!(!cargo.bidi);
    }

    #[test]
    fn hostexec_long_lived_parses_true() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[hostexec.commands]
daemon = { long_lived = true }
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let daemon = &cfg.hostexec.commands["daemon"];
        assert!(daemon.long_lived);
        assert!(!daemon.bidi);
    }

    #[test]
    fn hostexec_bidi_with_long_lived_parses() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[hostexec.commands]
daemon = { long_lived = true, bidi = true }
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let daemon = &cfg.hostexec.commands["daemon"];
        assert!(daemon.long_lived);
        assert!(daemon.bidi);
    }

    #[test]
    fn hostexec_bidi_without_long_lived_errors() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[hostexec.commands]
bad = { bidi = true }
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("bidi = true requires long_lived = true"),
            "{msg}"
        );
    }

    #[test]
    fn project_workflow_fields_default() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.myproj]
repo = "git@github.com:example/myproj.git"
[projects.myproj.container]
image = "base"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let proj = &cfg.projects["myproj"];
        assert_eq!(proj.workflow_hooks_dir, None);
        assert_eq!(proj.max_fix_attempts, DEFAULT_MAX_FIX_ATTEMPTS);
        assert_eq!(proj.protected_branches, default_protected_branches());
    }

    #[test]
    fn project_workflow_fields_custom() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.myproj]
repo = "git@github.com:example/myproj.git"
workflow_hooks_dir = "%PROJECT%/.workflow"
max_fix_attempts = 3
protected_branches = ["main", "release/*"]
[projects.myproj.container]
image = "base"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let proj = &cfg.projects["myproj"];
        assert_eq!(
            proj.workflow_hooks_dir.as_deref(),
            Some("%PROJECT%/.workflow")
        );
        assert_eq!(proj.max_fix_attempts, 3);
        assert_eq!(proj.protected_branches, vec!["main", "release/*"]);
    }

    #[test]
    fn project_workflow_hooks_dir_validates_template() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.myproj]
repo = "git@github.com:example/myproj.git"
workflow_hooks_dir = "relative/path"
[projects.myproj.container]
image = "base"
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("workflow_hooks_dir"),
            "expected workflow_hooks_dir error, got: {msg}"
        );
    }

    #[test]
    fn hostexec_bidi_false_with_long_lived_false_ok() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[hostexec.commands]
tool = { long_lived = false, bidi = false }
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let tool = &cfg.hostexec.commands["tool"];
        assert!(!tool.long_lived);
        assert!(!tool.bidi);
    }

    #[test]
    fn container_image_alias_base_resolves() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["ur"].container.image, "ur-worker:latest");
    }

    #[test]
    fn container_image_alias_rust_resolves() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "rust"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["ur"].container.image, "ur-worker-rust:latest");
    }

    #[test]
    fn container_image_full_reference_passes_through() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "myregistry/custom-image:v1.2.3"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.projects["ur"].container.image,
            "myregistry/custom-image:v1.2.3"
        );
    }

    #[test]
    fn container_image_unknown_alias_errors() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "unknown"
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown alias 'unknown'"), "{msg}");
        assert!(msg.contains("base"), "{msg}");
        assert!(msg.contains("rust"), "{msg}");
    }

    #[test]
    fn project_missing_container_section_errors() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing required"), "{msg}");
        assert!(msg.contains("[projects.ur.container]"), "{msg}");
    }

    #[test]
    fn mounts_at_project_root_errors() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
mounts = ["/opt/tools:/workspace/.tools"]
[projects.ur.container]
image = "base"
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("must be inside [projects.ur.container]"),
            "{msg}"
        );
    }

    #[test]
    fn ports_defaults_to_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.projects["ur"].container.ports.is_empty());
    }

    #[test]
    fn ports_parses_host_container_format() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
ports = ["8080:80", "3000:3000"]
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["ur"].container.ports.len(), 2);
        assert_eq!(
            cfg.projects["ur"].container.ports[0],
            PortMapping {
                host_port: 8080,
                container_port: 80,
            }
        );
        assert_eq!(
            cfg.projects["ur"].container.ports[1],
            PortMapping {
                host_port: 3000,
                container_port: 3000,
            }
        );
    }

    #[test]
    fn ports_rejects_missing_colon() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
ports = ["8080"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ports[0]"), "{msg}");
        assert!(msg.contains("host_port:container_port"), "{msg}");
    }

    #[test]
    fn ports_rejects_invalid_host_port() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
ports = ["notaport:80"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ports[0]"), "{msg}");
        assert!(msg.contains("invalid host port"), "{msg}");
    }

    #[test]
    fn ports_rejects_invalid_container_port() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "base"
ports = ["8080:notaport"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ports[0]"), "{msg}");
        assert!(msg.contains("invalid container port"), "{msg}");
    }

    #[test]
    fn server_defaults_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        // Set container_command explicitly to avoid env var race conditions
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[server]\ncontainer_command = \"docker\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.server.container_command, DEFAULT_CONTAINER_COMMAND);
        assert_eq!(
            cfg.server.stale_worker_ttl_days,
            DEFAULT_STALE_WORKER_TTL_DAYS
        );
        assert_eq!(
            cfg.server.max_transition_attempts,
            DEFAULT_MAX_TRANSITION_ATTEMPTS
        );
        assert_eq!(cfg.server.poll_interval_ms, DEFAULT_POLL_INTERVAL_MS);
        assert_eq!(
            cfg.server.github_scan_interval_secs,
            DEFAULT_GITHUB_SCAN_INTERVAL_SECS
        );
    }

    #[test]
    fn server_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        // Set container_command explicitly to avoid env var race conditions
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[server]\ncontainer_command = \"docker\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.server.container_command, DEFAULT_CONTAINER_COMMAND);
        assert_eq!(
            cfg.server.stale_worker_ttl_days,
            DEFAULT_STALE_WORKER_TTL_DAYS
        );
        assert_eq!(
            cfg.server.max_transition_attempts,
            DEFAULT_MAX_TRANSITION_ATTEMPTS
        );
        assert_eq!(cfg.server.poll_interval_ms, DEFAULT_POLL_INTERVAL_MS);
        assert_eq!(
            cfg.server.github_scan_interval_secs,
            DEFAULT_GITHUB_SCAN_INTERVAL_SECS
        );
    }

    #[test]
    fn server_reads_partial_overrides() {
        let tmp = TempDir::new().unwrap();
        // Set container_command explicitly to avoid env var race conditions
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[server]\ncontainer_command = \"docker\"\nstale_worker_ttl_days = 14\npoll_interval_ms = 1000\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.server.container_command, DEFAULT_CONTAINER_COMMAND);
        assert_eq!(cfg.server.stale_worker_ttl_days, 14);
        assert_eq!(
            cfg.server.max_transition_attempts,
            DEFAULT_MAX_TRANSITION_ATTEMPTS
        );
        assert_eq!(cfg.server.poll_interval_ms, 1000);
        assert_eq!(
            cfg.server.github_scan_interval_secs,
            DEFAULT_GITHUB_SCAN_INTERVAL_SECS
        );
    }

    #[test]
    fn server_reads_all_custom_values() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[server]
container_command = "nerdctl"
stale_worker_ttl_days = 30
max_transition_attempts = 5
poll_interval_ms = 2000
github_scan_interval_secs = 60
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.server.container_command, "nerdctl");
        assert_eq!(cfg.server.stale_worker_ttl_days, 30);
        assert_eq!(cfg.server.max_transition_attempts, 5);
        assert_eq!(cfg.server.poll_interval_ms, 2000);
        assert_eq!(cfg.server.github_scan_interval_secs, 60);
    }

    #[test]
    fn server_container_command_falls_back_to_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        // SAFETY: serialized by ENV_MUTEX; no other test mutates this var concurrently.
        unsafe { std::env::set_var(UR_CONTAINER_ENV, "nerdctl") };
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        // SAFETY: serialized by ENV_MUTEX.
        unsafe { std::env::remove_var(UR_CONTAINER_ENV) };
        assert_eq!(cfg.server.container_command, "nerdctl");
    }

    #[test]
    fn server_container_command_toml_overrides_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        // SAFETY: serialized by ENV_MUTEX; no other test mutates this var concurrently.
        unsafe { std::env::set_var(UR_CONTAINER_ENV, "nerdctl") };
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[server]\ncontainer_command = \"podman\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        // SAFETY: serialized by ENV_MUTEX.
        unsafe { std::env::remove_var(UR_CONTAINER_ENV) };
        assert_eq!(cfg.server.container_command, "podman");
    }
}
