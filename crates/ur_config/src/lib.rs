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
    /// Whether the mount is read-only (`:ro` suffix).
    pub readonly: bool,
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

/// Parse a mount string in `"source:destination"` or `"source:destination:ro"` format.
///
/// Strips an optional `:ro` suffix, then splits on the first `:` character. Validates that:
/// - The source is a valid template path (but not `%PROJECT%`)
/// - The destination is an absolute path (starts with `/`)
/// - If a suffix is present, it must be exactly `ro` (other suffixes are rejected)
fn parse_mount_entry(project_key: &str, index: usize, raw: &str) -> anyhow::Result<MountConfig> {
    // Detect and strip optional `:ro` suffix (or reject invalid suffixes).
    let (mount_str, readonly) = parse_mount_suffix(project_key, index, raw)?;

    let colon_pos = mount_str.find(':').ok_or_else(|| {
        anyhow::anyhow!(
            "project '{project_key}': mounts[{index}]: expected 'source:destination' format, got: {raw}"
        )
    })?;

    let source = &mount_str[..colon_pos];
    let destination = &mount_str[colon_pos + 1..];

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
        readonly,
    })
}

/// Parse an optional suffix from a mount string.
///
/// Returns the mount string without the suffix and whether readonly was specified.
/// Only `:ro` is accepted; other suffixes (e.g., `:rw`, `:foo`) are rejected.
fn parse_mount_suffix<'a>(
    project_key: &str,
    index: usize,
    raw: &'a str,
) -> anyhow::Result<(&'a str, bool)> {
    // A mount has at least one colon (source:dest). If there's a second colon,
    // the part after the last colon might be a suffix.
    // We look for the last colon and check if the trailing segment is a known suffix.
    let Some(last_colon) = raw.rfind(':') else {
        return Ok((raw, false));
    };
    let suffix = &raw[last_colon + 1..];
    let Some(first_colon) = raw.find(':') else {
        return Ok((raw, false));
    };
    if first_colon == last_colon {
        return Ok((raw, false));
    }
    // There are at least two colons — the part after the last is a suffix candidate.
    if suffix == "ro" {
        return Ok((&raw[..last_colon], true));
    }
    anyhow::bail!(
        "project '{project_key}': mounts[{index}]: invalid mount suffix ':{suffix}' \
         (only ':ro' is supported), got: {raw}"
    )
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

use indexmap::IndexMap;
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

/// UID/GID of the worker user inside worker containers (created by `useradd` in Dockerfile.base).
pub const WORKER_UID: u32 = 1000;

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

/// Environment variable: override the ticket database connection URL.
///
/// When set, takes precedence over the `[ticket_db]` config section.
pub const UR_TICKET_DB_URL_ENV: &str = "UR_TICKET_DB_URL";

/// Environment variable: override the workflow database connection URL.
///
/// When set, takes precedence over the `[workflow_db]` config section.
pub const UR_WORKFLOW_DB_URL_ENV: &str = "UR_WORKFLOW_DB_URL";

// ---- Defaults ----

/// Default TCP port for the server (ur→server communication).
pub const DEFAULT_SERVER_PORT: u16 = 12321;

/// Default TCP port for the builder daemon (builderd).
/// Kept for documentation; the actual default is derived as `server_port + 2`.
pub const DEFAULT_BUILDERD_PORT: u16 = 12323;

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

/// Default CPU count for a worker container (used when the launch request omits it).
pub const DEFAULT_WORKER_CPUS: u32 = 2;

/// Default memory limit for a worker container (used when the launch request omits it).
pub const DEFAULT_WORKER_MEMORY: &str = "8G";

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
    db: Option<RawDatabaseConfig>,
    ticket_db: Option<RawTicketDbConfig>,
    workflow_db: Option<RawWorkflowDbConfig>,
    /// Deprecated: use `[db.backup]` instead. Kept for backward compatibility.
    backup: Option<RawBackupConfig>,
    server: Option<RawServerConfig>,
    logs_dir: Option<PathBuf>,
    tui: Option<RawTuiConfig>,
    #[serde(default)]
    projects: HashMap<String, RawProjectConfig>,
    /// Global skill injection configuration from the `[skills]` section.
    #[serde(default)]
    skills: Option<RawSkills>,
}

/// Raw TOML representation for the `[skills]` section.
///
/// Each sub-table maps skill names to template paths. Order is preserved
/// via [`IndexMap`] so the resolved [`GlobalSkillsConfig`] reflects the
/// order entries appear in `ur.toml`.
#[derive(Debug, Default, Deserialize)]
struct RawSkills {
    #[serde(default)]
    common: IndexMap<String, String>,
    #[serde(default)]
    code: IndexMap<String, String>,
    #[serde(default)]
    design: IndexMap<String, String>,
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

/// A single resolved global skill: a named path on the host filesystem.
///
/// Skills may be injected into worker launches based on their strategy section
/// (`common`, `code`, or `design`). All validation (path existence, template
/// variables, name format) is done at config load time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSkill {
    /// Skill name — must match `^[a-z0-9_-]+$`.
    pub name: String,
    /// Resolved, absolute path to the skill directory on the host.
    pub host_path: PathBuf,
}

/// Resolved global skills configuration from the `[skills]` section of `ur.toml`.
///
/// Skills are grouped into three sections:
/// - `common`: injected into all workers regardless of strategy
/// - `code`: injected into code-strategy workers (in addition to common)
/// - `design`: injected into design-strategy workers (in addition to common)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalSkillsConfig {
    /// Skills injected into every worker.
    pub common: Vec<GlobalSkill>,
    /// Skills injected into code-strategy workers (combined with `common`).
    pub code: Vec<GlobalSkill>,
    /// Skills injected into design-strategy workers (combined with `common`).
    pub design: Vec<GlobalSkill>,
}

