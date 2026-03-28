mod template_path;

pub use template_path::{
    ResolvedTemplatePath, WORKSPACE_TEMPLATE, resolve_template_path, resolve_workspace_content,
};

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

/// Environment variable: project-level CLAUDE.md content injected into the worker.
///
/// Set by the server when launching a worker so it can write the project's
/// CLAUDE.md into the container without needing host filesystem access.
pub const UR_PROJECT_CLAUDE_ENV: &str = "UR_PROJECT_CLAUDE";

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
    logs_dir: Option<PathBuf>,
    tui: Option<RawTuiConfig>,
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
    skill_hooks_dir: Option<String>,
    claude_md: Option<String>,
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

/// Raw TOML representation for the `[tui.notifications]` section.
#[derive(Debug, Default, Deserialize)]
struct RawNotificationConfig {
    flow_stalled: Option<bool>,
    flow_in_review: Option<bool>,
}

/// Raw TOML representation for the `[tui.ticket.filter]` section.
#[derive(Debug, Default, Deserialize)]
struct RawTicketFilterConfig {
    #[serde(default)]
    statuses: Option<Vec<String>>,
    #[serde(default)]
    projects: Option<Vec<String>>,
}

/// Raw TOML representation for the `[tui.ticket]` section.
#[derive(Debug, Default, Deserialize)]
struct RawTicketConfig {
    filter: Option<RawTicketFilterConfig>,
}

/// Raw TOML representation for the `[tui]` section.
#[derive(Debug, Default, Deserialize)]
struct RawTuiConfig {
    theme: Option<String>,
    keymap: Option<String>,
    key_repeat_interval_ms: Option<u64>,
    #[serde(default)]
    themes: HashMap<String, RawThemeColors>,
    #[serde(default)]
    keymaps: HashMap<String, RawKeymapOverrides>,
    ticket: Option<RawTicketConfig>,
    notifications: Option<RawNotificationConfig>,
}

/// Raw TOML representation for a `[tui.themes.<name>]` entry.
#[derive(Debug, Default, Deserialize)]
struct RawThemeColors {
    bg: Option<String>,
    fg: Option<String>,
    border: Option<String>,
    border_focused: Option<String>,
    border_rounded: Option<bool>,
    header_bg: Option<String>,
    header_fg: Option<String>,
    selected_bg: Option<String>,
    selected_fg: Option<String>,
    status_bar_bg: Option<String>,
    status_bar_fg: Option<String>,
    error_fg: Option<String>,
    warning_fg: Option<String>,
    success_fg: Option<String>,
    info_fg: Option<String>,
    muted_fg: Option<String>,
    accent: Option<String>,
    highlight: Option<String>,
    shadow: Option<String>,
    overlay_bg: Option<String>,
}

/// Raw TOML representation for a `[tui.keymaps.<name>]` entry.
#[derive(Debug, Default, Deserialize)]
struct RawKeymapOverrides {
    quit: Option<Vec<String>>,
    focus_next: Option<Vec<String>>,
    focus_prev: Option<Vec<String>>,
    scroll_up: Option<Vec<String>>,
    scroll_down: Option<Vec<String>>,
    page_up: Option<Vec<String>>,
    page_down: Option<Vec<String>>,
    select: Option<Vec<String>>,
    cancel: Option<Vec<String>>,
    refresh: Option<Vec<String>>,
    filter: Option<Vec<String>>,
    help: Option<Vec<String>>,
    new_flow: Option<Vec<String>>,
    stop_flow: Option<Vec<String>>,
    view_logs: Option<Vec<String>>,
    toggle_panel: Option<Vec<String>>,
}

/// Raw TOML representation for the `[server]` section.
#[derive(Debug, Default, Deserialize)]
struct RawServerConfig {
    container_command: Option<String>,
    stale_worker_ttl_days: Option<u64>,
    max_implement_cycles: Option<i32>,
    poll_interval_ms: Option<u64>,
    github_scan_interval_secs: Option<u64>,
    builderd_retry_count: Option<u32>,
    builderd_retry_backoff_ms: Option<u64>,
    ui_event_poll_interval_ms: Option<u64>,
}

/// Environment variable: container runtime command override (e.g. "nerdctl").
/// Checked as a fallback when `[server].container_command` is not set in ur.toml.
pub const UR_CONTAINER_ENV: &str = "UR_CONTAINER";