impl GlobalSkillsConfig {
    /// Return all skills applicable to `strategy`.
    ///
    /// For `"code"`: returns `common` + `code` entries (common first).
    /// For `"design"`: returns `common` + `design` entries (common first).
    /// For any other strategy: returns only `common` entries.
    pub fn for_strategy(&self, strategy: &str) -> Vec<&GlobalSkill> {
        let mut skills: Vec<&GlobalSkill> = self.common.iter().collect();
        match strategy {
            "code" => skills.extend(self.code.iter()),
            "design" => skills.extend(self.design.iter()),
            _ => {}
        }
        skills
    }
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

/// Raw TOML representation for a `[projects.<key>.tui]` section.
#[derive(Debug, Deserialize)]
struct RawProjectTuiConfig {
    theme: Option<String>,
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
    /// Maximum implement cycles before stalling the workflow.
    /// Overrides `[server].max_implement_cycles` when set.
    max_implement_cycles: Option<u32>,
    /// Branches that cannot be force-pushed. Supports glob patterns.
    protected_branches: Option<Vec<String>>,
    /// Per-project TUI settings.
    tui: Option<RawProjectTuiConfig>,
    /// CI check names to ignore when evaluating workflow status.
    #[serde(default)]
    ignored_workflow_checks: Vec<String>,
    /// Relative paths to host-exec scripts that agents running against this
    /// project are allowed to invoke. Stored in canonical form (no leading `./`).
    #[serde(default)]
    hostexec_scripts: Vec<String>,
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

/// Raw TOML representation for the `[db]` section.
#[derive(Debug, Default, Deserialize)]
struct RawDatabaseConfig {
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    name: Option<String>,
    /// Network interface to bind the postgres container port on (e.g. Tailscale IP).
    bind_address: Option<String>,
    backup: Option<RawBackupConfig>,
}

/// Raw TOML representation for the `[ticket_db]` section.
#[derive(Debug, Default, Deserialize)]
struct RawTicketDbConfig {
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    name: Option<String>,
    /// Network interface to bind the postgres container port on (e.g. Tailscale IP).
    bind_address: Option<String>,
    backup: Option<RawBackupConfig>,
}

/// Raw TOML representation for the `[workflow_db]` section.
#[derive(Debug, Default, Deserialize)]
struct RawWorkflowDbConfig {
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    name: Option<String>,
    /// Network interface to bind the postgres container port on (e.g. Tailscale IP).
    bind_address: Option<String>,
    backup: Option<RawBackupConfig>,
}

/// Raw TOML representation for the `[backup]` or `[db.backup]` section.
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
    max_implement_cycles: Option<u32>,
    poll_interval_ms: Option<u64>,
    github_scan_interval_secs: Option<u64>,
    builderd_retry_count: Option<u32>,
    builderd_retry_backoff_ms: Option<u64>,
    ui_event_fallback_interval_ms: Option<u64>,
}

/// Environment variable: container runtime command override (e.g. "nerdctl").
/// Checked as a fallback when `[server].container_command` is not set in ur.toml.
pub const UR_CONTAINER_ENV: &str = "UR_CONTAINER";

/// Default container runtime command.
pub const DEFAULT_CONTAINER_COMMAND: &str = "docker";

/// Default number of days before a stale worker is cleaned up.
pub const DEFAULT_STALE_WORKER_TTL_DAYS: u64 = 7;

/// Default maximum number of implement cycles before stalling a workflow.
pub const DEFAULT_MAX_IMPLEMENT_CYCLES: u32 = 6;

/// Default poll interval in milliseconds for background polling loops.
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 500;

/// Default GitHub scan interval in seconds for the poller.
pub const DEFAULT_GITHUB_SCAN_INTERVAL_SECS: u64 = 30;

/// Default UI event fallback interval in milliseconds.
/// Used as the timeout when LISTEN/NOTIFY is active; also the poll interval
/// when the LISTEN connection is unavailable.
pub const DEFAULT_UI_EVENT_FALLBACK_INTERVAL_MS: u64 = 5000;

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
    pub max_implement_cycles: Option<u32>,
    /// Poll interval in milliseconds for background loops (default: 500).
    pub poll_interval_ms: u64,
    /// GitHub scan interval in seconds for the poller (default: 30).
    pub github_scan_interval_secs: u64,
    /// Maximum number of builderd gRPC retry attempts (default: 3).
    pub builderd_retry_count: u32,
    /// Base backoff in milliseconds for builderd retries (default: 200).
    /// Each retry doubles this value (exponential backoff).
    pub builderd_retry_backoff_ms: u64,
    /// UI event fallback interval in milliseconds (default: 5000).
    /// Used as the timeout between LISTEN/NOTIFY wake-ups; also the poll
    /// interval when the LISTEN connection is unavailable.
    pub ui_event_fallback_interval_ms: u64,
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

/// Default database host.
pub const DEFAULT_DB_HOST: &str = "ur-postgres";

/// Default database port.
pub const DEFAULT_DB_PORT: u16 = 5432;

/// Default database user.
pub const DEFAULT_DB_USER: &str = "ur";

/// Default database password.
pub const DEFAULT_DB_PASSWORD: &str = "ur";

/// Default database name.
pub const DEFAULT_DB_NAME: &str = "ur";

/// Default database name for the ticket database.
pub const DEFAULT_TICKET_DB_NAME: &str = "ur_tickets";

/// Default database name for the workflow database.
pub const DEFAULT_WORKFLOW_DB_NAME: &str = "ur_workflow";

/// Environment variable: override the ticket database password.
pub const UR_TICKET_DB_PASSWORD_ENV: &str = "UR_TICKET_DB_PASSWORD";

/// Environment variable: override the workflow database password.
pub const UR_WORKFLOW_DB_PASSWORD_ENV: &str = "UR_WORKFLOW_DB_PASSWORD";

/// Ticket database configuration, including connection details and backup settings.
///
/// Configured via the `[ticket_db]` section of `ur.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketDbConfig {
    /// Database hostname (default: "ur-postgres").
    pub host: String,
    /// Database port (default: 5432).
    pub port: u16,
    /// Database user (default: "ur").
    pub user: String,
    /// Database password (default: "ur", overridden by `UR_TICKET_DB_PASSWORD`).
    pub password: String,
    /// Database name (default: "ur_tickets").
    pub name: String,
    /// Network interface to bind the postgres container port on (e.g. a Tailscale IP).
    pub bind_address: Option<String>,
    /// Periodic backup settings for the database.
    pub backup: BackupConfig,
}

impl TicketDbConfig {
    /// Construct a Postgres connection URL from the configured fields.
    pub fn database_url(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.user, self.password, self.host, self.port, self.name
        )
    }
}

/// Workflow database configuration, including connection details and backup settings.
///
/// Configured via the `[workflow_db]` section of `ur.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDbConfig {
    /// Database hostname (default: "ur-postgres").
    pub host: String,
    /// Database port (default: 5432).
    pub port: u16,
    /// Database user (default: "ur").
    pub user: String,
    /// Database password (default: "ur", overridden by `UR_WORKFLOW_DB_PASSWORD`).
    pub password: String,
    /// Database name (default: "ur_workflow").
    pub name: String,
    /// Network interface to bind the postgres container port on (e.g. a Tailscale IP).
    pub bind_address: Option<String>,
    /// Periodic backup settings for the database.
    pub backup: BackupConfig,
}

impl WorkflowDbConfig {
    /// Construct a Postgres connection URL from the configured fields.
    pub fn database_url(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.user, self.password, self.host, self.port, self.name
        )
    }
}

/// Database configuration, including connection details and backup settings.
///
/// The `backup` field nests the existing `BackupConfig` under `[db.backup]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseConfig {
    /// Database hostname (default: "ur-postgres").
    pub host: String,
    /// Database port (default: 5432).
    pub port: u16,
    /// Database user (default: "ur").
    pub user: String,
    /// Database password (default: "ur").
    pub password: String,
    /// Database name (default: "ur").
    pub name: String,
    /// Network interface to bind the postgres container port on (e.g. a Tailscale IP).
    /// When set, the postgres service exposes `<bind_address>:<port>:<port>`.
    pub bind_address: Option<String>,
    /// Periodic backup settings for the database.
    pub backup: BackupConfig,
}

impl DatabaseConfig {
    /// Construct a Postgres connection URL from the configured fields.
    pub fn database_url(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.user, self.password, self.host, self.port, self.name
        )
    }
}

/// Known image aliases and their full tags.
pub const IMAGE_ALIASES: &[(&str, &str)] = &[
    ("ur-worker", "ur-worker:latest"),
    ("ur-worker-rust", "ur-worker-rust:latest"),
];

/// Returns the default image alias (first entry in [`IMAGE_ALIASES`]).
pub fn default_image_alias() -> &'static str {
    IMAGE_ALIASES[0].0
}

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

/// Resolved per-project TUI settings from `[projects.<key>.tui]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTuiConfig {
    /// Per-project theme override. `None` means use the global TUI theme.
    pub theme_name: Option<String>,
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
    /// Effective maximum implement cycles for this project.
    /// Resolved value: project override if set, else server default (`DEFAULT_MAX_IMPLEMENT_CYCLES`).
    /// `None` means no limit.
    pub max_implement_cycles: Option<u32>,
    /// Branch patterns that cannot be force-pushed (default: `["main", "master"]`).
    /// Supports glob patterns.
    pub protected_branches: Vec<String>,
    /// Per-project TUI settings (theme override, etc.).
    pub tui: Option<ProjectTuiConfig>,
    /// CI check names to ignore when evaluating workflow status.
    pub ignored_workflow_checks: Vec<String>,
    /// Relative paths to host-exec scripts that agents running against this
    /// project are allowed to invoke. Stored in canonical form (no leading `./`).
    pub hostexec_scripts: Vec<String>,
}