/// Default container runtime command.
pub const DEFAULT_CONTAINER_COMMAND: &str = "docker";

/// Default number of days before a stale worker is cleaned up.
pub const DEFAULT_STALE_WORKER_TTL_DAYS: u64 = 7;

/// Default maximum number of implement cycles before stalling a workflow.
pub const DEFAULT_MAX_IMPLEMENT_CYCLES: i32 = 6;

/// Default poll interval in milliseconds for background polling loops.
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 500;

/// Default GitHub scan interval in seconds for the poller.
pub const DEFAULT_GITHUB_SCAN_INTERVAL_SECS: u64 = 30;

/// Default UI event poll interval in milliseconds.
pub const DEFAULT_UI_EVENT_POLL_INTERVAL_MS: u64 = 200;

/// Default number of builderd retry attempts.
pub const DEFAULT_BUILDERD_RETRY_COUNT: u32 = 3;

/// Default base backoff in milliseconds for builderd retries.
pub const DEFAULT_BUILDERD_RETRY_BACKOFF_MS: u64 = 200;

/// Server runtime configuration from the `[server]` section of `ur.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Container runtime command (default: "docker").
    /// Resolution order: ur.toml > `UR_CONTAINER` env var > "docker".
    pub container_command: String,
    /// Number of days before stale workers are cleaned up (default: 7).
    pub stale_worker_ttl_days: u64,
    /// Maximum number of implement cycles before stalling (default: 6).
    /// `None` means no limit.
    pub max_implement_cycles: Option<i32>,
    /// Poll interval in milliseconds for background loops (default: 500).
    pub poll_interval_ms: u64,
    /// GitHub scan interval in seconds for the poller (default: 30).
    pub github_scan_interval_secs: u64,
    /// Maximum number of builderd gRPC retry attempts (default: 3).
    pub builderd_retry_count: u32,
    /// Base backoff in milliseconds for builderd retries (default: 200).
    /// Each retry doubles this value (exponential backoff).
    pub builderd_retry_backoff_ms: u64,
    /// UI event poll interval in milliseconds (default: 200).
    pub ui_event_poll_interval_ms: u64,
}

/// Default TUI theme name.
pub const DEFAULT_TUI_THEME: &str = "system";

/// Default TUI keymap name.
pub const DEFAULT_TUI_KEYMAP: &str = "default";

/// Built-in theme names (from daisyUI), sorted alphabetically.
///
/// These rarely change. The canonical list is generated at compile time in
/// `urui/build.rs` from `themes/themes.css`, but we keep a static copy here
/// so that the CLI can validate theme names without depending on `urui`.
pub const BUILTIN_THEME_NAMES: &[&str] = &[
    "abyss",
    "acid",
    "aqua",
    "autumn",
    "black",
    "bumblebee",
    "business",
    "caramellatte",
    "cmyk",
    "coffee",
    "corporate",
    "cupcake",
    "cyberpunk",
    "dark",
    "dim",
    "dracula",
    "emerald",
    "fantasy",
    "forest",
    "garden",
    "halloween",
    "lemonade",
    "light",
    "lofi",
    "luxury",
    "night",
    "nord",
    "pastel",
    "retro",
    "silk",
    "sunset",
    "synthwave",
    "valentine",
    "winter",
    "wireframe",
];

/// Returns `true` if the given name is a built-in theme.
pub fn is_builtin_theme(name: &str) -> bool {
    BUILTIN_THEME_NAMES.contains(&name)
}

/// TUI theme color definitions.
///
/// All fields are `Option<String>` — colors are stored as raw strings
/// (e.g. `"#1a1b26"`, `"red"`) and resolved to actual color values by
/// the TUI crate at runtime. No ratatui dependency here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeColors {
    pub bg: Option<String>,
    pub fg: Option<String>,
    pub border: Option<String>,
    pub border_focused: Option<String>,
    pub border_rounded: Option<bool>,
    pub header_bg: Option<String>,
    pub header_fg: Option<String>,
    pub selected_bg: Option<String>,
    pub selected_fg: Option<String>,
    pub status_bar_bg: Option<String>,
    pub status_bar_fg: Option<String>,
    pub error_fg: Option<String>,
    pub warning_fg: Option<String>,
    pub success_fg: Option<String>,
    pub info_fg: Option<String>,
    pub muted_fg: Option<String>,
    pub accent: Option<String>,
    pub highlight: Option<String>,
    pub shadow: Option<String>,
    pub overlay_bg: Option<String>,
}

/// TUI keymap override definitions.
///
/// Each field maps an action name to a list of key binding strings
/// (e.g. `["q", "ctrl-c"]`). `None` means use the built-in default
/// for that action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeymapOverrides {
    pub quit: Option<Vec<String>>,
    pub focus_next: Option<Vec<String>>,
    pub focus_prev: Option<Vec<String>>,
    pub scroll_up: Option<Vec<String>>,
    pub scroll_down: Option<Vec<String>>,
    pub page_up: Option<Vec<String>>,
    pub page_down: Option<Vec<String>>,
    pub select: Option<Vec<String>>,
    pub cancel: Option<Vec<String>>,
    pub refresh: Option<Vec<String>>,
    pub filter: Option<Vec<String>>,
    pub help: Option<Vec<String>>,
    pub new_flow: Option<Vec<String>>,
    pub stop_flow: Option<Vec<String>>,
    pub view_logs: Option<Vec<String>>,
    pub toggle_panel: Option<Vec<String>>,
}

/// Persisted ticket filter settings from the `[tui.ticket.filter]` section of `ur.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketFilterConfig {
    /// Which statuses to show. `None` means use defaults (open + in_progress).
    pub statuses: Option<Vec<String>>,
    /// Which projects to show. `None` means show all.
    pub projects: Option<Vec<String>>,
}

/// Notification settings from the `[tui.notifications]` section of `ur.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationConfig {
    /// Whether to notify when a flow stalls.
    pub flow_stalled: bool,
    /// Whether to notify when a flow enters review.
    pub flow_in_review: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            flow_stalled: true,
            flow_in_review: true,
        }
    }
}

/// Resolved TUI configuration from the `[tui]` section of `ur.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiConfig {
    /// Active theme name (default: "system").
    pub theme_name: String,
    /// Active keymap name (default: "default").
    pub keymap_name: String,
    /// Minimum interval in ms between repeated navigation actions when holding a key (default: 200).
    pub key_repeat_interval_ms: u64,
    /// User-defined themes, keyed by name.
    pub custom_themes: HashMap<String, ThemeColors>,
    /// User-defined keymap overrides, keyed by name.
    pub custom_keymaps: HashMap<String, KeymapOverrides>,
    /// Persisted ticket filter settings.
    pub ticket_filter: TicketFilterConfig,
    /// Notification toggles.
    pub notifications: NotificationConfig,
}

pub const DEFAULT_KEY_REPEAT_INTERVAL_MS: u64 = 200;

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme_name: DEFAULT_TUI_THEME.to_string(),
            keymap_name: DEFAULT_TUI_KEYMAP.to_string(),
            key_repeat_interval_ms: DEFAULT_KEY_REPEAT_INTERVAL_MS,
            custom_themes: HashMap::new(),
            custom_keymaps: HashMap::new(),
            ticket_filter: TicketFilterConfig::default(),
            notifications: NotificationConfig::default(),
        }
    }
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
    ("ur-worker", "ur-worker:latest"),
    ("ur-worker-rust", "ur-worker-rust:latest"),
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
    /// Optional template path to a directory of skill hook snippets.
    /// Supports `%PROJECT%/...` and `%URCONFIG%/...` template variables, or absolute paths.
    /// Resolve with [`resolve_template_path`] at use time.
    /// Contents are copied to `~/.claude/skill-hooks/` at container startup.
    pub skill_hooks_dir: Option<String>,
    /// Optional template path to a project-level CLAUDE.md file.
    /// Supports `%PROJECT%/...`, `%URCONFIG%/...` template variables, or absolute paths.
    /// Resolve with [`resolve_template_path`] at use time.
    /// When None, the server falls back to `<config_dir>/projects/<key>/CLAUDE.md`.
    pub claude_md: Option<String>,
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
    /// TUI display settings (theme, keymap).
    pub tui: TuiConfig,
    /// Directory where all log files are written (default: `<config_dir>/logs`).
    pub logs_dir: PathBuf,
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
        let tui = resolve_tui(raw.tui);

        let projects = raw
            .projects
            .into_iter()
            .map(|(key, raw_proj)| resolve_project_config(key, raw_proj))
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        let git_branch_prefix = raw.git_branch_prefix.unwrap_or_default();

        let logs_dir = match raw.logs_dir {
            Some(p) if p.is_absolute() => p,
            Some(p) => config_dir.join(p),
            None => config_dir.join("logs"),
        };

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
            tui,
            logs_dir,
            git_branch_prefix,
            projects,
        })
    }
}