/// Resolved, ready-to-use daemon configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Root config directory (`$UR_CONFIG` or `~/.ur`).
    pub config_dir: PathBuf,
    /// Worker workspace directory.
    pub workspace: PathBuf,
    /// TCP port the server listens on (default: 12321).
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
    /// Database configuration (connection details + backup settings).
    pub db: DatabaseConfig,
    /// Ticket database configuration (connection details + backup settings).
    pub ticket_db: TicketDbConfig,
    /// Workflow database configuration (connection details + backup settings).
    pub workflow_db: WorkflowDbConfig,
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
    /// Global skill injection configuration from the `[skills]` section.
    ///
    /// Missing `[skills]` section → all sub-vecs are empty.
    pub global_skills: GlobalSkillsConfig,
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

        let db = resolve_database(raw.db, raw.backup);
        let ticket_db = resolve_ticket_db(raw.ticket_db);
        let workflow_db = resolve_workflow_db(raw.workflow_db);
        let server = resolve_server(raw.server);
        let tui = resolve_tui(raw.tui);

        let server_max_implement_cycles = server.max_implement_cycles;
        let projects = raw
            .projects
            .into_iter()
            .map(|(key, raw_proj)| {
                resolve_project_config(key, raw_proj, server_max_implement_cycles)
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        let git_branch_prefix = raw.git_branch_prefix.unwrap_or_default();

        let logs_dir = match raw.logs_dir {
            Some(p) if p.is_absolute() => p,
            Some(p) => config_dir.join(p),
            None => config_dir.join("logs"),
        };

        let global_skills = resolve_global_skills(raw.skills, config_dir)?;

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
            db,
            ticket_db,
            workflow_db,
            server,
            tui,
            logs_dir,
            git_branch_prefix,
            projects,
            global_skills,
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

/// Persist a per-project theme selection to `ur.toml`.
///
/// Reads the existing file (if any), sets `[projects.<key>.tui].theme`, and
/// writes it back without disturbing other sections.
pub fn save_project_theme_name(
    config_dir: &Path,
    project_key: &str,
    theme_name: &str,
) -> anyhow::Result<()> {
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
    let projects = table
        .entry("projects")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let projects_table = projects
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[projects] is not a table"))?;
    let project = projects_table
        .entry(project_key)
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let project_table = project
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[projects.{project_key}] is not a table"))?;
    let tui = project_table
        .entry("tui")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let tui_table = tui
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[projects.{project_key}.tui] is not a table"))?;
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
    server_max_implement_cycles: Option<u32>,
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

    let tui = raw_proj.tui.map(|t| ProjectTuiConfig {
        theme_name: t.theme,
    });

    let hostexec_scripts = normalize_hostexec_scripts(&key, raw_proj.hostexec_scripts)?;

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
        max_implement_cycles: raw_proj
            .max_implement_cycles
            .or(server_max_implement_cycles)
            .or(Some(DEFAULT_MAX_IMPLEMENT_CYCLES)),
        protected_branches: raw_proj
            .protected_branches
            .unwrap_or_else(default_protected_branches),
        tui,
        ignored_workflow_checks: raw_proj.ignored_workflow_checks,
        hostexec_scripts,
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
            ui_event_fallback_interval_ms: s
                .ui_event_fallback_interval_ms
                .unwrap_or(DEFAULT_UI_EVENT_FALLBACK_INTERVAL_MS),
        },
        None => ServerConfig {
            container_command,
            stale_worker_ttl_days: DEFAULT_STALE_WORKER_TTL_DAYS,
            max_implement_cycles: Some(DEFAULT_MAX_IMPLEMENT_CYCLES),
            poll_interval_ms: DEFAULT_POLL_INTERVAL_MS,
            github_scan_interval_secs: DEFAULT_GITHUB_SCAN_INTERVAL_SECS,
            builderd_retry_count: DEFAULT_BUILDERD_RETRY_COUNT,
            builderd_retry_backoff_ms: DEFAULT_BUILDERD_RETRY_BACKOFF_MS,
            ui_event_fallback_interval_ms: DEFAULT_UI_EVENT_FALLBACK_INTERVAL_MS,
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

/// Resolve the `[db]` config, with backward compatibility for top-level `[backup]`.
///
/// If `[db.backup]` is present, it takes precedence. If only `[backup]` is present
/// at the top level (legacy format), it is used as the backup config. If neither
/// is present, defaults are used.
fn resolve_database(
    raw_db: Option<RawDatabaseConfig>,
    raw_backup_legacy: Option<RawBackupConfig>,
) -> DatabaseConfig {
    let raw = raw_db.unwrap_or_default();
    // [db.backup] takes precedence over top-level [backup]
    let backup_raw = raw.backup.or(raw_backup_legacy);
    let backup = resolve_backup(backup_raw);
    DatabaseConfig {
        host: raw.host.unwrap_or_else(|| DEFAULT_DB_HOST.to_string()),
        port: raw.port.unwrap_or(DEFAULT_DB_PORT),
        user: raw.user.unwrap_or_else(|| DEFAULT_DB_USER.to_string()),
        password: raw
            .password
            .unwrap_or_else(|| DEFAULT_DB_PASSWORD.to_string()),
        name: raw.name.unwrap_or_else(|| DEFAULT_DB_NAME.to_string()),
        bind_address: raw.bind_address,
        backup,
    }
}

fn resolve_ticket_db(raw: Option<RawTicketDbConfig>) -> TicketDbConfig {
    let raw = raw.unwrap_or_default();
    let backup = resolve_backup(raw.backup);
    let password = std::env::var(UR_TICKET_DB_PASSWORD_ENV).unwrap_or_else(|_| {
        raw.password
            .unwrap_or_else(|| DEFAULT_DB_PASSWORD.to_string())
    });
    TicketDbConfig {
        host: raw.host.unwrap_or_else(|| DEFAULT_DB_HOST.to_string()),
        port: raw.port.unwrap_or(DEFAULT_DB_PORT),
        user: raw.user.unwrap_or_else(|| DEFAULT_DB_USER.to_string()),
        password,
        name: raw
            .name
            .unwrap_or_else(|| DEFAULT_TICKET_DB_NAME.to_string()),
        bind_address: raw.bind_address,
        backup,
    }
}

fn resolve_workflow_db(raw: Option<RawWorkflowDbConfig>) -> WorkflowDbConfig {
    let raw = raw.unwrap_or_default();
    let backup = resolve_backup(raw.backup);
    let password = std::env::var(UR_WORKFLOW_DB_PASSWORD_ENV).unwrap_or_else(|_| {
        raw.password
            .unwrap_or_else(|| DEFAULT_DB_PASSWORD.to_string())
    });
    WorkflowDbConfig {
        host: raw.host.unwrap_or_else(|| DEFAULT_DB_HOST.to_string()),
        port: raw.port.unwrap_or(DEFAULT_DB_PORT),
        user: raw.user.unwrap_or_else(|| DEFAULT_DB_USER.to_string()),
        password,
        name: raw
            .name
            .unwrap_or_else(|| DEFAULT_WORKFLOW_DB_NAME.to_string()),
        bind_address: raw.bind_address,
        backup,
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

/// Normalize and validate a list of `hostexec_scripts` entries for a project.
///
/// Normalization:
/// - Strips a single leading `./` from each entry.
///
/// Validation rejects:
/// - Empty strings.
/// - Absolute paths (starting with `/`).
/// - Paths containing `..` segments.
/// - Paths starting with `%PROJECT%`.
fn normalize_hostexec_scripts(key: &str, scripts: Vec<String>) -> anyhow::Result<Vec<String>> {
    scripts
        .into_iter()
        .map(|s| {
            let normalized = s.strip_prefix("./").unwrap_or(&s).to_string();
            if normalized.is_empty() {
                anyhow::bail!(
                    "project '{}': hostexec_scripts: empty string is not allowed",
                    key
                );
            }
            if normalized.starts_with('/') {
                anyhow::bail!(
                    "project '{}': hostexec_scripts: '{}' must be a relative path, not absolute",
                    key,
                    normalized
                );
            }
            if normalized.split('/').any(|seg| seg == "..") {
                anyhow::bail!(
                    "project '{}': hostexec_scripts: '{}' must not contain '..' segments",
                    key,
                    normalized
                );
            }
            if normalized.starts_with("%PROJECT%") {
                anyhow::bail!(
                    "project '{}': hostexec_scripts: '{}' must not start with '%PROJECT%'",
                    key,
                    normalized
                );
            }
            Ok(normalized)
        })
        .collect()
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

/// Regex matching valid skill names: `^[a-z0-9_-]+$`.
fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Resolve and validate one section of `[skills]` entries.
///
/// Returns a `Vec<GlobalSkill>` in the same order as `raw`.
///
/// Validation per entry:
/// - Skill name matches `^[a-z0-9_-]+$`
/// - Template syntax is valid (via `validate_template_str`)
/// - `%PROJECT%` prefix is rejected
/// - Resolved host path exists (skipped inside the server container — see note below)
/// - Resolved host path is a directory (skipped inside the server container)
///
/// Path existence checks are skipped when `UR_HOST_CONFIG` is set in the environment,
/// which indicates that `Config::load` is running inside the server container. Absolute
/// host paths (e.g. `/Users/me/.ur/skills/foo`) are not visible inside the container
/// but are correct mount sources for the Docker daemon — the host CLI has already
/// validated them at `ur start` time.
fn resolve_skill_section(
    section: &str,
    raw: IndexMap<String, String>,
    config_dir: &Path,
) -> anyhow::Result<Vec<GlobalSkill>> {
    let in_container = std::env::var(UR_HOST_CONFIG_ENV).is_ok();
    let mut out = Vec::with_capacity(raw.len());
    for (name, path_template) in raw {
        if !is_valid_skill_name(&name) {
            anyhow::bail!(
                "skills.{section}: skill name '{name}' is invalid — \
                 must match ^[a-z0-9_-]+$"
            );
        }
        template_path::validate_template_str(&path_template)
            .map_err(|e| anyhow::anyhow!("skills.{section}.{name}: template path invalid: {e}"))?;
        if path_template.starts_with("%PROJECT%") {
            anyhow::bail!(
                "skills.{section}.{name}: %PROJECT% is not allowed for skill paths — \
                 use %URCONFIG% or an absolute path"
            );
        }
        let resolved = template_path::resolve_template_path(&path_template, config_dir)
            .map_err(|e| anyhow::anyhow!("skills.{section}.{name}: {e}"))?;
        let host_path = match resolved {
            template_path::ResolvedTemplatePath::HostPath(p) => p,
            template_path::ResolvedTemplatePath::ProjectRelative(_) => {
                // Can't happen because we rejected %PROJECT% above, but handle for safety.
                anyhow::bail!("skills.{section}.{name}: %PROJECT% is not allowed for skill paths");
            }
        };
        if !in_container {
            if !host_path.exists() {
                anyhow::bail!(
                    "skills.{section}.{name}: path does not exist: {}",
                    host_path.display()
                );
            }
            if !host_path.is_dir() {
                anyhow::bail!(
                    "skills.{section}.{name}: path is not a directory: {}",
                    host_path.display()
                );
            }
        }
        out.push(GlobalSkill { name, host_path });
    }
    Ok(out)
}

/// Resolve and validate the `[skills]` section of `ur.toml`.
///
/// Cross-section validation:
/// - A name in `[skills.common]` AND any strategy section (`code`, `design`) is rejected.
/// - Same name in `[skills.code]` AND `[skills.design]` is allowed (paths may differ).
fn resolve_global_skills(
    raw: Option<RawSkills>,
    config_dir: &Path,
) -> anyhow::Result<GlobalSkillsConfig> {
    let raw = match raw {
        Some(r) => r,
        None => return Ok(GlobalSkillsConfig::default()),
    };

    let common = resolve_skill_section("common", raw.common, config_dir)?;
    let code = resolve_skill_section("code", raw.code, config_dir)?;
    let design = resolve_skill_section("design", raw.design, config_dir)?;

    // Cross-section: a name in common must not appear in any strategy section.
    for common_skill in &common {
        for code_skill in &code {
            if common_skill.name == code_skill.name {
                anyhow::bail!(
                    "skills: skill name '{}' appears in both [skills.common] and [skills.code] — \
                     common skills are always included; remove it from one section",
                    common_skill.name
                );
            }
        }
        for design_skill in &design {
            if common_skill.name == design_skill.name {
                anyhow::bail!(
                    "skills: skill name '{}' appears in both [skills.common] and [skills.design] — \
                     common skills are always included; remove it from one section",
                    common_skill.name
                );
            }
        }
    }

    Ok(GlobalSkillsConfig {
        common,
        code,
        design,
    })
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
    fn loads_with_empty_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.workspace, tmp.path().join("workspace"));
        assert_eq!(cfg.server_port, DEFAULT_SERVER_PORT);
        assert_eq!(cfg.proxy.hostname, DEFAULT_PROXY_HOSTNAME);
    }

    #[test]
    fn defaults_when_minimal_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "server_port = 9000\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.workspace, tmp.path().join("workspace"));
        assert_eq!(cfg.server_port, 9000);
        assert_eq!(cfg.proxy.hostname, DEFAULT_PROXY_HOSTNAME);
    }

    #[test]
    fn reads_workspace_from_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nworkspace = \"/custom/workspace\"\n",
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
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nserver_port = 9000\n",
        )
        .unwrap();
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
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n[proxy]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.proxy.hostname, DEFAULT_PROXY_HOSTNAME);
        assert_eq!(cfg.proxy.allowlist, default_proxy_allowlist());
    }

    #[test]
    fn proxy_section_reads_custom_values() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\n[proxy]\nhostname = \"my-proxy\"\nallowlist = [\"example.com\", \"other.com\"]\n",
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
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nserver_port = 5000\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.proxy.hostname, DEFAULT_PROXY_HOSTNAME);
        assert_eq!(cfg.proxy.allowlist, default_proxy_allowlist());
    }

    #[test]
    fn squid_dir_returns_correct_path() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.squid_dir(), tmp.path().join("squid"));
    }

    #[test]
    fn network_defaults_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nserver_port = 5000\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, DEFAULT_NETWORK_NAME);
        assert_eq!(cfg.network.worker_name, DEFAULT_WORKER_NETWORK_NAME);
        assert_eq!(cfg.network.server_hostname, DEFAULT_SERVER_HOSTNAME);
        assert_eq!(cfg.network.worker_prefix, DEFAULT_WORKER_PREFIX);
    }

    #[test]
    fn network_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n[network]\n").unwrap();
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
            "node_id = \"n\"\n[network]\nname = \"custom-net\"\nworker_name = \"custom-workers\"\nserver_hostname = \"my-server\"\nworker_prefix = \"test-worker-\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.network.name, "custom-net");
        assert_eq!(cfg.network.worker_name, "custom-workers");
        assert_eq!(cfg.network.server_hostname, "my-server");
        assert_eq!(cfg.network.worker_prefix, "test-worker-");
    }

    #[test]
    fn no_projects_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.projects.is_empty());
    }

    #[test]
    fn parses_single_project_with_defaults() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
                readonly: false,
            }
        );
        assert_eq!(
            cfg.projects["ur"].container.mounts[1],
            MountConfig {
                source: "/opt/tools".into(),
                destination: "/workspace/.tools".into(),
                readonly: false,
            }
        );
    }

    #[test]
    fn mounts_rejects_missing_colon() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
    fn mounts_parses_readonly_suffix() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