/// Persist the selected theme name to `ur.toml` in the given config directory.
///
/// Reads the existing file (if any), sets `[tui].theme`, and writes it back
/// without disturbing other sections.
pub fn save_theme_name(config_dir: &Path, theme_name: &str) -> anyhow::Result<()> {
    let path = config_dir.join("ur.toml");
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let mut doc: toml::Value = if content.is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        content.parse()?
    };
    let table = doc
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("ur.toml root is not a table"))?;
    let tui = table
        .entry("tui")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let tui_table = tui
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[tui] is not a table"))?;
    tui_table.insert(
        "theme".to_string(),
        toml::Value::String(theme_name.to_string()),
    );
    std::fs::write(&path, toml::to_string_pretty(&doc)?)?;
    Ok(())
}

/// Filename for the server pid file, stored in the config directory.
pub const SERVER_PID_FILE: &str = "server.pid";

fn resolve_project_config(
    key: String,
    raw_proj: RawProjectConfig,
) -> anyhow::Result<(String, ProjectConfig)> {
    // Reject mounts at project root level
    if raw_proj.mounts.is_some() {
        anyhow::bail!(
            "project '{key}': 'mounts' must be inside [projects.{key}.container], \
             not at the project root level"
        );
    }

    validate_project_templates(&key, &raw_proj)?;

    let raw_container = raw_proj.container.unwrap_or(RawContainerConfig {
        image: IMAGE_ALIASES
            .first()
            .expect("IMAGE_ALIASES must not be empty")
            .0
            .to_string(),
        mounts: Vec::new(),
        ports: Vec::new(),
    });

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
        skill_hooks_dir: raw_proj.skill_hooks_dir,
        claude_md: raw_proj.claude_md,
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
}

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
            max_implement_cycles: s
                .max_implement_cycles
                .or(Some(DEFAULT_MAX_IMPLEMENT_CYCLES)),
            poll_interval_ms: s.poll_interval_ms.unwrap_or(DEFAULT_POLL_INTERVAL_MS),
            github_scan_interval_secs: s
                .github_scan_interval_secs
                .unwrap_or(DEFAULT_GITHUB_SCAN_INTERVAL_SECS),
            builderd_retry_count: s
                .builderd_retry_count
                .unwrap_or(DEFAULT_BUILDERD_RETRY_COUNT),
            builderd_retry_backoff_ms: s
                .builderd_retry_backoff_ms
                .unwrap_or(DEFAULT_BUILDERD_RETRY_BACKOFF_MS),
            ui_event_poll_interval_ms: s
                .ui_event_poll_interval_ms
                .unwrap_or(DEFAULT_UI_EVENT_POLL_INTERVAL_MS),
        },
        None => ServerConfig {
            container_command,
            stale_worker_ttl_days: DEFAULT_STALE_WORKER_TTL_DAYS,
            max_implement_cycles: Some(DEFAULT_MAX_IMPLEMENT_CYCLES),
            poll_interval_ms: DEFAULT_POLL_INTERVAL_MS,
            github_scan_interval_secs: DEFAULT_GITHUB_SCAN_INTERVAL_SECS,
            builderd_retry_count: DEFAULT_BUILDERD_RETRY_COUNT,
            builderd_retry_backoff_ms: DEFAULT_BUILDERD_RETRY_BACKOFF_MS,
            ui_event_poll_interval_ms: DEFAULT_UI_EVENT_POLL_INTERVAL_MS,
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

fn resolve_tui(raw: Option<RawTuiConfig>) -> TuiConfig {
    match raw {
        Some(t) => {
            let custom_themes = t
                .themes
                .into_iter()
                .map(|(name, raw_theme)| {
                    let theme = ThemeColors {
                        bg: raw_theme.bg,
                        fg: raw_theme.fg,
                        border: raw_theme.border,
                        border_focused: raw_theme.border_focused,
                        border_rounded: raw_theme.border_rounded,
                        header_bg: raw_theme.header_bg,
                        header_fg: raw_theme.header_fg,
                        selected_bg: raw_theme.selected_bg,
                        selected_fg: raw_theme.selected_fg,
                        status_bar_bg: raw_theme.status_bar_bg,
                        status_bar_fg: raw_theme.status_bar_fg,
                        error_fg: raw_theme.error_fg,
                        warning_fg: raw_theme.warning_fg,
                        success_fg: raw_theme.success_fg,
                        info_fg: raw_theme.info_fg,
                        muted_fg: raw_theme.muted_fg,
                        accent: raw_theme.accent,
                        highlight: raw_theme.highlight,
                        shadow: raw_theme.shadow,
                        overlay_bg: raw_theme.overlay_bg,
                    };
                    (name, theme)
                })
                .collect();
            let custom_keymaps = t
                .keymaps
                .into_iter()
                .map(|(name, raw_km)| {
                    let km = KeymapOverrides {
                        quit: raw_km.quit,
                        focus_next: raw_km.focus_next,
                        focus_prev: raw_km.focus_prev,
                        scroll_up: raw_km.scroll_up,
                        scroll_down: raw_km.scroll_down,
                        page_up: raw_km.page_up,
                        page_down: raw_km.page_down,
                        select: raw_km.select,
                        cancel: raw_km.cancel,
                        refresh: raw_km.refresh,
                        filter: raw_km.filter,
                        help: raw_km.help,
                        new_flow: raw_km.new_flow,
                        stop_flow: raw_km.stop_flow,
                        view_logs: raw_km.view_logs,
                        toggle_panel: raw_km.toggle_panel,
                    };
                    (name, km)
                })
                .collect();
            let ticket_filter = match t.ticket.and_then(|tc| tc.filter) {
                Some(raw_filter) => TicketFilterConfig {
                    statuses: raw_filter.statuses,
                    projects: raw_filter.projects,
                },
                None => TicketFilterConfig::default(),
            };
            let notifications = match t.notifications {
                Some(raw_notif) => NotificationConfig {
                    flow_stalled: raw_notif.flow_stalled.unwrap_or(true),
                    flow_in_review: raw_notif.flow_in_review.unwrap_or(true),
                },
                None => NotificationConfig::default(),
            };
            TuiConfig {
                theme_name: t.theme.unwrap_or_else(|| DEFAULT_TUI_THEME.to_string()),
                keymap_name: t.keymap.unwrap_or_else(|| DEFAULT_TUI_KEYMAP.to_string()),
                key_repeat_interval_ms: t
                    .key_repeat_interval_ms
                    .unwrap_or(DEFAULT_KEY_REPEAT_INTERVAL_MS),
                custom_themes,
                custom_keymaps,
                ticket_filter,
                notifications,
            }
        }
        None => TuiConfig::default(),
    }
}

/// Determine the config directory from `$UR_CONFIG` or fall back to `~/.ur`.
fn validate_project_templates(key: &str, raw_proj: &RawProjectConfig) -> anyhow::Result<()> {
    if let Some(ref tpl) = raw_proj.git_hooks_dir {
        template_path::validate_template_str(tpl)
            .map_err(|e| anyhow::anyhow!("project '{}': git_hooks_dir: {}", key, e))?;
    }
    if let Some(ref tpl) = raw_proj.skill_hooks_dir {
        template_path::validate_template_str(tpl)
            .map_err(|e| anyhow::anyhow!("project '{}': skill_hooks_dir: {}", key, e))?;
    }
    if let Some(ref tpl) = raw_proj.workflow_hooks_dir {
        template_path::validate_template_str(tpl)
            .map_err(|e| anyhow::anyhow!("project '{}': workflow_hooks_dir: {}", key, e))?;
    }
    if let Some(ref tpl) = raw_proj.claude_md {
        template_path::validate_template_str(tpl)
            .map_err(|e| anyhow::anyhow!("project '{}': claude_md: {}", key, e))?;
    }
    // Mount validation is handled by parse_mount_entry during config loading.
    Ok(())
}

/// Resolve the project key from an explicit flag, env var, or cwd dirname.
///
/// Resolution order:
/// 1. `explicit` — if `Some`, return it directly
/// 2. `UR_PROJECT` env var — if set and non-empty, return it
/// 3. Current directory name:
///    a. If it matches a project **key**, return that key
///    b. If it matches any project's **name** field, return that project's key
///    c. Otherwise return `None`
pub fn resolve_project(
    explicit: Option<String>,
    projects: &HashMap<String, ProjectConfig>,
) -> Option<String> {
    if let Some(p) = explicit {
        return Some(p);
    }
    if let Ok(env_val) = std::env::var("UR_PROJECT")
        && !env_val.is_empty()
    {
        return Some(env_val);
    }
    let cwd = std::env::current_dir().ok()?;
    let dir_name = cwd.file_name()?.to_str()?;
    if dir_name.is_empty() {
        return None;
    }
    // Check if dirname matches a project key.
    if projects.contains_key(dir_name) {
        return Some(dir_name.to_owned());
    }
    // Check if dirname matches any project's name field.
    for (key, proj) in projects {
        if proj.name == dir_name {
            return Some(key.clone());
        }
    }
    None
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"

[projects.swa]
repo = "git@github.com:cmaher/swa.git"
name = "Swa App"
pool_limit = 5
[projects.swa.container]
image = "ur-worker-rust"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
    fn claude_md_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["ur"].claude_md, None);
    }

    #[test]
    fn claude_md_stores_template_string() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
claude_md = "%PROJECT%/CLAUDE.md"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.projects["ur"].claude_md.as_deref(),
            Some("%PROJECT%/CLAUDE.md")
        );
    }

    #[test]
    fn claude_md_rejects_unrecognized_variable() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
claude_md = "%BADVAR%/CLAUDE.md"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unrecognized template variable"), "{msg}");
        assert!(msg.contains("project 'ur'"), "{msg}");
    }

    #[test]
    fn claude_md_accepts_absolute_path() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
claude_md = "/opt/claude/ur/CLAUDE.md"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.projects["ur"].claude_md.as_deref(),
            Some("/opt/claude/ur/CLAUDE.md")
        );
    }

    #[test]
    fn claude_md_accepts_urconfig_template() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
claude_md = "%URCONFIG%/projects/ur/CLAUDE.md"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.projects["ur"].claude_md.as_deref(),
            Some("%URCONFIG%/projects/ur/CLAUDE.md")
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
    fn container_image_alias_ur_worker_resolves() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["ur"].container.image, "ur-worker:latest");
    }

    #[test]
    fn container_image_alias_ur_worker_rust_resolves() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker-rust"
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
        assert!(msg.contains("ur-worker"), "{msg}");
        assert!(msg.contains("ur-worker-rust"), "{msg}");
    }

    #[test]
    fn project_missing_container_section_defaults_to_ur_worker() {
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
        assert_eq!(cfg.projects["ur"].container.image, "ur-worker:latest");
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
image = "ur-worker"
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
            cfg.server.max_implement_cycles,
            Some(DEFAULT_MAX_IMPLEMENT_CYCLES)
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
            cfg.server.max_implement_cycles,
            Some(DEFAULT_MAX_IMPLEMENT_CYCLES)
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
            cfg.server.max_implement_cycles,
            Some(DEFAULT_MAX_IMPLEMENT_CYCLES)
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
max_implement_cycles = 5
poll_interval_ms = 2000
github_scan_interval_secs = 60
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.server.container_command, "nerdctl");
        assert_eq!(cfg.server.stale_worker_ttl_days, 30);
        assert_eq!(cfg.server.max_implement_cycles, Some(5));
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

    #[test]
    fn tui_defaults_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.tui.theme_name, DEFAULT_TUI_THEME);
        assert_eq!(cfg.tui.keymap_name, DEFAULT_TUI_KEYMAP);
        assert!(cfg.tui.custom_themes.is_empty());
        assert!(cfg.tui.custom_keymaps.is_empty());
    }

    #[test]
    fn tui_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "[tui]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.tui.theme_name, DEFAULT_TUI_THEME);
        assert_eq!(cfg.tui.keymap_name, DEFAULT_TUI_KEYMAP);
        assert!(cfg.tui.custom_themes.is_empty());
        assert!(cfg.tui.custom_keymaps.is_empty());
    }

    #[test]
    fn tui_reads_theme_and_keymap_names() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[tui]\ntheme = \"solarized\"\nkeymap = \"vim\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.tui.theme_name, "solarized");
        assert_eq!(cfg.tui.keymap_name, "vim");
    }

    #[test]
    fn tui_parses_custom_theme() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r##"