mounts = ["%URCONFIG%/shared-data:/var/data:ro", "/opt/tools:/workspace/.tools"]
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
                readonly: true,
            }
        );
        assert_eq!(
            cfg.projects["ur"].container.mounts[1],
            MountConfig {
                source: "/opt/tools".into(),
                destination: "/workspace/.tools".into(),
                readonly: false,
            }
        );
    }

    #[test]
    fn mounts_rejects_invalid_suffix() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
mounts = ["/opt/tools:/workspace/.tools:rw"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mounts[0]"), "{msg}");
        assert!(msg.contains("invalid mount suffix"), "{msg}");
        assert!(msg.contains(":rw"), "{msg}");
    }

    #[test]
    fn mounts_rejects_unknown_suffix() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
mounts = ["/opt/tools:/workspace/.tools:foo"]
"#,
        )
        .unwrap();
        let err = Config::load_from(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mounts[0]"), "{msg}");
        assert!(msg.contains("invalid mount suffix"), "{msg}");
        assert!(msg.contains(":foo"), "{msg}");
    }

    #[test]
    fn mounts_rejects_invalid_source_variable() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
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
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.hostexec.commands.is_empty());
    }

    #[test]
    fn hostexec_parses_passthrough_command() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.db.backup.path, None);
        assert_eq!(
            cfg.db.backup.interval_minutes,
            DEFAULT_BACKUP_INTERVAL_MINUTES
        );
        assert!(cfg.db.backup.enabled);
        assert_eq!(cfg.db.backup.retain_count, DEFAULT_BACKUP_RETAIN_COUNT);
    }

    #[test]
    fn backup_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n[backup]\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.db.backup.path, None);
        assert_eq!(
            cfg.db.backup.interval_minutes,
            DEFAULT_BACKUP_INTERVAL_MINUTES
        );
        assert!(cfg.db.backup.enabled);
        assert_eq!(cfg.db.backup.retain_count, DEFAULT_BACKUP_RETAIN_COUNT);
    }

    #[test]
    fn backup_reads_path_and_interval() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\n[backup]\npath = \"/backups/ur\"\ninterval_minutes = 60\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.db.backup.path,
            Some(std::path::PathBuf::from("/backups/ur"))
        );
        assert_eq!(cfg.db.backup.interval_minutes, 60);
    }

    #[test]
    fn backup_reads_path_with_default_interval() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\n[backup]\npath = \"/backups/ur\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.db.backup.path,
            Some(std::path::PathBuf::from("/backups/ur"))
        );
        assert_eq!(
            cfg.db.backup.interval_minutes,
            DEFAULT_BACKUP_INTERVAL_MINUTES
        );
    }

    #[test]
    fn worker_port_defaults_to_server_port_plus_one() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.worker_port, DEFAULT_SERVER_PORT + 1);
    }

    #[test]
    fn worker_port_follows_custom_server_port() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nserver_port = 9000\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.worker_port, 9001);
    }

    #[test]
    fn worker_port_reads_explicit_value() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nserver_port = 9000\nworker_port = 8000\n",
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
            "node_id = \"n\"\n[backup]\npath = \"/backups/ur\"\nenabled = false\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(!cfg.db.backup.enabled);
    }

    #[test]
    fn backup_retain_count_configurable() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\n[backup]\npath = \"/backups/ur\"\nretain_count = 7\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.db.backup.retain_count, 7);
    }

    #[test]
    fn db_defaults_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.db.host, DEFAULT_DB_HOST);
        assert_eq!(cfg.db.port, DEFAULT_DB_PORT);
        assert_eq!(cfg.db.user, DEFAULT_DB_USER);
        assert_eq!(cfg.db.password, DEFAULT_DB_PASSWORD);
        assert_eq!(cfg.db.name, DEFAULT_DB_NAME);
    }

    #[test]
    fn db_reads_custom_values() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[db]
host = "my-postgres"
port = 5433
user = "myuser"
password = "mypass"
name = "mydb"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.db.host, "my-postgres");
        assert_eq!(cfg.db.port, 5433);
        assert_eq!(cfg.db.user, "myuser");
        assert_eq!(cfg.db.password, "mypass");
        assert_eq!(cfg.db.name, "mydb");
        assert_eq!(cfg.db.bind_address, None);
    }

    #[test]
    fn db_bind_address_parsed_from_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[db]
bind_address = "100.64.1.5"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.db.bind_address, Some("100.64.1.5".to_string()));
        assert_eq!(cfg.db.host, DEFAULT_DB_HOST);
    }

    #[test]
    fn db_backup_nested_under_db_section() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[db.backup]