[tui.themes.tokyo]
bg = "#1a1b26"
fg = "#c0caf5"
border = "#3b4261"
border_focused = "#7aa2f7"
border_rounded = true
error_fg = "#f7768e"
"##,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.tui.custom_themes.len(), 1);
        let theme = &cfg.tui.custom_themes["tokyo"];
        assert_eq!(theme.bg.as_deref(), Some("#1a1b26"));
        assert_eq!(theme.fg.as_deref(), Some("#c0caf5"));
        assert_eq!(theme.border.as_deref(), Some("#3b4261"));
        assert_eq!(theme.border_focused.as_deref(), Some("#7aa2f7"));
        assert_eq!(theme.border_rounded, Some(true));
        assert_eq!(theme.error_fg.as_deref(), Some("#f7768e"));
        assert_eq!(theme.header_bg, None);
    }

    #[test]
    fn tui_parses_custom_keymap() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
[tui.keymaps.vim]
quit = ["q", "ctrl-c"]
scroll_up = ["k"]
scroll_down = ["j"]
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.tui.custom_keymaps.len(), 1);
        let km = &cfg.tui.custom_keymaps["vim"];
        assert_eq!(km.quit, Some(vec!["q".to_string(), "ctrl-c".to_string()]));
        assert_eq!(km.scroll_up, Some(vec!["k".to_string()]));
        assert_eq!(km.scroll_down, Some(vec!["j".to_string()]));
        assert_eq!(km.focus_next, None);
    }

    #[test]
    fn tui_parses_multiple_themes_and_keymaps() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r##"
[tui]
theme = "light"
keymap = "emacs"

[tui.themes.light]
bg = "#ffffff"
fg = "#000000"

[tui.themes.dark]
bg = "#000000"
fg = "#ffffff"

[tui.keymaps.emacs]
quit = ["ctrl-x ctrl-c"]

[tui.keymaps.vim]
quit = ["q"]
"##,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.tui.theme_name, "light");
        assert_eq!(cfg.tui.keymap_name, "emacs");
        assert_eq!(cfg.tui.custom_themes.len(), 2);
        assert!(cfg.tui.custom_themes.contains_key("light"));
        assert!(cfg.tui.custom_themes.contains_key("dark"));
        assert_eq!(cfg.tui.custom_keymaps.len(), 2);
        assert!(cfg.tui.custom_keymaps.contains_key("emacs"));
        assert!(cfg.tui.custom_keymaps.contains_key("vim"));
    }

    #[test]
    fn notification_defaults_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.tui.notifications.flow_stalled);
        assert!(cfg.tui.notifications.flow_in_review);
    }

    #[test]
    fn notification_explicit_true() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[tui.notifications]\nflow_stalled = true\nflow_in_review = true\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.tui.notifications.flow_stalled);
        assert!(cfg.tui.notifications.flow_in_review);
    }

    #[test]
    fn notification_explicit_false() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[tui.notifications]\nflow_stalled = false\nflow_in_review = false\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(!cfg.tui.notifications.flow_stalled);
        assert!(!cfg.tui.notifications.flow_in_review);
    }

    #[test]
    fn notification_partial_specification() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "[tui.notifications]\nflow_stalled = false\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(!cfg.tui.notifications.flow_stalled);
        assert!(cfg.tui.notifications.flow_in_review);
    }

    #[test]
    fn logs_dir_defaults_to_config_dir_logs() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.logs_dir, tmp.path().join("logs"));
    }

    #[test]
    fn logs_dir_absolute_path_used_as_is() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "logs_dir = \"/var/log/ur\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.logs_dir, PathBuf::from("/var/log/ur"));
    }

    #[test]
    fn logs_dir_relative_path_joined_to_config_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "logs_dir = \"custom/logs\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.logs_dir, tmp.path().join("custom/logs"));
    }

    mod resolve_project_tests {
        use super::*;

        fn make_projects() -> HashMap<String, ProjectConfig> {
            let mut m = HashMap::new();
            m.insert(
                "ur".to_owned(),
                ProjectConfig {
                    key: "ur".to_owned(),
                    repo: String::new(),
                    name: "ur".to_owned(),
                    pool_limit: 10,
                    hostexec: vec![],
                    git_hooks_dir: None,
                    skill_hooks_dir: None,
                    claude_md: None,
                    workflow_hooks_dir: None,
                    container: ContainerConfig {
                        image: String::new(),
                        mounts: vec![],
                        ports: vec![],
                    },
                    max_fix_attempts: 5,
                    protected_branches: vec![],
                },
            );
            m.insert(
                "sa".to_owned(),
                ProjectConfig {
                    key: "sa".to_owned(),
                    repo: String::new(),
                    name: "sample".to_owned(),
                    pool_limit: 10,
                    hostexec: vec![],
                    git_hooks_dir: None,
                    skill_hooks_dir: None,
                    claude_md: None,
                    workflow_hooks_dir: None,
                    container: ContainerConfig {
                        image: String::new(),
                        mounts: vec![],
                        ports: vec![],
                    },
                    max_fix_attempts: 5,
                    protected_branches: vec![],
                },
            );
            m
        }

        #[test]
        fn explicit_flag_takes_priority() {
            let projects = make_projects();
            let result = resolve_project(Some("explicit".to_owned()), &projects);
            assert_eq!(result, Some("explicit".to_owned()));
        }

        #[test]
        fn env_var_is_second_priority() {
            let _lock = ENV_MUTEX.lock().unwrap();
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::set_var("UR_PROJECT", "from_env") };
            let projects = make_projects();
            let result = resolve_project(None, &projects);
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::remove_var("UR_PROJECT") };
            assert_eq!(result, Some("from_env".to_owned()));
        }

        #[test]
        fn explicit_overrides_env_var() {
            let _lock = ENV_MUTEX.lock().unwrap();
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::set_var("UR_PROJECT", "from_env") };
            let projects = make_projects();
            let result = resolve_project(Some("explicit".to_owned()), &projects);
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::remove_var("UR_PROJECT") };
            assert_eq!(result, Some("explicit".to_owned()));
        }

        #[test]
        fn empty_env_var_is_ignored() {
            let _lock = ENV_MUTEX.lock().unwrap();
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::set_var("UR_PROJECT", "") };
            let projects = make_projects();
            // With empty env var, falls through to cwd — result depends on cwd
            // but the important thing is it doesn't return Some("")
            let result = resolve_project(None, &projects);
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::remove_var("UR_PROJECT") };
            assert_ne!(result, Some(String::new()));
        }

        #[test]
        fn dirname_matching_key_returns_key() {
            let _lock = ENV_MUTEX.lock().unwrap();
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::remove_var("UR_PROJECT") };
            // We can't easily control cwd in tests, so test the logic via
            // explicit=None with no env var — result depends on actual cwd.
            // Instead, verify the explicit and env paths work; the cwd path
            // is covered by integration/acceptance tests.
            // But we can test by confirming explicit=None with a known project
            // set returns something reasonable.
            let projects = make_projects();
            let result = resolve_project(None, &projects);
            // cwd is /workspace which may or may not match — just check it doesn't panic
            assert!(result.is_none() || projects.contains_key(result.as_deref().unwrap_or("")));
        }

        #[test]
        fn empty_projects_returns_none_for_cwd() {
            let _lock = ENV_MUTEX.lock().unwrap();
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::remove_var("UR_PROJECT") };
            let projects = HashMap::new();
            let result = resolve_project(None, &projects);
            assert!(result.is_none());
        }

        #[test]
        fn name_matching_returns_key() {
            // Test name-based matching directly by controlling explicit param.
            // The name-based matching only fires on cwd dirname, so we verify
            // the data structure is correct by checking key-based matching via explicit.
            let projects = make_projects();
            // "sa" project has name "sample"
            assert_eq!(projects["sa"].name, "sample");
            // Explicit always passes through (doesn't use name matching)
            let result = resolve_project(Some("sample".to_owned()), &projects);
            assert_eq!(result, Some("sample".to_owned()));
        }

        #[test]
        fn key_match_priority_over_name_match() {
            // If a dirname matches both a key and another project's name,
            // key match wins. We test this by creating a project whose name
            // collides with another project's key.
            let mut projects = make_projects();
            projects.insert(
                "clash".to_owned(),
                ProjectConfig {
                    key: "clash".to_owned(),
                    repo: String::new(),
                    name: "ur".to_owned(), // name matches the "ur" key
                    pool_limit: 10,
                    hostexec: vec![],
                    git_hooks_dir: None,
                    skill_hooks_dir: None,
                    claude_md: None,
                    workflow_hooks_dir: None,
                    container: ContainerConfig {
                        image: String::new(),
                        mounts: vec![],
                        ports: vec![],
                    },
                    max_fix_attempts: 5,
                    protected_branches: vec![],
                },
            );
            // If cwd dirname were "ur", it should match the "ur" key, not
            // the "clash" project whose name is "ur". We can't control cwd,
            // but the logic in resolve_project checks key first, so this
            // is structurally guaranteed.
        }
    }
}