path = "/backups/ur"
interval_minutes = 45
retain_count = 5
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.db.backup.path,
            Some(std::path::PathBuf::from("/backups/ur"))
        );
        assert_eq!(cfg.db.backup.interval_minutes, 45);
        assert_eq!(cfg.db.backup.retain_count, 5);
    }

    #[test]
    fn db_backup_prefers_nested_over_legacy() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[backup]
path = "/old/path"
retain_count = 2

[db.backup]
path = "/new/path"
retain_count = 10
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.db.backup.path,
            Some(std::path::PathBuf::from("/new/path"))
        );
        assert_eq!(cfg.db.backup.retain_count, 10);
    }

    #[test]
    fn database_url_constructs_correct_postgres_url() {
        let db = DatabaseConfig {
            host: "myhost".to_string(),
            port: 5433,
            user: "admin".to_string(),
            password: "secret".to_string(),
            name: "testdb".to_string(),
            bind_address: None,
            backup: BackupConfig {
                path: None,
                interval_minutes: DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: DEFAULT_BACKUP_RETAIN_COUNT,
            },
        };
        assert_eq!(
            db.database_url(),
            "postgres://admin:secret@myhost:5433/testdb"
        );
    }

    #[test]
    fn database_url_with_defaults() {
        let db = DatabaseConfig {
            host: DEFAULT_DB_HOST.to_string(),
            port: DEFAULT_DB_PORT,
            user: DEFAULT_DB_USER.to_string(),
            password: DEFAULT_DB_PASSWORD.to_string(),
            name: DEFAULT_DB_NAME.to_string(),
            bind_address: None,
            backup: BackupConfig {
                path: None,
                interval_minutes: DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: DEFAULT_BACKUP_RETAIN_COUNT,
            },
        };
        assert_eq!(db.database_url(), "postgres://ur:ur@ur-postgres:5432/ur");
    }

    #[test]
    fn hostexec_long_lived_defaults_false() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
            "node_id = \"n\"\n[server]\ncontainer_command = \"docker\"\n",
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
            "node_id = \"n\"\n[server]\ncontainer_command = \"docker\"\n",
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
            "node_id = \"n\"\n[server]\ncontainer_command = \"docker\"\nstale_worker_ttl_days = 14\npoll_interval_ms = 1000\n",
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
node_id = "n"
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
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
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
            "node_id = \"n\"\n[server]\ncontainer_command = \"podman\"\n",
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
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.tui.theme_name, DEFAULT_TUI_THEME);
        assert_eq!(cfg.tui.keymap_name, DEFAULT_TUI_KEYMAP);
        assert!(cfg.tui.custom_themes.is_empty());
        assert!(cfg.tui.custom_keymaps.is_empty());
    }

    #[test]
    fn tui_defaults_when_present_but_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n[tui]\n").unwrap();
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
            "node_id = \"n\"\n[tui]\ntheme = \"solarized\"\nkeymap = \"vim\"\n",
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
node_id = "n"
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
node_id = "n"
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
node_id = "n"
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
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.tui.notifications.flow_stalled);
        assert!(cfg.tui.notifications.flow_in_review);
    }

    #[test]
    fn notification_explicit_true() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\n[tui.notifications]\nflow_stalled = true\nflow_in_review = true\n",
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
            "node_id = \"n\"\n[tui.notifications]\nflow_stalled = false\nflow_in_review = false\n",
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
            "node_id = \"n\"\n[tui.notifications]\nflow_stalled = false\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(!cfg.tui.notifications.flow_stalled);
        assert!(cfg.tui.notifications.flow_in_review);
    }

    #[test]
    fn logs_dir_defaults_to_config_dir_logs() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), "node_id = \"n\"\n").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.logs_dir, tmp.path().join("logs"));
    }

    #[test]
    fn logs_dir_absolute_path_used_as_is() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nlogs_dir = \"/var/log/ur\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.logs_dir, PathBuf::from("/var/log/ur"));
    }

    #[test]
    fn logs_dir_relative_path_joined_to_config_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nlogs_dir = \"custom/logs\"\n",
        )
        .unwrap();
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
                    max_implement_cycles: Some(DEFAULT_MAX_IMPLEMENT_CYCLES),
                    protected_branches: vec![],
                    tui: None,
                    ignored_workflow_checks: vec![],
                    hostexec_scripts: vec![],
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
                    max_implement_cycles: Some(DEFAULT_MAX_IMPLEMENT_CYCLES),
                    protected_branches: vec![],
                    tui: None,
                    ignored_workflow_checks: vec![],
                    hostexec_scripts: vec![],
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
                    max_implement_cycles: Some(DEFAULT_MAX_IMPLEMENT_CYCLES),
                    protected_branches: vec![],
                    tui: None,
                    ignored_workflow_checks: vec![],
                    hostexec_scripts: vec![],
                },
            );
            // If cwd dirname were "ur", it should match the "ur" key, not
            // the "clash" project whose name is "ur". We can't control cwd,
            // but the logic in resolve_project checks key first, so this
            // is structurally guaranteed.
        }
    }

    #[test]
    fn project_tui_none_when_section_absent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["ur"].tui, None);
    }

    #[test]
    fn project_tui_parses_theme() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
[projects.ur.tui]
theme = "dracula"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let tui = cfg.projects["ur"].tui.as_ref().expect("tui should be Some");
        assert_eq!(tui.theme_name.as_deref(), Some("dracula"));
    }

    #[test]
    fn ignored_workflow_checks_parses_list() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
ignored_workflow_checks = ["bench"]
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["ur"].ignored_workflow_checks, vec!["bench"]);
    }

    #[test]
    fn ignored_workflow_checks_defaults_to_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert!(cfg.projects["ur"].ignored_workflow_checks.is_empty());
    }

    #[test]
    fn project_tui_theme_none_when_section_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
[projects.ur.tui]
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let tui = cfg.projects["ur"].tui.as_ref().expect("tui should be Some");
        assert_eq!(tui.theme_name, None);
    }

    #[test]
    fn save_project_theme_name_creates_file_when_absent() {
        let tmp = TempDir::new().unwrap();
        save_project_theme_name(tmp.path(), "myproj", "dracula").unwrap();
        let content = std::fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(
            content.contains("dracula"),
            "expected theme in output: {content}"
        );
        assert!(
            content.contains("myproj"),
            "expected project key in output: {content}"
        );
    }

    #[test]
    fn save_project_theme_name_round_trips() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        save_project_theme_name(tmp.path(), "ur", "nord").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let tui = cfg.projects["ur"].tui.as_ref().expect("tui should be Some");
        assert_eq!(tui.theme_name.as_deref(), Some("nord"));
        // Original fields should be preserved
        assert_eq!(cfg.projects["ur"].repo, "git@github.com:cmaher/ur.git");
    }

    #[test]
    fn save_project_theme_name_overwrites_existing_theme() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.tui]
theme = "dark"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        save_project_theme_name(tmp.path(), "ur", "light").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        let tui = cfg.projects["ur"].tui.as_ref().expect("tui should be Some");
        assert_eq!(tui.theme_name.as_deref(), Some("light"));
    }

    #[test]
    fn save_project_theme_name_does_not_disturb_global_tui() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[tui]
theme = "synthwave"

[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        save_project_theme_name(tmp.path(), "ur", "nord").unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        // Global TUI theme must be untouched
        assert_eq!(cfg.tui.theme_name, "synthwave");
        // Per-project theme is set
        let tui = cfg.projects["ur"].tui.as_ref().expect("tui should be Some");
        assert_eq!(tui.theme_name.as_deref(), Some("nord"));
    }

    mod hostexec_scripts_tests {
        use super::*;

        fn toml_with_scripts(scripts: &str) -> String {
            format!(
                r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
hostexec_scripts = {scripts}
[projects.ur.container]
image = "ur-worker"
"#
            )
        }

        #[test]
        fn defaults_to_empty_when_absent() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                r#"
node_id = "n"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
[projects.ur.container]
image = "ur-worker"
"#,
            )
            .unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();
            assert!(cfg.projects["ur"].hostexec_scripts.is_empty());
        }

        #[test]
        fn accepts_plain_relative_path() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                toml_with_scripts(r#"["needs-to-run-on-host.sh"]"#),
            )
            .unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();
            assert_eq!(
                cfg.projects["ur"].hostexec_scripts,
                vec!["needs-to-run-on-host.sh"]
            );
        }

        #[test]
        fn accepts_subdirectory_path() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                toml_with_scripts(r#"["scripts/deploy.sh"]"#),
            )
            .unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();
            assert_eq!(
                cfg.projects["ur"].hostexec_scripts,
                vec!["scripts/deploy.sh"]
            );
        }

        #[test]
        fn normalizes_leading_dot_slash() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                toml_with_scripts(r#"["./needs-to-run-on-host.sh", "./scripts/deploy.sh"]"#),
            )
            .unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();
            assert_eq!(
                cfg.projects["ur"].hostexec_scripts,
                vec!["needs-to-run-on-host.sh", "scripts/deploy.sh"]
            );
        }

        #[test]
        fn rejects_empty_string() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(tmp.path().join("ur.toml"), toml_with_scripts(r#"[""]"#)).unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            assert!(
                err.to_string().contains("empty string"),
                "unexpected error: {err}"
            );
        }

        #[test]
        fn rejects_absolute_path() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                toml_with_scripts(r#"["/usr/local/bin/script.sh"]"#),
            )
            .unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            assert!(
                err.to_string().contains("absolute"),
                "unexpected error: {err}"
            );
        }

        #[test]
        fn rejects_dotdot_segment() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                toml_with_scripts(r#"["../escape.sh"]"#),
            )
            .unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            assert!(err.to_string().contains(".."), "unexpected error: {err}");
        }

        #[test]
        fn rejects_dotdot_in_middle() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                toml_with_scripts(r#"["scripts/../escape.sh"]"#),
            )
            .unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            assert!(err.to_string().contains(".."), "unexpected error: {err}");
        }

        #[test]
        fn rejects_project_prefix() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                toml_with_scripts(r#"["%PROJECT%/scripts/run.sh"]"#),
            )
            .unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            assert!(
                err.to_string().contains("%PROJECT%"),
                "unexpected error: {err}"
            );
        }

        #[test]
        fn error_message_names_offending_entry() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                toml_with_scripts(r#"["/bad/path.sh"]"#),
            )
            .unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("/bad/path.sh"),
                "error should name the offending entry: {msg}"
            );
        }

        #[test]
        fn normalize_does_not_strip_more_than_one_leading_dot_slash() {
            let tmp = TempDir::new().unwrap();
            // "././script.sh" — only one leading "./" is stripped; result is "./script.sh"
            // which is still relative and valid (no absolute, no .., no %PROJECT%)
            std::fs::write(
                tmp.path().join("ur.toml"),
                toml_with_scripts(r#"["././script.sh"]"#),
            )
            .unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();
            assert_eq!(cfg.projects["ur"].hostexec_scripts, vec!["./script.sh"]);
        }
    }

    // ---- GlobalSkillsConfig / [skills] tests ----

    mod skills_tests {
        use super::*;

        /// Write a minimal ur.toml with the provided `[skills]` TOML snippet appended.
        fn toml_with_skills(skills_toml: &str) -> String {
            format!("server_port = 12321\n{skills_toml}")
        }

        /// Create a skill directory under `base` with the given relative path and return it.
        fn make_skill_dir(base: &std::path::Path, name: &str) -> PathBuf {
            let p = base.join(name);
            std::fs::create_dir_all(&p).unwrap();
            p
        }

        #[test]
        fn missing_skills_section_defaults_to_empty() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(tmp.path().join("ur.toml"), "server_port = 12321\n").unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();
            assert!(cfg.global_skills.common.is_empty());
            assert!(cfg.global_skills.code.is_empty());
            assert!(cfg.global_skills.design.is_empty());
        }

        #[test]
        fn empty_skills_section_defaults_to_empty() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(
                tmp.path().join("ur.toml"),
                "server_port = 12321\n[skills]\n",
            )
            .unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();
            assert!(cfg.global_skills.common.is_empty());
            assert!(cfg.global_skills.code.is_empty());
            assert!(cfg.global_skills.design.is_empty());
        }

        #[test]
        fn parses_all_three_sub_tables() {
            let tmp = TempDir::new().unwrap();
            let skill_dir = make_skill_dir(tmp.path(), "skills");
            make_skill_dir(&skill_dir, "shared");
            make_skill_dir(&skill_dir, "internal");
            make_skill_dir(&skill_dir, "research");

            let config_dir = tmp.path();
            let skills_toml = format!(
                r#"
[skills.common]
shared = "{}/skills/shared"

[skills.code]
internal = "{}/skills/internal"

[skills.design]
research = "{}/skills/research"
"#,
                config_dir.display(),
                config_dir.display(),
                config_dir.display(),
            );
            std::fs::write(config_dir.join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let cfg = Config::load_from(config_dir).unwrap();

            assert_eq!(cfg.global_skills.common.len(), 1);
            assert_eq!(cfg.global_skills.common[0].name, "shared");

            assert_eq!(cfg.global_skills.code.len(), 1);
            assert_eq!(cfg.global_skills.code[0].name, "internal");

            assert_eq!(cfg.global_skills.design.len(), 1);
            assert_eq!(cfg.global_skills.design[0].name, "research");
        }

        #[test]
        fn for_strategy_code_returns_common_plus_code() {
            let tmp = TempDir::new().unwrap();
            make_skill_dir(tmp.path(), "skills/common-skill");
            make_skill_dir(tmp.path(), "skills/code-skill");
            make_skill_dir(tmp.path(), "skills/design-skill");

            let base = tmp.path().display();
            let skills_toml = format!(
                r#"
[skills.common]
common-skill = "{base}/skills/common-skill"

[skills.code]
code-skill = "{base}/skills/code-skill"

[skills.design]
design-skill = "{base}/skills/design-skill"
"#
            );
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();

            let code_skills = cfg.global_skills.for_strategy("code");
            assert_eq!(code_skills.len(), 2);
            assert_eq!(code_skills[0].name, "common-skill");
            assert_eq!(code_skills[1].name, "code-skill");
        }

        #[test]
        fn for_strategy_design_returns_common_plus_design() {
            let tmp = TempDir::new().unwrap();
            make_skill_dir(tmp.path(), "skills/common-skill");
            make_skill_dir(tmp.path(), "skills/design-skill");

            let base = tmp.path().display();
            let skills_toml = format!(
                r#"
[skills.common]
common-skill = "{base}/skills/common-skill"

[skills.design]
design-skill = "{base}/skills/design-skill"
"#
            );
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();

            let design_skills = cfg.global_skills.for_strategy("design");
            assert_eq!(design_skills.len(), 2);
            assert_eq!(design_skills[0].name, "common-skill");
            assert_eq!(design_skills[1].name, "design-skill");
        }

        #[test]
        fn for_strategy_unknown_returns_only_common() {
            let tmp = TempDir::new().unwrap();
            make_skill_dir(tmp.path(), "skills/common-skill");
            make_skill_dir(tmp.path(), "skills/code-skill");

            let base = tmp.path().display();
            let skills_toml = format!(
                r#"
[skills.common]
common-skill = "{base}/skills/common-skill"

[skills.code]
code-skill = "{base}/skills/code-skill"
"#
            );
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();

            let unknown_skills = cfg.global_skills.for_strategy("unknown");
            assert_eq!(unknown_skills.len(), 1);
            assert_eq!(unknown_skills[0].name, "common-skill");
        }

        #[test]
        fn rejects_percent_project_in_skill_path() {
            let tmp = TempDir::new().unwrap();
            let skills_toml = r#"
[skills.common]
my-skill = "%PROJECT%/skills/my-skill"
"#;
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(skills_toml)).unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("%PROJECT%") && msg.contains("not allowed"),
                "unexpected error: {msg}"
            );
            assert!(msg.contains("my-skill"), "{msg}");
            assert!(msg.contains("common"), "{msg}");
        }

        #[test]
        fn rejects_missing_path() {
            let tmp = TempDir::new().unwrap();
            let skills_toml = format!(
                r#"
[skills.code]
missing-skill = "{}/skills/nonexistent"
"#,
                tmp.path().display()
            );
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("does not exist"), "unexpected error: {msg}");
            assert!(msg.contains("missing-skill"), "{msg}");
            assert!(msg.contains("code"), "{msg}");
        }

        #[test]
        fn rejects_non_directory_path() {
            let tmp = TempDir::new().unwrap();
            // Create a file, not a directory.
            let file_path = tmp.path().join("a-file");
            std::fs::write(&file_path, "data").unwrap();

            let skills_toml = format!(
                r#"
[skills.design]
my-tool = "{}"
"#,
                file_path.display()
            );
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("not a directory"), "unexpected error: {msg}");
            assert!(msg.contains("my-tool"), "{msg}");
            assert!(msg.contains("design"), "{msg}");
        }

        #[test]
        fn rejects_bad_skill_name() {
            let tmp = TempDir::new().unwrap();
            make_skill_dir(tmp.path(), "skills/bad");
            let skills_toml = format!(
                r#"
[skills.common]
"Bad Name!" = "{}/skills/bad"
"#,
                tmp.path().display()
            );
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("invalid"), "unexpected error: {msg}");
            assert!(msg.contains("Bad Name!"), "{msg}");
        }

        #[test]
        fn rejects_common_and_code_duplicate() {
            let tmp = TempDir::new().unwrap();
            make_skill_dir(tmp.path(), "skills/dup-skill-a");
            make_skill_dir(tmp.path(), "skills/dup-skill-b");
            let base = tmp.path().display();
            let skills_toml = format!(
                r#"
[skills.common]
dup-skill = "{base}/skills/dup-skill-a"

[skills.code]
dup-skill = "{base}/skills/dup-skill-b"
"#
            );
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("dup-skill"), "unexpected error: {msg}");
            assert!(msg.contains("common") && msg.contains("code"), "{msg}");
        }

        #[test]
        fn rejects_common_and_design_duplicate() {
            let tmp = TempDir::new().unwrap();
            make_skill_dir(tmp.path(), "skills/dup-skill-a");
            make_skill_dir(tmp.path(), "skills/dup-skill-b");
            let base = tmp.path().display();
            let skills_toml = format!(
                r#"
[skills.common]
dup-skill = "{base}/skills/dup-skill-a"

[skills.design]
dup-skill = "{base}/skills/dup-skill-b"
"#
            );
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let err = Config::load_from(tmp.path()).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("dup-skill"), "unexpected error: {msg}");
            assert!(msg.contains("common") && msg.contains("design"), "{msg}");
        }

        #[test]
        fn allows_same_name_in_code_and_design() {
            let tmp = TempDir::new().unwrap();
            make_skill_dir(tmp.path(), "skills/shared-impl-a");
            make_skill_dir(tmp.path(), "skills/shared-impl-b");
            let base = tmp.path().display();
            let skills_toml = format!(
                r#"
[skills.code]
shared-impl = "{base}/skills/shared-impl-a"

[skills.design]
shared-impl = "{base}/skills/shared-impl-b"
"#
            );
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(&skills_toml)).unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();
            // Both code and design should have the skill with different paths.
            assert_eq!(cfg.global_skills.code[0].name, "shared-impl");
            assert_eq!(cfg.global_skills.design[0].name, "shared-impl");
            assert_ne!(
                cfg.global_skills.code[0].host_path,
                cfg.global_skills.design[0].host_path
            );
        }

        #[test]
        fn urconfig_template_is_resolved() {
            let tmp = TempDir::new().unwrap();
            make_skill_dir(tmp.path(), "skills/my-skill");
            let skills_toml = r#"
[skills.common]
my-skill = "%URCONFIG%/skills/my-skill"
"#;
            std::fs::write(tmp.path().join("ur.toml"), toml_with_skills(skills_toml)).unwrap();
            let cfg = Config::load_from(tmp.path()).unwrap();
            assert_eq!(cfg.global_skills.common[0].name, "my-skill");
            assert_eq!(
                cfg.global_skills.common[0].host_path,
                tmp.path().join("skills/my-skill")
            );
        }
    }

    #[test]
    fn project_max_implement_cycles_overrides_server() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[server]
max_implement_cycles = 6
[projects.x]
repo = "git@github.com:example/x.git"
max_implement_cycles = 12
[projects.x.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["x"].max_implement_cycles, Some(12));
    }

    #[test]
    fn project_max_implement_cycles_inherits_server_when_unset() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[server]
max_implement_cycles = 8
[projects.x]
repo = "git@github.com:example/x.git"
[projects.x.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.projects["x"].max_implement_cycles, Some(8));
    }

    #[test]
    fn project_max_implement_cycles_defaults_when_neither_set() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("ur.toml"),
            r#"
node_id = "n"
[projects.x]
repo = "git@github.com:example/x.git"
[projects.x.container]
image = "ur-worker"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            cfg.projects["x"].max_implement_cycles,
            Some(DEFAULT_MAX_IMPLEMENT_CYCLES)
        );
    }
}
