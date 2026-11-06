use std::collections::HashMap;
use std::path::PathBuf;

use chrono::Utc;
use rand::Rng;
use serde::Deserialize;
use tracing::info;
use uuid::Uuid;

use ur_config::{GlobalSkillsConfig, NetworkConfig};
use workflow_db::WorkerRepo;

use crate::network_manager::NetworkManager;

use ur_rpc::proto::builder_container::{LaunchWorkerRequest, StopWorkerRequest};

use crate::builder_container_client::BuilderContainerClient;
use crate::builder_local_worker_client::BuilderLocalWorkerClient;
use crate::run_opts_builder::RunOptsBuilder;
use crate::strategy::WorkerStrategy;

/// Unique identifier for a running worker, format: `{process_id}-{4 random [a-z0-9]}`.
///
/// The random suffix prevents collisions when the same process_id is reused
/// across launches (e.g. after a stop/start cycle).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkerId(pub String);

impl std::fmt::Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl WorkerId {
    /// Generate a new worker ID from a process_id by appending `-{4 random [a-z0-9]}`.
    pub fn generate(process_id: &str) -> Self {
        let mut rng = rand::rng();
        let suffix: String = (0..4)
            .map(|_| {
                let idx = rng.random_range(0..36u8);
                if idx < 10 {
                    (b'0' + idx) as char
                } else {
                    (b'a' + idx - 10) as char
                }
            })
            .collect();
        Self(format!("{process_id}-{suffix}"))
    }

    /// Validate that a string matches the expected worker ID format:
    /// non-empty prefix, a dash, then exactly 4 alphanumeric lowercase chars.
    pub fn parse(s: &str) -> Result<Self, String> {
        let Some(dash_pos) = s.rfind('-') else {
            return Err(format!("invalid worker ID (no dash): {s}"));
        };
        if dash_pos == 0 {
            return Err(format!("invalid worker ID (empty process_id): {s}"));
        }
        let suffix = &s[dash_pos + 1..];
        if suffix.len() != 4
            || !suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Err(format!(
                "invalid worker ID suffix (expected 4 [a-z0-9] chars): {s}"
            ));
        }
        Ok(Self(s.to_string()))
    }
}

/// Returns the hardcoded default worker modes derived from WorkerStrategy variants.
fn default_worker_modes() -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    map.insert("code".into(), WorkerStrategy::Code.skills());
    map.insert("design".into(), WorkerStrategy::Design.skills());
    map.insert("manual".into(), WorkerStrategy::Manual.skills());
    map
}

/// Raw TOML representation for the `[worker_modes]` section.
/// Each key is a mode name mapping to a table with a `skills` list.
#[derive(Debug, Default, Deserialize)]
struct RawWorkerModes {
    #[serde(flatten)]
    modes: HashMap<String, RawModeEntry>,
}

/// A single worker mode entry with its base strategy, skills list, and optional
/// model override.
#[derive(Debug, Deserialize)]
struct RawModeEntry {
    /// The base worker strategy name (e.g. "code" or "design"). Required for custom modes.
    base: String,
    skills: Vec<String>,
    /// Optional Claude Code model alias override. When omitted, the mode
    /// inherits the effective default (a `[worker_models]` override, if any,
    /// else `agent.default_model(strategy)`) from its base strategy.
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    /// Optional agent override for this mode (e.g. "claude"). Defaults to
    /// claude when omitted.
    #[serde(default)]
    agent: Option<String>,
    /// Optional runtime override for this mode ("container" or "local").
    /// Kept raw so the deserialization error can name `worker_modes.<mode>.runtime`.
    #[serde(default)]
    runtime: Option<toml::Value>,
}

/// Raw TOML representation for a strategy-keyed model table: either the flat
/// `[worker_models]` table (applies to any agent) or one of its per-agent
/// sub-tables (`[worker_models.<agent>]`, wins for that agent only).
/// `deny_unknown_fields` rejects typos/unknown strategy names with an error
/// naming the bad key, since only "code", "design", and "manual" are valid.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStrategyDefaults {
    code: Option<StrategyEntry>,
    design: Option<StrategyEntry>,
    manual: Option<StrategyEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyEntry {
    model: Option<String>,
    effort: Option<String>,
}

impl RawStrategyDefaults {
    fn get(&self, strategy: WorkerStrategy) -> Option<&StrategyEntry> {
        match strategy {
            WorkerStrategy::Code => self.code.as_ref(),
            WorkerStrategy::Design => self.design.as_ref(),
            WorkerStrategy::Manual => self.manual.as_ref(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct RawWorkerModels {
    per_agent: HashMap<ur_config::AgentType, RawStrategyDefaults>,
}

/// Error message for an unrecognized `[worker_models.<agent>]` table name.
fn unknown_worker_models_agent_error(key: &str) -> String {
    let valid: Vec<&str> = ur_config::AgentType::ALL.iter().map(|a| a.name()).collect();
    format!(
        "invalid worker_models config: unknown agent '{key}' in \
         [worker_models.{key}]. Valid agents: {valid:?}"
    )
}

fn validate_effort(agent: ur_config::AgentType, effort: &str, context: &str) -> Result<(), String> {
    if agent.supported_efforts().contains(&effort) {
        return Ok(());
    }
    Err(format!(
        "invalid {context}: agent '{}' does not support effort '{effort}'. Valid efforts: {:?}",
        agent.name(),
        agent.supported_efforts()
    ))
}

fn parse_worker_models(section: Option<&toml::Value>) -> Result<RawWorkerModels, String> {
    let Some(section) = section else {
        return Ok(RawWorkerModels::default());
    };
    {
        let table = section
            .as_table()
            .ok_or_else(|| "invalid worker_models config: expected a table".to_string())?;
        for (key, val) in table {
            if !val.is_table() {
                return Err(format!(
                    "invalid worker_models config: '{key}' must be an agent table; use [worker_models.<agent>] with inline {{ model, effort }} strategy entries"
                ));
            }
        }
    }

    let raw: HashMap<String, RawStrategyDefaults> = section
        .clone()
        .try_into()
        .map_err(|e| format!("invalid worker_models config: {e}"))?;
    let mut per_agent = HashMap::new();
    for (key, defaults) in raw {
        let agent = ur_config::AgentType::parse(&key)
            .map_err(|_| unknown_worker_models_agent_error(&key))?;
        for entry in [&defaults.code, &defaults.design, &defaults.manual]
            .into_iter()
            .flatten()
        {
            if let Some(effort) = &entry.effort {
                validate_effort(agent, effort, "worker_models config")?;
            }
        }
        per_agent.insert(agent, defaults);
    }
    Ok(RawWorkerModels { per_agent })
}

/// Resolve the effective default model for a strategy, most specific first:
/// `[worker_models.<agent>]` override, then `agent.default_model(strategy)`.
///
/// An empty string means "no model" and is a valid outcome, not a failure: it
/// is what an agent with no model concept returns, and `build_worker_env_vars`
/// then omits `UR_WORKER_MODEL` so `spawn_command(None)` launches the agent
/// bare. Deliberately not a panic — `every_strategy_has_a_default_model` pins
/// the invariant that every agent covers every `WorkerStrategy`, so a missing
/// table entry fails in CI rather than at launch.
fn effective_default_model(
    agent: ur_config::AgentType,
    strategy: WorkerStrategy,
    overrides: &RawWorkerModels,
) -> String {
    overrides
        .per_agent
        .get(&agent)
        .and_then(|m| m.get(strategy))
        .and_then(|entry| entry.model.as_deref())
        .or_else(|| agent.default_model(strategy.name()))
        .unwrap_or_default()
        .to_owned()
}

fn effective_default_effort(
    agent: ur_config::AgentType,
    strategy: WorkerStrategy,
    overrides: &RawWorkerModels,
) -> String {
    overrides
        .per_agent
        .get(&agent)
        .and_then(|m| m.get(strategy))
        .and_then(|entry| entry.effort.as_deref())
        .unwrap_or_else(|| agent.default_effort())
        .to_owned()
}

/// Seed the built-in strategies map, one entry per [`WorkerStrategy`].
///
/// Built-in modes have no explicit model of their own — their model is always
/// computed from whichever agent the launch actually resolves to (see
/// `resolve_mode`), not baked in here, since an explicit `--agent` override
/// can pick a different agent than the one this function is seeding for.
fn seed_strategies() -> HashMap<String, WorkerStrategy> {
    WorkerStrategy::ALL
        .iter()
        .copied()
        .map(|strategy| (strategy.name().to_owned(), strategy))
        .collect()
}

/// Parse the top-level `runtime` key via serde; omitted → container. The error
/// names the key so a typo in `ur.toml` points at the right line.
fn parse_top_level_runtime(raw: Option<&toml::Value>) -> Result<ur_config::WorkerRuntime, String> {
    raw.map(|v| v.clone().try_into::<ur_config::WorkerRuntime>())
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(|e| format!("runtime: {e}"))
}

/// Seed the built-in agents map, one entry per [`WorkerStrategy`] — all
/// `default_agent`, since built-in modes have no `agent` field of their own.
fn default_agents(default_agent: ur_config::AgentType) -> HashMap<String, ur_config::AgentType> {
    WorkerStrategy::ALL
        .iter()
        .map(|strategy| (strategy.name().to_owned(), default_agent))
        .collect()
}

/// Resolve a custom mode's agent field, defaulting to `default_agent` when omitted.
/// Errors name the offending mode, matching the `base` validation convention.
fn resolve_entry_agent(
    mode_name: &str,
    raw_agent: Option<&str>,
    default_agent: ur_config::AgentType,
) -> Result<ur_config::AgentType, String> {
    match raw_agent {
        Some(name) => ur_config::AgentType::parse(name)
            .map_err(|e| format!("invalid agent '{name}' for worker mode '{mode_name}': {e}")),
        None => Ok(default_agent),
    }
}

/// Resolved worker modes configuration mapping mode names to skill lists, strategies,
/// and Claude Code model aliases.
#[derive(Debug, Clone)]
pub struct WorkerModesConfig {
    modes: HashMap<String, Vec<String>>,
    /// Maps mode names to their worker strategy. Built-in modes ("code", "design")
    /// map to their corresponding variants; custom modes map via their `base` field.
    strategies: HashMap<String, WorkerStrategy>,
    /// Explicit model override per mode, from a custom mode's
    /// `worker_modes.<name>.model`. Built-in modes never have an entry here.
    ///
    /// Deliberately *not* a precomputed "resolved model per mode": which
    /// model applies depends on the agent actually running the launch, and an
    /// explicit `--agent`/launch-request override (`resolve_mode`'s
    /// `requested_agent`) can pick a different agent than whichever one this
    /// mode was configured or defaulted to. `resolve_mode` computes the
    /// effective model from `model_overrides` and the *final* resolved agent,
    /// falling back to this map only for a mode's own explicit pin.
    explicit_models: HashMap<String, String>,
    explicit_efforts: HashMap<String, String>,
    /// Maps mode names to their agent. Built-in modes default to `default_agent`;
    /// custom modes may override via `worker_modes.<name>.agent`.
    agents: HashMap<String, ur_config::AgentType>,
    /// The top-level `agent` default (from `ur.toml`'s `agent` key, or claude
    /// when omitted). `resolve_mode`'s fallback uses this instead of a
    /// hardcoded `AgentType::Claude` — it is the floor beneath a mode's own
    /// `agent` field and an explicit `--agent`/launch-request override.
    default_agent: ur_config::AgentType,
    /// Explicit `worker_modes.<name>.runtime` per mode; absent → project/top-level.
    mode_runtimes: HashMap<String, ur_config::WorkerRuntime>,
    /// The top-level `runtime` default (container when omitted).
    default_runtime: ur_config::WorkerRuntime,
    /// Parsed `[worker_models]` section (flat + per-agent tables), kept so
    /// `resolve_mode` can compute a mode's effective model against whichever
    /// agent the launch actually resolves to.
    model_overrides: RawWorkerModels,
}

impl Default for WorkerModesConfig {
    fn default() -> Self {
        let default_agent = ur_config::AgentType::Claude;
        Self {
            modes: default_worker_modes(),
            strategies: seed_strategies(),
            explicit_models: HashMap::new(),
            explicit_efforts: HashMap::new(),
            agents: default_agents(default_agent),
            default_agent,
            mode_runtimes: HashMap::new(),
            default_runtime: ur_config::WorkerRuntime::default(),
            model_overrides: RawWorkerModels::default(),
        }
    }
}

impl WorkerModesConfig {
    /// Parse worker_modes from a TOML string.
    /// If no `[worker_modes]` section exists, hardcoded defaults are used.
    /// Any modes defined in the config replace the defaults entirely.
    /// Custom modes must specify a valid `base` field ("code" or "design").
    ///
    /// An optional `[worker_models]` section overrides the built-in default
    /// model per strategy (code/design/manual); an optional
    /// `[worker_models.<agent>]` sub-table overrides it further for that agent
    /// only, so a model alias for one agent can't leak into a launch of
    /// another. Omitted keys fall back to `agent.default_model(strategy)`;
    /// unknown strategy keys and unknown agent table names are both config
    /// errors. Resolution precedence, most specific first: custom mode
    /// explicit `model` > `[worker_models.<agent>]` > `[worker_models]` >
    /// `agent.default_model(strategy)`.
    pub fn from_toml(toml_content: &str) -> Result<Self, String> {
        // Parse the full TOML to extract just the worker_modes section
        let value: toml::Value =
            toml::from_str(toml_content).map_err(|e| format!("invalid TOML: {e}"))?;

        let default_agent =
            ur_config::resolve_top_level_agent(value.get("agent").and_then(toml::Value::as_str))
                .map_err(|e| e.to_string())?;
        let default_runtime = parse_top_level_runtime(value.get("runtime"))?;

        let model_overrides = parse_worker_models(value.get("worker_models"))?;

        let Some(section) = value.get("worker_modes") else {
            return Ok(Self {
                modes: default_worker_modes(),
                strategies: seed_strategies(),
                explicit_models: HashMap::new(),
                explicit_efforts: HashMap::new(),
                agents: default_agents(default_agent),
                default_agent,
                mode_runtimes: HashMap::new(),
                default_runtime,
                model_overrides,
            });
        };

        let raw: RawWorkerModes = section
            .clone()
            .try_into()
            .map_err(|e| format!("invalid worker_modes config: {e}"))?;
        let mut modes = default_worker_modes();
        let mut strategies = seed_strategies();
        let mut explicit_models = HashMap::new();
        let mut explicit_efforts = HashMap::new();
        let mut agents = default_agents(default_agent);
        let mut mode_runtimes = HashMap::new();
        for (name, entry) in raw.modes {
            let strategy = WorkerStrategy::from_name(&entry.base).map_err(|_| {
                format!(
                    "invalid base '{}' for worker mode '{}': must be 'code', 'design', or 'manual'",
                    entry.base, name
                )
            })?;
            let agent = resolve_entry_agent(&name, entry.agent.as_deref(), default_agent)?;
            if let Some(model) = entry.model {
                explicit_models.insert(name.clone(), model);
            }
            if let Some(effort) = entry.effort {
                validate_effort(agent, &effort, &format!("worker mode '{name}'"))?;
                explicit_efforts.insert(name.clone(), effort);
            }
            if let Some(raw_runtime) = entry.runtime {
                let runtime = raw_runtime
                    .try_into::<ur_config::WorkerRuntime>()
                    .map_err(|e| format!("worker_modes.{name}.runtime: {e}"))?;
                mode_runtimes.insert(name.clone(), runtime);
            }
            strategies.insert(name.clone(), strategy);
            agents.insert(name.clone(), agent);
            modes.insert(name, entry.skills);
        }
        Ok(Self {
            modes,
            strategies,
            explicit_models,
            explicit_efforts,
            agents,
            default_agent,
            mode_runtimes,
            default_runtime,
            model_overrides,
        })
    }

    /// Resolve the runtime for a launch: `requested` (`--runtime`) →
    /// `worker_modes.<mode>.runtime` → `project` (`projects.<key>.runtime`) →
    /// top-level `runtime` → container. Workflow-dispatched launches pass
    /// `requested = None`. An empty mode name means "code".
    pub fn resolve_runtime(
        &self,
        mode: &str,
        requested: Option<ur_config::WorkerRuntime>,
        project: Option<ur_config::WorkerRuntime>,
    ) -> ur_config::WorkerRuntime {
        let mode_name = if mode.is_empty() { "code" } else { mode };
        ur_config::resolve_worker_runtime(
            requested,
            self.mode_runtimes.get(mode_name).copied(),
            project,
            self.default_runtime,
        )
    }

    /// Resolve skills for a launch request.
    ///
    /// Priority:
    /// 1. If `skills` is non-empty, use it directly.
    /// 2. If `mode` is non-empty, look up `worker_modes.<mode>.skills`.
    /// 3. Otherwise, use `worker_modes.code` (default).
    ///
    /// Returns an error if the requested mode name is not found.
    pub fn resolve_skills(&self, mode: &str, skills: &[String]) -> Result<Vec<String>, String> {
        if !skills.is_empty() {
            return Ok(skills.to_vec());
        }
        let mode_name = if mode.is_empty() { "code" } else { mode };
        self.modes
            .get(mode_name)
            .cloned()
            .ok_or_else(|| format!("unknown worker mode: {mode_name}"))
    }

    /// Resolve a mode name to its worker strategy, skill list, model, and agent.
    ///
    /// For built-in modes ("code", "design"), returns the corresponding
    /// `WorkerStrategy` variant, its default skills, and its default model.
    /// For custom modes, returns the strategy determined by the `base` field,
    /// the custom skills, and the resolved model (either the explicit
    /// `worker_modes.<name>.model` override or the base strategy's default).
    /// An empty mode name defaults to "code".
    ///
    /// Agent resolution order: `requested_agent` (explicit override, e.g. from
    /// the launch request) → the mode's `agent` field → the top-level `agent`
    /// default from `ur.toml` (claude if that key is omitted too).
    ///
    /// The model is resolved *after* the agent, against whichever agent wins
    /// that resolution — a mode's own explicit `model` pin always wins, but
    /// the computed default (`[worker_models.<agent>]` > `[worker_models]` >
    /// `agent.default_model`) must follow `requested_agent` when set, not
    /// whichever agent the mode was configured or defaulted to.
    pub fn resolve_mode(
        &self,
        mode: &str,
        requested_agent: Option<ur_config::AgentType>,
    ) -> Result<ResolvedMode, String> {
        let mode_name = if mode.is_empty() { "code" } else { mode };
        let strategy = self
            .strategies
            .get(mode_name)
            .copied()
            .ok_or_else(|| format!("unknown worker mode: {mode_name}"))?;
        let skills = self
            .modes
            .get(mode_name)
            .cloned()
            .ok_or_else(|| format!("unknown worker mode: {mode_name}"))?;
        let agent = requested_agent.unwrap_or_else(|| {
            self.agents
                .get(mode_name)
                .copied()
                .unwrap_or(self.default_agent)
        });
        let model = self
            .explicit_models
            .get(mode_name)
            .cloned()
            .unwrap_or_else(|| effective_default_model(agent, strategy, &self.model_overrides));
        let effort = self
            .explicit_efforts
            .get(mode_name)
            .cloned()
            .unwrap_or_else(|| effective_default_effort(agent, strategy, &self.model_overrides));
        Ok(ResolvedMode {
            strategy,
            skills,
            model,
            effort,
            agent,
        })
    }
}

/// What a worker mode name resolves to. Named rather than a positional tuple:
/// `skills` and `model` are both string-ish and every caller destructures all
/// four fields, so ordering mistakes would not be caught by the type checker.
#[derive(Debug, Clone)]
pub struct ResolvedMode {
    /// Strategy governing slot acquisition, release, and default skills.
    pub strategy: WorkerStrategy,
    /// Skills to install in the container, before global-skill merging.
    pub skills: Vec<String>,
    /// Model alias for the agent, or empty for "no model flag".
    pub model: String,
    /// Resolved reasoning effort for the agent.
    pub effort: String,
    /// Which agent runs this mode.
    pub agent: ur_config::AgentType,
}

/// Context for a running worker, returned by `WorkerManager::get_worker_context`.
#[derive(Debug, Clone)]
pub struct WorkerContext {
    /// Project key if launched with `--project`, or `None` for raw workspace launches.
    pub project_key: Option<String>,
    /// Host path to the repo slot (workspace dir or pool slot).
    pub slot_path: PathBuf,
    /// Worker strategy string as persisted in the database (e.g. `"code"`, `"design"`, `"manual"`).
    pub strategy: String,
    /// Host path of the brain mounted at `/brain`, if any.
    pub brain_path: Option<PathBuf>,
}

/// Summary of a running process, returned by `WorkerManager::list()`.
pub struct WorkerSummary {
    pub process_id: String,
    pub worker_id: String,
    pub container_id: String,
    pub project_key: String,
    pub mode: String,
    pub directory: String,
    pub container_status: String,
    pub agent_status: String,
    pub agent_type: String,
    pub runtime: ur_config::WorkerRuntime,
}

/// Configuration for launching a container process.
pub struct WorkerConfig {
    pub process_id: String,
    pub worker_id: WorkerId,
    pub image_id: String,
    pub cpus: u32,
    pub memory: String,
    pub workspace_dir: Option<PathBuf>,
    pub proxy_hostname: String,
    /// Project key if launched with `--project` (empty string for raw workspace launches).
    pub project_key: String,
    /// Which agent runs this worker (e.g. "claude"). Resolved from the launch
    /// request, defaulting to claude when unset — full mode-based resolution
    /// lands in a follow-up ticket.
    pub agent_type: String,
    /// Worker strategy governing slot acquisition and release behavior.
    pub strategy: WorkerStrategy,
    /// Resolved skills to pass as `UR_WORKER_SKILLS` env var (comma-separated).
    pub skills: Vec<String>,
    /// Resolved agent model name to pass as `UR_WORKER_MODEL` env var.
    /// Empty string means no `UR_WORKER_MODEL` env var is set, so
    /// `AgentType::spawn_command(None)` launches the agent with no model flag
    /// and it falls back to its own built-in default.
    pub model: String,
    /// Resolved reasoning effort to pass to workerd.
    pub effort: String,
    /// Optional project instruction-file (e.g. CLAUDE.md) template string from
    /// project config. When None, the server falls back to
    /// `<config_dir>/projects/<project_key>/{agent.instruction_filename()}`.
    pub instruction_md: Option<String>,
    /// Additional volume mounts from project config (source:destination pairs).
    pub mounts: Vec<ur_config::MountConfig>,
    /// Port mappings from project config (host_port:container_port pairs).
    pub ports: Vec<ur_config::PortMapping>,
    /// Context repositories to mount read-only into the container.
    /// Each entry is (project_key, host_path) from shared pool slots.
    pub context_mounts: Vec<(String, PathBuf)>,
    /// Relative paths to hostexec scripts declared by the project.
    /// Each entry is bind-mounted over `/workspace/<rel_path>` with the shim as the source.
    pub hostexec_scripts: Vec<String>,
    /// Extra skill mounts from global config, merged by `WorkerManager::merge_global_skills`.
    /// Each entry is `(name, host_path)` — mounted read-only at
    /// `/home/worker/.agent-shared/potential-skills/<name>` by `RunOptsBuilder::add_extra_skills`.
    pub extra_skill_mounts: Vec<(String, PathBuf)>,
    /// Optional memory directory template string from project config.
    /// Mounted at `/home/worker/.claude/projects/-workspace/memory` inside the container.
    pub memory_dir: Option<String>,
    /// Optional brain directory template string from project config.
    /// Mounted read-write at `/brain` inside the container.
    pub brain_dir: Option<String>,
    /// Optional brain directory template string from the top-level `workspace_brain_dir`
    /// config key. Only consulted when the worker has no project (`-w` workspace mode);
    /// see [`resolve_brain_dir`].
    pub workspace_brain_dir: Option<String>,
    /// Optional resolved forge platform for this worker's project.
    pub forge: Option<ur_config::ForgeType>,
    /// Resolved runtime, persisted on the worker row.
    pub runtime: ur_config::WorkerRuntime,
    /// Project `git.allow_no_verify`; local workers mirror it in their
    /// generated permission denies.
    pub allow_no_verify: bool,
}

/// Orchestrates the full lifecycle of worker processes:
/// repo registration, git init, container run/stop.
#[derive(Clone)]
pub struct WorkerManager {
    workspace: PathBuf,
    /// Host-side config directory path, used to construct volume mounts for
    /// worker containers (e.g., shared credentials file).
    pub(crate) host_config_dir: PathBuf,
    /// Locally-accessible config directory path, used for filesystem existence
    /// checks. Equals `host_config_dir` when the server runs natively; differs
    /// when the server runs inside a container where the config dir is mounted
    /// at a container-internal path (e.g. `/config`) while `host_config_dir`
    /// holds the real host path passed to builderd for Docker volume mounts.
    local_config_dir: PathBuf,
    /// Local (container-side) logs directory, used to create per-worker log
    /// directories before launching containers.
    logs_dir: PathBuf,
    /// Host-side logs directory path, used as the volume mount source for
    /// per-worker log directories (e.g., `~/.ur/logs/workers/<worker_id>/`).
    host_logs_dir: PathBuf,
    network_manager: NetworkManager,
    network_config: NetworkConfig,
    /// TCP port the shared worker gRPC server listens on.
    /// Injected into containers as part of `UR_SERVER_ADDR`.
    pub(crate) worker_port: u16,
    worker_modes: WorkerModesConfig,
    pub(crate) worker_repo: WorkerRepo,
    global_skills: GlobalSkillsConfig,
    /// gRPC client for launching/stopping containers via builderd on the host.
    builder_container_client: BuilderContainerClient,
    /// gRPC client for launching/stopping `runtime = local` workers via builderd.
    pub(crate) builder_local_worker_client: BuilderLocalWorkerClient,
}

impl WorkerManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace: PathBuf,
        host_config_dir: PathBuf,
        local_config_dir: PathBuf,
        logs_dir: PathBuf,
        host_logs_dir: PathBuf,
        network_manager: NetworkManager,
        network_config: NetworkConfig,
        worker_port: u16,
        worker_modes: WorkerModesConfig,
        worker_repo: WorkerRepo,
        global_skills: GlobalSkillsConfig,
        builder_container_client: BuilderContainerClient,
        builder_local_worker_client: BuilderLocalWorkerClient,
    ) -> Self {
        Self {
            workspace,
            host_config_dir,
            local_config_dir,
            logs_dir,
            host_logs_dir,
            network_manager,
            network_config,
            worker_port,
            worker_modes,
            worker_repo,
            global_skills,
            builder_container_client,
            builder_local_worker_client,
        }
    }

    /// Resolve skills for a launch request using the configured worker modes.
    pub fn resolve_skills(&self, mode: &str, skills: &[String]) -> Result<Vec<String>, String> {
        self.worker_modes.resolve_skills(mode, skills)
    }

    /// Resolve a launch's runtime; see [`WorkerModesConfig::resolve_runtime`].
    pub fn resolve_runtime(
        &self,
        mode: &str,
        requested: Option<ur_config::WorkerRuntime>,
        project: Option<ur_config::WorkerRuntime>,
    ) -> ur_config::WorkerRuntime {
        self.worker_modes.resolve_runtime(mode, requested, project)
    }

    /// Resolve a mode name to its worker strategy, skill list, model alias, and agent.
    pub fn resolve_mode(
        &self,
        mode: &str,
        requested_agent: Option<ur_config::AgentType>,
    ) -> Result<ResolvedMode, String> {
        self.worker_modes.resolve_mode(mode, requested_agent)
    }

    /// Merge project-required skills and global skills into `mode_skills`.
    ///
    /// Precedence order:
    /// 1. `mode_skills` (mode default or explicit launch override)
    /// 2. `project_skills` (from project configuration)
    /// 3. `global_skills` (from `[skills]` in `ur.toml` for the strategy)
    ///
    /// Duplicate names preserve the earliest source.
    ///
    /// Returns `(merged_names, extra_mounts)`:
    /// - `merged_names`: deduplicated skill names in precedence order.
    /// - `extra_mounts`: host paths for new global skill entries that need bind mounts.
    pub fn merge_skills(
        &self,
        strategy: WorkerStrategy,
        mode_skills: Vec<String>,
        project_skills: &[String],
    ) -> (Vec<String>, Vec<(String, PathBuf)>) {
        let mut seen: std::collections::HashSet<String> = mode_skills.iter().cloned().collect();
        let mut merged = mode_skills;
        for skill in project_skills {
            if seen.insert(skill.clone()) {
                merged.push(skill.clone());
            }
        }
        let globals = self.global_skills.for_strategy(strategy.name());
        let mut extra_mounts: Vec<(String, PathBuf)> = Vec::new();
        for global in globals {
            if seen.insert(global.name.clone()) {
                merged.push(global.name.clone());
                extra_mounts.push((global.name.clone(), global.host_path.clone()));
            }
        }
        (merged, extra_mounts)
    }

    /// Merge global skills from `ur.toml` into `mode_skills` based on the resolved strategy.
    ///
    /// Globals are always appended even when the launch supplied an explicit `--skills`
    /// override — `[skills]` in `ur.toml` declares "I want these everywhere."
    pub fn merge_global_skills(
        &self,
        strategy: WorkerStrategy,
        mode_skills: Vec<String>,
    ) -> (Vec<String>, Vec<(String, PathBuf)>) {
        self.merge_skills(strategy, mode_skills, &[])
    }

    /// Generate a new unique worker ID for the given process_id.
    pub fn generate_worker_id(&self, process_id: &str) -> WorkerId {
        WorkerId::generate(process_id)
    }

    /// Look up a worker by worker ID and return the associated process_id.
    pub async fn resolve_process_id(&self, worker_id: &WorkerId) -> Result<String, String> {
        let worker = self
            .worker_repo
            .get_worker(&worker_id.0)
            .await
            .map_err(|e| format!("db error: {e}"))?;
        worker
            .map(|w| w.process_id)
            .ok_or_else(|| format!("unknown worker: {worker_id}"))
    }

    /// Look up worker context (project_key, slot_path) by worker ID.
    /// Returns `None` if the worker is not registered or has no workspace_path.
    pub async fn get_worker_context(&self, worker_id: &WorkerId) -> Option<WorkerContext> {
        let worker = self.worker_repo.get_worker(&worker_id.0).await.ok()??;
        let workspace_path = worker.workspace_path?;
        let project_key = if worker.project_key.is_empty() {
            None
        } else {
            Some(worker.project_key)
        };
        Some(WorkerContext {
            project_key,
            slot_path: PathBuf::from(workspace_path),
            strategy: worker.strategy,
            brain_path: worker.brain_path.map(PathBuf::from),
        })
    }

    /// Verify that the given worker_id and secret match a registered worker.
    pub async fn verify_worker(&self, worker_id: &str, secret: &str) -> bool {
        let Ok(_parsed) = WorkerId::parse(worker_id) else {
            return false;
        };
        self.worker_repo
            .verify_worker(worker_id, secret)
            .await
            .unwrap_or(false)
    }

    /// Register a worker in the database without running a container.
    ///
    /// Used by tests that need a registered worker but cannot (or should not) spawn
    /// a real container. The caller supplies the worker_secret and container_id
    /// directly.
    #[allow(clippy::too_many_arguments)]
    pub async fn register_worker(
        &self,
        worker_id: WorkerId,
        process_id: String,
        project_key: String,
        slot_path: Option<PathBuf>,
        strategy: WorkerStrategy,
        container_id: String,
        worker_secret: String,
    ) {
        self.register_worker_with_brain(
            worker_id,
            process_id,
            project_key,
            slot_path,
            strategy,
            container_id,
            worker_secret,
            None,
        )
        .await;
    }

    /// [`Self::register_worker`] with the worker's host brain path recorded, as a
    /// launch with a brain mount would.
    #[allow(clippy::too_many_arguments)]
    pub async fn register_worker_with_brain(
        &self,
        worker_id: WorkerId,
        process_id: String,
        project_key: String,
        slot_path: Option<PathBuf>,
        strategy: WorkerStrategy,
        container_id: String,
        worker_secret: String,
        brain_path: Option<PathBuf>,
    ) {
        let now = Utc::now().to_rfc3339();
        let worker = workflow_db::model::Worker {
            worker_id: worker_id.0,
            process_id,
            project_key,
            container_id,
            worker_secret,
            strategy: strategy.name().to_owned(),
            agent_type: ur_config::AgentType::Claude.name().to_owned(),
            container_status: "running".to_owned(),
            agent_status: "starting".to_owned(),
            workspace_path: slot_path.map(|p| p.display().to_string()),
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
            brain_path: brain_path.map(|p| p.display().to_string()),
            runtime: ur_config::WorkerRuntime::Container,
        };
        self.worker_repo
            .insert_worker(&worker)
            .await
            .expect("failed to register worker");
    }

    /// Phase 1 of launch: create repo dir and git init.
    /// When `workspace_dir` is Some, the directory is used as-is (no git init).
    /// When None, creates a new directory under `self.workspace` and runs git init.
    /// Returns the resolved workspace path for volume mounting.
    /// The caller is responsible for calling `run_and_record` after `prepare`.
    pub async fn prepare(
        &self,
        process_id: &str,
        worker_id: &WorkerId,
        workspace_dir: Option<PathBuf>,
    ) -> Result<Option<PathBuf>, String> {
        // Check for duplicate process ID via database
        let running = self
            .worker_repo
            .list_workers_by_container_status("running")
            .await
            .map_err(|e| format!("db error: {e}"))?;
        let provisioning = self
            .worker_repo
            .list_workers_by_container_status("provisioning")
            .await
            .map_err(|e| format!("db error: {e}"))?;
        // A pool launch's own claim row is already `provisioning`.
        let has_duplicate = running
            .iter()
            .chain(provisioning.iter())
            .any(|a| a.process_id == process_id && a.worker_id != worker_id.0);
        if has_duplicate {
            return Err(format!("process already running: {process_id}"));
        }

        if let Some(ws_dir) = workspace_dir {
            // External workspace: skip git init (worker.workspace_path in DB handles CWD resolution)
            info!(process_id, %worker_id, workspace_dir = %ws_dir.display(), "registering external workspace");
            Ok(Some(ws_dir))
        } else {
            // Default: create repo dir and git init
            info!(process_id, %worker_id, "creating repo directory and initializing git");
            let repo_dir = self.workspace.join(process_id);
            tokio::fs::create_dir_all(&repo_dir)
                .await
                .map_err(|e| format!("failed to create repo dir: {e}"))?;

            let git_init = tokio::process::Command::new("git")
                .args(["init"])
                .current_dir(&repo_dir)
                .output()
                .await
                .map_err(|e| format!("failed to run git init: {e}"))?;
            if !git_init.status.success() {
                return Err(format!(
                    "git init failed: {}",
                    String::from_utf8_lossy(&git_init.stderr)
                ));
            }
            Ok(Some(repo_dir))
        }
    }

    /// Phase 2 of launch: run the container and record the worker in the database.
    /// Generates and stores a worker secret (UUID v4) for auth.
    /// Returns `(container_id, worker_secret)`.
    pub async fn run_and_record(&self, config: WorkerConfig) -> Result<(String, String), String> {
        if config.runtime == ur_config::WorkerRuntime::Local {
            return self.run_and_record_local(config).await;
        }
        // Create per-worker log directory on the local filesystem so the
        // volume mount has a source directory when the container starts.
        let worker_logs_dir = self.logs_dir.join("workers").join(&config.worker_id.0);
        tokio::fs::create_dir_all(&worker_logs_dir)
            .await
            .map_err(|e| format!("failed to create worker logs dir: {e}"))?;

        // Ensure the Docker network exists before launching the container
        self.network_manager
            .ensure()
            .await
            .map_err(|e| format!("failed to ensure Docker network: {e}"))?;

        // Generate worker secret for worker auth
        let worker_secret = Uuid::new_v4().to_string();

        // Named by worker id (unique per launch); builderd also labels it
        // with the worker id, which is how Stop and List find it.
        let container_name = self
            .network_config
            .worker_container_name(&config.worker_id.0);
        let mut opts = self.build_launch_request(&config, &worker_secret, &container_name)?;
        opts.worker_id = config.worker_id.0.clone();

        // Workerd authenticates with the server during initialization, before
        // its health endpoint comes up. Persist the credential first so that
        // builderd can wait for readiness without deadlocking authentication.
        self.record_provisioning_worker(&config, &container_name, &worker_secret)
            .await?;

        // Launch the container via builderd on the host, which stats each volume
        // source against the host filesystem before calling docker run.
        if let Err(status) = self.builder_container_client.launch_worker(opts).await {
            let error = format!("launch_worker gRPC error: {status}");
            return Err(self.abandon_launch(&config.worker_id, error).await);
        }
        info!(
            process_id = config.process_id,
            worker_id = %config.worker_id,
            container_name,
            "process launched"
        );
        self.finish_launch(&config, container_name, worker_secret)
            .await
    }

    /// Resolve all host paths and configuration needed by builderd to launch
    /// one worker container.
    fn build_launch_request(
        &self,
        config: &WorkerConfig,
        worker_secret: &str,
        container_name: &str,
    ) -> Result<LaunchWorkerRequest, String> {
        let env_vars = build_worker_env_vars(
            config,
            worker_secret,
            &self.network_config,
            self.worker_port,
        );
        let agent =
            ur_config::AgentType::parse(&config.agent_type).unwrap_or(ur_config::AgentType::Claude);
        let project_instruction = resolve_project_instruction(
            &config.instruction_md,
            &config.project_key,
            &self.host_config_dir,
            agent,
        );
        let memory_dir = resolve_memory_dir(
            &config.memory_dir,
            &config.project_key,
            &self.host_config_dir,
        );
        let brain_dir = resolve_brain_dir(
            &config.brain_dir,
            &config.workspace_brain_dir,
            &config.project_key,
            &self.host_config_dir,
        );
        let shim_host_path = self
            .host_config_dir
            .join(ur_config::HOSTEXEC_DIR)
            .join("script-shim.sh");

        Ok(RunOptsBuilder::new(
            config.image_id.clone(),
            container_name.to_owned(),
            self.network_manager.network_name().to_string(),
        )
        .cpus(config.cpus)
        .memory(config.memory.clone())
        .workdir("/workspace")
        .add_workspace(&config.workspace_dir)
        .add_logs_dir(&self.host_logs_dir, &self.logs_dir, &config.worker_id.0)
        .add_credentials(&self.host_config_dir, &self.local_config_dir, agent)?
        .add_host_hooks_overlay(
            &config.project_key,
            &self.host_config_dir,
            &self.local_config_dir,
        )
        .add_extra_skills(&config.extra_skill_mounts)
        .add_project_instruction(&project_instruction, &self.host_config_dir, agent)?
        .add_memory_dir(&memory_dir, &self.host_config_dir, agent)?
        .add_brain_dir(&brain_dir, &self.host_config_dir)?
        .add_mounts(&config.mounts, &self.host_config_dir)?
        .add_mounts(
            &context_mount_configs(&config.context_mounts),
            &self.host_config_dir,
        )?
        .add_project_hostexec_scripts(
            &config.hostexec_scripts,
            &shim_host_path,
            // Builderd validates volume sources against the host filesystem.
            None,
        )?
        .add_ports(&config.ports)
        .add_env_vars(env_vars)
        .build())
    }

    /// Persist the worker credential before container launch so workerd can
    /// authenticate while builderd waits for its health endpoint.
    pub(crate) async fn record_provisioning_worker(
        &self,
        config: &WorkerConfig,
        container_name: &str,
        worker_secret: &str,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        let worker = workflow_db::model::Worker {
            worker_id: config.worker_id.0.clone(),
            process_id: config.process_id.clone(),
            project_key: config.project_key.clone(),
            container_id: container_name.to_owned(),
            worker_secret: worker_secret.to_owned(),
            strategy: config.strategy.name().to_owned(),
            agent_type: config.agent_type.clone(),
            container_status: "provisioning".to_owned(),
            agent_status: "starting".to_owned(),
            workspace_path: config
                .workspace_dir
                .as_ref()
                .map(|path| path.display().to_string()),
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
            brain_path: brain_host_path(config, &self.host_config_dir),
            runtime: config.runtime,
        };
        let db = |e: sqlx::Error| format!("failed to record worker: {e}");
        // A pool launch claimed its slot with this row already; refresh it.
        if self
            .worker_repo
            .update_provisioning_worker(&worker)
            .await
            .map_err(db)?
        {
            return Ok(());
        }
        match self.worker_repo.insert_worker(&worker).await {
            // The claim row exists but is no longer `provisioning`.
            Err(sqlx::Error::Database(e)) if e.code().is_some_and(|c| c == "23505") => {
                Err(stopped_during_launch(&config.worker_id))
            }
            result => result.map_err(db),
        }
    }

    /// Finalize (`provisioning → running`, CAS) after builderd's Launch was
    /// Ok. If finalize changes no row (the worker was stopped mid-launch) or
    /// fails, the worker is stopped like any failed launch
    /// ([`Self::abandon_launch`]). Returns `(recorded, worker_secret)`;
    /// `recorded` is the container or session name.
    pub(crate) async fn finish_launch(
        &self,
        config: &WorkerConfig,
        recorded: String,
        worker_secret: String,
    ) -> Result<(String, String), String> {
        let finalized = self
            .worker_repo
            .finalize_worker_launch(&config.worker_id.0, &recorded)
            .await
            .map_err(|e| format!("failed to finalize worker launch: {e}"));
        match finalized {
            Ok(true) => Ok((recorded, worker_secret)),
            Ok(false) => Err(self
                .abandon_launch(&config.worker_id, stopped_during_launch(&config.worker_id))
                .await),
            Err(error) => Err(self.abandon_launch(&config.worker_id, error).await),
        }
    }

    /// Whether `worker` runs a container launched before containers were
    /// labeled by worker id ([`legacy_container_id`]); such a container is
    /// never listed by builderd.
    pub fn is_legacy_container(&self, worker: &workflow_db::model::Worker) -> bool {
        legacy_container_id(&self.network_config.worker_prefix, worker).is_some()
    }

    /// Clean up after a failed launch: stop it like any other worker, since
    /// a failed launch may still have left a live worker. Releases the slot
    /// (row `stopped`) only once builderd's Stop is Ok; otherwise the row
    /// stays `stopping`, still holding the slot. Returns the launch error.
    pub(crate) async fn abandon_launch(&self, worker_id: &WorkerId, error: String) -> String {
        match self.stop_by_worker_id(worker_id).await {
            Ok(()) => error,
            Err(e) => format!("{error}; failed to clean up, left stopping: {e}"),
        }
    }

    pub(crate) async fn remove_provisioning_worker(
        &self,
        worker_id: &WorkerId,
        error: String,
    ) -> String {
        match self.worker_repo.delete_worker(&worker_id.0).await {
            Ok(_) => error,
            Err(cleanup) => format!("{error}; failed to remove provisional worker: {cleanup}"),
        }
    }

    /// Stop a running worker process by worker ID. Stops + removes the container.
    pub async fn stop_by_worker_id(&self, worker_id: &WorkerId) -> Result<(), String> {
        let worker = self
            .worker_repo
            .get_worker(&worker_id.0)
            .await
            .map_err(|e| format!("db error: {e}"))?
            .ok_or_else(|| format!("unknown worker: {worker_id}"))?;

        info!(
            process_id = worker.process_id,
            %worker_id,
            container_id = worker.container_id,
            "stopping container"
        );

        // Any stopper finishes any stop: `stopping` (if active), builderd
        // Stop, then `finish_stop` (a no-op unless `stopping`). builderd's Stop
        // is a barrier per worker id: it waits for an in-flight launch (or
        // refuses a later one), so Ok means the worker is not running and
        // never will be, even mid-launch. A stopped row is still stopped in
        // builderd (a drifted row may own a live container); nothing to release.
        self.worker_repo
            .begin_stopping(&worker_id.0)
            .await
            .map_err(|e| format!("failed to update worker status: {e}"))?;
        match worker.runtime {
            ur_config::WorkerRuntime::Container => self
                .builder_container_client
                .stop_worker(StopWorkerRequest {
                    worker_id: worker_id.0.clone(),
                    legacy_container_id: legacy_container_id(
                        &self.network_config.worker_prefix,
                        &worker,
                    )
                    .unwrap_or_default()
                    .to_owned(),
                })
                .await
                .map(|_| ())
                .map_err(|status| format!("stop_worker gRPC error: {status}"))?,
            ur_config::WorkerRuntime::Local => self.stop_local_worker(worker_id).await?,
        }
        self.worker_repo
            .finish_stop(&worker_id.0)
            .await
            .map_err(|e| format!("failed to mark worker stopped: {e}"))?;

        info!(
            process_id = worker.process_id,
            %worker_id,
            "process stopped"
        );

        Ok(())
    }

    /// List all running processes with their metadata.
    pub async fn list(&self) -> Vec<WorkerSummary> {
        let workers = self
            .worker_repo
            .list_workers_by_container_status("running")
            .await
            .unwrap_or_default();
        let mut result: Vec<WorkerSummary> = workers
            .into_iter()
            .map(|worker| WorkerSummary {
                process_id: worker.process_id,
                worker_id: worker.worker_id,
                container_id: worker.container_id,
                project_key: worker.project_key,
                mode: worker.strategy,
                directory: worker.workspace_path.unwrap_or_default(),
                container_status: worker.container_status,
                agent_status: worker.agent_status,
                agent_type: worker.agent_type,
                runtime: worker.runtime,
            })
            .collect();
        result.sort_by(|a, b| a.process_id.cmp(&b.process_id));
        result
    }

    /// Look up the workspace/slot directory for a running process by its process ID.
    pub async fn get_workspace_dir(&self, process_id: &str) -> Result<Option<PathBuf>, String> {
        let workers = self
            .worker_repo
            .list_workers_by_container_status("running")
            .await
            .map_err(|e| format!("db error: {e}"))?;
        let worker = workers
            .iter()
            .find(|w| w.process_id == process_id)
            .ok_or_else(|| format!("unknown worker: {process_id}"))?;
        Ok(worker.workspace_path.as_ref().map(PathBuf::from))
    }

    /// Stop a running worker process by process_id (searches all entries).
    /// Used by the CLI which only knows the process_id, not the worker_id.
    ///
    /// A process_id names a slot, and every worker that has ever occupied that
    /// slot shares it, so the newest running instance is the one a stop means.
    pub async fn stop(&self, process_id: &str) -> Result<(), String> {
        let workers = self
            .worker_repo
            .list_workers_by_container_status("running")
            .await
            .map_err(|e| format!("db error: {e}"))?;
        // list_workers_by_container_status orders oldest-first; take the last
        // match so a re-used slot stops its current worker, not a stale row.
        if let Some(worker) = workers.iter().rev().find(|w| w.process_id == process_id) {
            let worker_id = WorkerId::parse(&worker.worker_id)?;
            return self.stop_by_worker_id(&worker_id).await;
        }
        Err(format!("unknown worker: {process_id}"))
    }
}

fn stopped_during_launch(worker_id: &WorkerId) -> String {
    format!("worker {worker_id} was stopped during launch")
}

/// The Docker ID (or name) of a container launched before containers were
/// labeled by worker id (ur-ezhdb), or `None`. Such a row recorded something
/// other than its worker-id name; builderd's Stop then also stops that
/// container, but only after checking its env names this worker. Upgrade
/// only: such containers are not listed, and the reconciler leaves their
/// `running` rows alone.
pub fn legacy_container_id<'a>(
    worker_prefix: &str,
    worker: &'a workflow_db::model::Worker,
) -> Option<&'a str> {
    let named = format!("{worker_prefix}{}", worker.worker_id);
    let legacy = worker.runtime == ur_config::WorkerRuntime::Container
        && !worker.container_id.is_empty()
        && worker.container_id != named;
    legacy.then_some(worker.container_id.as_str())
}

/// Build the environment variables for a worker container launch.
///
/// Assembles server address, worker identity, proxy, skills, agent type,
/// strategy instruction-file name, project key, and host workspace path into
/// a list of key-value pairs.
fn build_worker_env_vars(
    config: &WorkerConfig,
    worker_secret: &str,
    network_config: &ur_config::NetworkConfig,
    worker_port: u16,
) -> Vec<(String, String)> {
    let server_addr = format!("{}:{}", network_config.server_hostname, worker_port);
    let mut env_vars = vec![
        (ur_config::UR_SERVER_ADDR_ENV.into(), server_addr),
        (
            ur_config::UR_WORKER_ID_ENV.into(),
            config.worker_id.0.clone(),
        ),
        (
            ur_config::UR_WORKER_SECRET_ENV.into(),
            worker_secret.to_owned(),
        ),
    ];

    // Inject proxy env vars (Squid proxy reachable via Docker DNS on the internal network)
    env_vars.extend(proxy_env_vars(&config.proxy_hostname));

    // Inject resolved skills as comma-separated list
    let skills = worker_skills(config);
    if !skills.is_empty() {
        env_vars.push((ur_config::UR_WORKER_SKILLS_ENV.into(), skills.join(",")));
    }

    // Inject which agent this worker runs, so workerd can resolve
    // agent-specific paths and the spawn command via `AgentType::from_env()`.
    env_vars.push((
        ur_config::UR_AGENT_TYPE_ENV.into(),
        config.agent_type.clone(),
    ));

    // Inject strategy-specific instruction-file name for workerd to copy at init
    env_vars.push((
        ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV.into(),
        config.strategy.instruction_strategy_name().into(),
    ));

    // Inject the resolved model name (only when non-empty — empty means the
    // agent launches with no model flag and uses its own built-in default,
    // which is also what an agent with no model concept resolves to)
    if !config.model.is_empty() {
        env_vars.push((ur_config::UR_WORKER_MODEL_ENV.into(), config.model.clone()));
    }
    env_vars.push((
        ur_config::UR_WORKER_EFFORT_ENV.into(),
        config.effort.clone(),
    ));

    // Inject resolved forge platform (if any)
    if let Some(forge) = config.forge {
        env_vars.push((
            ur_config::UR_WORKER_FORGE_ENV.into(),
            forge.as_str().to_string(),
        ));
    }

    // Inject project key so workers can resolve project context via env
    if !config.project_key.is_empty() {
        env_vars.push(("UR_PROJECT".into(), config.project_key.clone()));
    }

    // Inject host workspace path so workers know the host-side mount source
    if let Some(ref ws_dir) = config.workspace_dir {
        env_vars.push((
            ur_config::UR_HOST_WORKSPACE_ENV.into(),
            ws_dir.display().to_string(),
        ));
    }

    env_vars
}

/// The worker's skill names: `config.skills`, plus `gitea` for a Gitea forge.
pub(crate) fn worker_skills(config: &WorkerConfig) -> Vec<String> {
    let mut skills = config.skills.clone();
    if config.forge == Some(ur_config::ForgeType::Gitea) && !skills.iter().any(|s| s == "gitea") {
        skills.push("gitea".to_string());
    }
    skills
}

/// Map a host convention path to the path the server can actually touch.
///
/// Used both for existence checks and for creating directories the server must
/// materialize before Docker mounts them (see `RunOptsBuilder::add_brain_dir`).
///
/// Convention resolvers build paths under `host_config_dir` (the *host* path, e.g.
/// `/Users/me/.ur`) because those paths become Docker volume-mount sources, which the
/// Docker daemon interprets on the host. But the server itself runs inside a container
/// where the host path is not visible — only the config bind mount at `UR_CONFIG`
/// (e.g. `/config`) is. Calling `.exists()` on the host path from inside the container
/// therefore always returns `false`, silently disabling every convention fallback.
///
/// When `UR_HOST_CONFIG` is set (indicating we are inside the server container), remap
/// the `host_config_dir` prefix onto `UR_CONFIG` so the existence check hits the visible
/// bind mount. Outside the container (host CLI, tests) the two directories are identical
/// and the path is returned unchanged. Returned mount sources always use the host path.
pub(crate) fn convention_check_path(
    host_path: &std::path::Path,
    host_config_dir: &std::path::Path,
) -> PathBuf {
    // Only remap when running inside the server container: `UR_HOST_CONFIG` set (host path
    // differs from the bind mount) AND `UR_CONFIG` giving the container-visible mount point.
    let container_config = match (
        std::env::var(ur_config::UR_HOST_CONFIG_ENV),
        std::env::var(ur_config::UR_CONFIG_ENV),
    ) {
        (Ok(_), Ok(container_config)) => Some(PathBuf::from(container_config)),
        _ => None,
    };
    remap_to_container_config(host_path, host_config_dir, container_config.as_deref())
}

/// Pure core of [`convention_check_path`]. When `container_config` is `Some`, rebase any
/// `host_config_dir`-relative `host_path` onto it; otherwise (or when `host_path` is not
/// under `host_config_dir`) return `host_path` unchanged.
fn remap_to_container_config(
    host_path: &std::path::Path,
    host_config_dir: &std::path::Path,
    container_config: Option<&std::path::Path>,
) -> PathBuf {
    let Some(container_config) = container_config else {
        return host_path.to_path_buf();
    };
    match host_path.strip_prefix(host_config_dir) {
        Ok(rel) => container_config.join(rel),
        Err(_) => host_path.to_path_buf(),
    }
}

/// Resolve the project instruction-file template string, falling back to the
/// convention path.
///
/// When `instruction_md` is already set (from project config), returns it as-is.
/// When `instruction_md` is None and a non-empty `project_key` is provided,
/// checks `<host_config_dir>/projects/<project_key>/{agent.instruction_filename()}`
/// — if it exists, returns the absolute path as a host path string.
pub(crate) fn resolve_project_instruction(
    instruction_md: &Option<String>,
    project_key: &str,
    host_config_dir: &std::path::Path,
    agent: ur_config::AgentType,
) -> Option<String> {
    if instruction_md.is_some() {
        return instruction_md.clone();
    }
    if project_key.is_empty() {
        return None;
    }
    let convention_path = host_config_dir
        .join("projects")
        .join(project_key)
        .join(agent.instruction_filename());
    if convention_check_path(&convention_path, host_config_dir).exists() {
        Some(convention_path.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// Resolve the project memory directory path, falling back to the convention path.
///
/// When `memory_dir` is already set (from project config), returns it as-is.
/// When `memory_dir` is None and a non-empty `project_key` is provided, checks
/// `<host_config_dir>/projects/<project_key>/memory/` — if it exists, returns
/// the absolute path as a host path string.
pub(crate) fn resolve_memory_dir(
    memory_dir: &Option<String>,
    project_key: &str,
    host_config_dir: &std::path::Path,
) -> Option<String> {
    if memory_dir.is_some() {
        return memory_dir.clone();
    }
    if project_key.is_empty() {
        return None;
    }
    let convention_path = host_config_dir
        .join("projects")
        .join(project_key)
        .join("memory");
    if convention_check_path(&convention_path, host_config_dir).exists() {
        Some(convention_path.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// Resolve the brain directory path to mount at `/brain`.
///
/// Resolution order:
/// 1. `brain_dir` set (from project config) → use it as-is.
/// 2. `project_key` empty (a `-w` workspace-mode launch with no project) →
///    `workspace_brain_dir` from the top-level config, or None when unset. There is
///    deliberately no convention fallback here: the workspace brain must be configured
///    explicitly so an upgrade never starts mounting a brain nobody asked for.
/// 3. `<host_config_dir>/projects/<project_key>/brain/` if it exists on disk.
/// 4. None — no brain mount.
pub(crate) fn resolve_brain_dir(
    brain_dir: &Option<String>,
    workspace_brain_dir: &Option<String>,
    project_key: &str,
    host_config_dir: &std::path::Path,
) -> Option<String> {
    if brain_dir.is_some() {
        return brain_dir.clone();
    }
    if project_key.is_empty() {
        return workspace_brain_dir.clone();
    }
    let convention_path = ur_config::brain_convention_path(host_config_dir, project_key);
    if convention_check_path(&convention_path, host_config_dir).exists() {
        Some(convention_path.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// Host path of the brain this worker gets mounted at `/brain`, or `None` without one.
///
/// Uses the same resolution as the mount itself ([`resolve_brain_dir`] then template
/// resolution), so hostexec maps `/brain` to exactly the directory that was mounted.
fn brain_host_path(config: &WorkerConfig, host_config_dir: &std::path::Path) -> Option<String> {
    let template = resolve_brain_dir(
        &config.brain_dir,
        &config.workspace_brain_dir,
        &config.project_key,
        host_config_dir,
    )?;
    match ur_config::resolve_template_path(&template, host_config_dir).ok()? {
        ur_config::ResolvedTemplatePath::HostPath(path) => {
            Some(path.to_string_lossy().into_owned())
        }
        ur_config::ResolvedTemplatePath::ProjectRelative(_) => None,
    }
}

/// Convert context mounts (project_key, host_path) to `MountConfig` entries.
///
/// Each context repo is mounted read-only at `/context/<project_key>` inside
/// the container. The host path is an absolute path to the shared pool slot.
pub(crate) fn context_mount_configs(
    context_mounts: &[(String, PathBuf)],
) -> Vec<ur_config::MountConfig> {
    context_mounts
        .iter()
        .map(|(project_key, host_path)| ur_config::MountConfig {
            source: host_path.display().to_string(),
            destination: format!("/context/{project_key}"),
            readonly: false,
        })
        .collect()
}

/// Ensure a file exists on disk, creating it (and parent dirs) if missing.
/// Docker bind-mounts require the source to exist as a file; if missing, Docker
/// creates a directory instead, causing an OCI runtime error.
pub(crate) fn ensure_file_exists(path: &PathBuf) -> Result<(), std::io::Error> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::os::unix::fs::OpenOptionsExt;

    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

/// Build `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` env var pairs for container injection.
///
/// Uses `http://` scheme even for `HTTPS_PROXY` — this tells the client to speak plain HTTP
/// to the proxy, which then tunnels TLS traffic via CONNECT.
fn proxy_env_vars(proxy_hostname: &str) -> Vec<(String, String)> {
    let proxy_url = format!("http://{proxy_hostname}:{}", ur_config::SQUID_PORT);
    vec![
        ("HTTP_PROXY".into(), proxy_url.clone()),
        ("HTTPS_PROXY".into(), proxy_url),
        ("NO_PROXY".into(), String::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_worker_repo() -> (WorkerRepo, ur_db_test::TestDb) {
        let test_db = ur_db_test::TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());
        (repo, test_db)
    }

    async fn test_manager() -> (WorkerManager, tempfile::TempDir, ur_db_test::TestDb) {
        let workspace = tempfile::tempdir().unwrap();
        let (worker_repo, test_db) = test_worker_repo().await;
        // Keep unit tests isolated from a builderd that may be running on its
        // normal development port. The lazy channel only connects in tests
        // that deliberately exercise an unreachable runtime.
        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let builder_container_client = BuilderContainerClient::new(channel.clone());
        let network_manager = NetworkManager::new(
            builder_container_client.clone(),
            ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
        );
        let network_config = NetworkConfig {
            name: ur_config::DEFAULT_NETWORK_NAME.into(),
            worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
            server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
            worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.into(),
        };
        let mgr = WorkerManager::new(
            workspace.path().to_path_buf(),
            workspace.path().to_path_buf(),
            workspace.path().join("logs"),
            workspace.path().join("logs"),
            workspace.path().join("logs"),
            network_manager,
            network_config,
            ur_config::DEFAULT_SERVER_PORT + 1,
            WorkerModesConfig::default(),
            worker_repo,
            GlobalSkillsConfig::default(),
            builder_container_client,
            crate::builder_local_worker_client::BuilderLocalWorkerClient::new(
                tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy(),
            ),
        );
        (mgr, workspace, test_db)
    }

    /// The provisional row written before builderd's launch carries the
    /// launch's resolved runtime, not a hardcoded container.
    #[tokio::test]
    async fn provisioning_row_persists_resolved_runtime() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let mut config = test_worker_config(WorkerStrategy::Code, "sonnet");
        config.runtime = ur_config::WorkerRuntime::Local;
        mgr.record_provisioning_worker(&config, "ur-worker-test-proc", "secret")
            .await
            .unwrap();
        let row = mgr
            .worker_repo
            .get_worker(&config.worker_id.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.runtime, ur_config::WorkerRuntime::Local);
    }

    #[tokio::test]
    async fn prepare_creates_repo_and_registers() {
        let (mgr, workspace, _test_db) = test_manager().await;
        let process_id = "test-proc";
        let wid = mgr.generate_worker_id(process_id);

        let result = mgr.prepare(process_id, &wid, None).await.unwrap();

        // Verify repo dir exists and has .git
        let repo_dir = workspace.path().join(process_id);
        assert!(repo_dir.join(".git").exists());
        assert_eq!(result, Some(repo_dir));
    }

    #[tokio::test]
    async fn prepare_with_workspace_skips_git_init() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let process_id = "ws-proc";
        let wid = mgr.generate_worker_id(process_id);

        // Create a temp dir to act as the external workspace
        let ext_workspace = tempfile::tempdir().unwrap();
        let ext_path = ext_workspace.path().to_path_buf();

        let result = mgr
            .prepare(process_id, &wid, Some(ext_path.clone()))
            .await
            .unwrap();

        // Should NOT have a .git dir — we skipped git init
        assert!(!ext_path.join(".git").exists());
        assert_eq!(result, Some(ext_path));
    }

    #[tokio::test]
    async fn prepare_duplicate_process_id_returns_error() {
        let (mgr, _workspace, _test_db) = test_manager().await;

        let existing_wid = WorkerId("dup-proc-ab12".into());
        // Insert a running worker into the database
        mgr.register_worker(
            existing_wid,
            "dup-proc".into(),
            String::new(),
            None,
            WorkerStrategy::Code,
            "fake-cid".into(),
            Uuid::new_v4().to_string(),
        )
        .await;

        // A new worker_id with a different suffix should still be rejected
        // because the process_id matches.
        let new_wid = WorkerId("dup-proc-zz99".into());
        let result = mgr.prepare("dup-proc", &new_wid, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("process already running"));
    }

    /// A stop by process id with no running row is an unknown worker: it
    /// never falls back to a container name (or any builderd call); a
    /// container no row wants is the reconciler's List sweep's.
    #[tokio::test]
    async fn stop_unknown_process_is_an_unknown_worker() {
        let (mut mgr, _workspace, _test_db) = test_manager().await;
        let unreachable =
            tonic::transport::Channel::from_static("http://127.0.0.1:0").connect_lazy();
        mgr.builder_container_client = BuilderContainerClient::new(unreachable);
        let err = mgr.stop("nonexistent").await.unwrap_err();
        assert_eq!(err, "unknown worker: nonexistent");
    }

    #[test]
    fn worker_id_generate_format() {
        let id = WorkerId::generate("deploy");
        assert!(
            id.0.starts_with("deploy-"),
            "expected deploy- prefix: {}",
            id.0
        );
        let suffix = &id.0["deploy-".len()..];
        assert_eq!(suffix.len(), 4, "expected 4-char suffix: {suffix}");
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "suffix must be [a-z0-9]: {suffix}"
        );
    }

    #[test]
    fn worker_id_parse_valid() {
        let id = WorkerId::parse("deploy-x7q2").unwrap();
        assert_eq!(id.0, "deploy-x7q2");
    }

    #[test]
    fn worker_id_parse_rejects_bad_suffix() {
        assert!(WorkerId::parse("deploy-ABCD").is_err());
        assert!(WorkerId::parse("deploy-abc").is_err());
        assert!(WorkerId::parse("deploy-abcde").is_err());
        assert!(WorkerId::parse("nodash").is_err());
        assert!(WorkerId::parse("-ab12").is_err());
    }

    #[test]
    fn worker_id_parse_with_multiple_dashes() {
        // process_id itself can contain dashes; we use rfind for the last dash
        let id = WorkerId::parse("my-proc-x7q2").unwrap();
        assert_eq!(id.0, "my-proc-x7q2");
    }

    #[tokio::test]
    async fn resolve_process_id_works() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let wid = WorkerId("test-ab12".into());
        mgr.register_worker(
            wid.clone(),
            "test".into(),
            "myproject".into(),
            None,
            WorkerStrategy::Code,
            "cid".into(),
            Uuid::new_v4().to_string(),
        )
        .await;
        assert_eq!(mgr.resolve_process_id(&wid).await.unwrap(), "test");
        assert!(
            mgr.resolve_process_id(&WorkerId("unknown-ab12".into()))
                .await
                .is_err()
        );
    }

    #[test]
    fn proxy_env_vars_uses_squid_hostname() {
        let vars = proxy_env_vars("ur-squid");
        assert_eq!(vars.len(), 3);
        assert_eq!(
            vars[0],
            ("HTTP_PROXY".into(), "http://ur-squid:3128".into())
        );
        assert_eq!(
            vars[1],
            ("HTTPS_PROXY".into(), "http://ur-squid:3128".into())
        );
        assert_eq!(vars[2], ("NO_PROXY".into(), String::new()));
    }

    #[test]
    fn proxy_env_vars_uses_http_scheme_for_https() {
        let vars = proxy_env_vars("ur-squid");
        let https_val = &vars[1].1;
        assert!(
            https_val.starts_with("http://"),
            "HTTPS_PROXY must use http:// scheme (proxy speaks plain HTTP, tunnels via CONNECT)"
        );
    }

    fn test_worker_config(strategy: WorkerStrategy, model: &str) -> WorkerConfig {
        WorkerConfig {
            process_id: "test-proc".into(),
            worker_id: WorkerId("test-proc-ab12".into()),
            image_id: "ur-worker-claude:latest".into(),
            cpus: 1,
            memory: "512m".into(),
            workspace_dir: None,
            proxy_hostname: "ur-squid".into(),
            project_key: String::new(),
            agent_type: ur_config::AgentType::Claude.name().to_owned(),
            strategy,
            skills: Vec::new(),
            model: model.into(),
            effort: "medium".into(),
            instruction_md: None,
            mounts: Vec::new(),
            ports: Vec::new(),
            context_mounts: Vec::new(),
            hostexec_scripts: Vec::new(),
            extra_skill_mounts: Vec::new(),
            memory_dir: None,
            brain_dir: None,
            workspace_brain_dir: None,
            forge: None,
            runtime: ur_config::WorkerRuntime::Container,
            allow_no_verify: false,
        }
    }

    fn test_network_config() -> ur_config::NetworkConfig {
        NetworkConfig {
            name: ur_config::DEFAULT_NETWORK_NAME.into(),
            worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
            server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
            worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.into(),
        }
    }

    #[test]
    fn build_worker_env_vars_design_mode_sets_model() {
        let cfg = WorkerModesConfig::default();
        let ResolvedMode {
            strategy, model, ..
        } = cfg.resolve_mode("design", None).unwrap();
        let config = test_worker_config(strategy, &model);
        let vars = build_worker_env_vars(&config, "secret", &test_network_config(), 12322);
        assert!(
            vars.contains(&("UR_WORKER_MODEL".into(), "opus".into())),
            "design mode should inject UR_WORKER_MODEL=opus; got {vars:?}"
        );
    }

    #[test]
    fn build_worker_env_vars_code_mode_sets_sonnet_model() {
        let cfg = WorkerModesConfig::default();
        let ResolvedMode {
            strategy, model, ..
        } = cfg.resolve_mode("code", None).unwrap();
        let config = test_worker_config(strategy, &model);
        let vars = build_worker_env_vars(&config, "secret", &test_network_config(), 12322);
        assert!(
            vars.contains(&("UR_WORKER_MODEL".into(), "sonnet".into())),
            "code mode should inject UR_WORKER_MODEL=sonnet; got {vars:?}"
        );
        assert!(
            vars.contains(&(ur_config::UR_WORKER_EFFORT_ENV.into(), "medium".into())),
            "code mode should inject the default effort; got {vars:?}"
        );
    }

    #[test]
    fn build_worker_env_vars_empty_model_omits_env_var() {
        let config = test_worker_config(WorkerStrategy::Code, "");
        let vars = build_worker_env_vars(&config, "secret", &test_network_config(), 12322);
        assert!(
            !vars.iter().any(|(k, _)| k == "UR_WORKER_MODEL"),
            "empty model should NOT inject UR_WORKER_MODEL; got {vars:?}"
        );
    }

    #[test]
    fn build_worker_env_vars_gitea_forge_sets_env_and_appends_skill() {
        let mut config = test_worker_config(WorkerStrategy::Code, "sonnet");
        config.skills = vec!["green".into()];
        config.forge = Some(ur_config::ForgeType::Gitea);
        let vars = build_worker_env_vars(&config, "secret", &test_network_config(), 12322);
        assert!(
            vars.contains(&(ur_config::UR_WORKER_FORGE_ENV.into(), "gitea".into())),
            "Gitea forge should inject UR_WORKER_FORGE=gitea; got {vars:?}"
        );
        assert!(
            vars.contains(&("UR_WORKER_SKILLS".into(), "green,gitea".into())),
            "Gitea forge should append gitea to skills; got {vars:?}"
        );
    }

    #[test]
    fn build_worker_env_vars_gitea_forge_does_not_duplicate_skill() {
        let mut config = test_worker_config(WorkerStrategy::Code, "sonnet");
        config.skills = vec!["green".into(), "gitea".into()];
        config.forge = Some(ur_config::ForgeType::Gitea);
        let vars = build_worker_env_vars(&config, "secret", &test_network_config(), 12322);
        assert!(
            vars.contains(&("UR_WORKER_SKILLS".into(), "green,gitea".into())),
            "Gitea forge should not duplicate gitea in skills; got {vars:?}"
        );
    }

    #[test]
    fn build_worker_env_vars_gitea_forge_with_empty_skills_adds_gitea() {
        let mut config = test_worker_config(WorkerStrategy::Code, "sonnet");
        config.skills = vec![];
        config.forge = Some(ur_config::ForgeType::Gitea);
        let vars = build_worker_env_vars(&config, "secret", &test_network_config(), 12322);
        assert!(
            vars.contains(&("UR_WORKER_SKILLS".into(), "gitea".into())),
            "Gitea forge with empty skills should set UR_WORKER_SKILLS=gitea; got {vars:?}"
        );
    }

    #[test]
    fn build_worker_env_vars_github_forge_sets_env_without_gitea_skill() {
        let mut config = test_worker_config(WorkerStrategy::Code, "sonnet");
        config.skills = vec!["green".into()];
        config.forge = Some(ur_config::ForgeType::GitHub);
        let vars = build_worker_env_vars(&config, "secret", &test_network_config(), 12322);
        assert!(
            vars.contains(&(ur_config::UR_WORKER_FORGE_ENV.into(), "github".into())),
            "GitHub forge should inject UR_WORKER_FORGE=github; got {vars:?}"
        );
        assert!(
            vars.contains(&("UR_WORKER_SKILLS".into(), "green".into())),
            "GitHub forge should NOT append gitea to skills; got {vars:?}"
        );
    }

    #[test]
    fn build_worker_env_vars_none_forge_omits_forge_env() {
        let mut config = test_worker_config(WorkerStrategy::Code, "sonnet");
        config.forge = None;
        let vars = build_worker_env_vars(&config, "secret", &test_network_config(), 12322);
        assert!(
            !vars
                .iter()
                .any(|(k, _)| k == ur_config::UR_WORKER_FORGE_ENV),
            "None forge should NOT inject UR_WORKER_FORGE; got {vars:?}"
        );
    }

    #[test]
    fn worker_modes_default_has_code_and_design() {
        let cfg = WorkerModesConfig::default();
        let code = cfg.resolve_skills("", &[]).unwrap();
        assert!(code.contains(&"green".to_string()));
        let design = cfg.resolve_skills("design", &[]).unwrap();
        assert!(design.contains(&"design".to_string()));
    }

    #[test]
    fn worker_modes_explicit_skills_override() {
        let cfg = WorkerModesConfig::default();
        let skills = vec!["custom-skill".to_string()];
        let resolved = cfg.resolve_skills("code", &skills).unwrap();
        assert_eq!(resolved, vec!["custom-skill"]);
    }

    #[test]
    fn worker_modes_unknown_mode_errors() {
        let cfg = WorkerModesConfig::default();
        let result = cfg.resolve_skills("nonexistent", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown worker mode"));
    }

    #[test]
    fn worker_modes_from_toml_overrides_defaults() {
        let toml = r#"
[worker_modes.code]
base = "code"
skills = ["only-one"]

[worker_modes.custom]
base = "design"
skills = ["a", "b"]
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let code = cfg.resolve_skills("code", &[]).unwrap();
        assert_eq!(code, vec!["only-one"]);
        let custom = cfg.resolve_skills("custom", &[]).unwrap();
        assert_eq!(custom, vec!["a", "b"]);
        // design default should still be present
        let design = cfg.resolve_skills("design", &[]).unwrap();
        assert!(design.contains(&"design".to_string()));
    }

    #[test]
    fn worker_modes_from_toml_no_section_uses_defaults() {
        let toml = "server_port = 5000\n";
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let code = cfg.resolve_skills("", &[]).unwrap();
        assert!(code.contains(&"green".to_string()));
    }

    #[test]
    fn resolve_mode_default_returns_code_strategy() {
        let cfg = WorkerModesConfig::default();
        let ResolvedMode {
            strategy,
            skills,
            model,
            ..
        } = cfg.resolve_mode("", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Code);
        assert!(skills.contains(&"implement".to_string()));
        assert_eq!(model, "sonnet");
    }

    #[test]
    fn resolve_mode_design_returns_design_strategy() {
        let cfg = WorkerModesConfig::default();
        let ResolvedMode {
            strategy,
            skills,
            model,
            ..
        } = cfg.resolve_mode("design", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Design);
        assert!(skills.contains(&"design".to_string()));
        assert_eq!(model, "opus");
    }

    #[test]
    fn resolve_mode_custom_inherits_base_strategy() {
        let toml = r#"
[worker_modes.my-docs]
base = "design"
skills = ["tickets", "my-custom-skill"]
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            strategy,
            skills,
            model,
            ..
        } = cfg.resolve_mode("my-docs", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Design);
        assert_eq!(skills, vec!["tickets", "my-custom-skill"]);
        assert_eq!(model, "opus");
    }

    #[test]
    fn resolve_mode_unknown_errors() {
        let cfg = WorkerModesConfig::default();
        let result = cfg.resolve_mode("nonexistent", None);
        assert!(result.is_err());
    }

    #[test]
    fn from_toml_rejects_invalid_base() {
        let toml = r#"
[worker_modes.bad]
base = "invalid"
skills = ["tickets"]
"#;
        let result = WorkerModesConfig::from_toml(toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid base"));
    }

    #[test]
    fn worker_modes_default_has_manual() {
        let cfg = WorkerModesConfig::default();
        let manual = cfg.resolve_skills("manual", &[]).unwrap();
        assert!(manual.contains(&"implement".to_string()));
        assert!(manual.contains(&"design".to_string()));
        assert!(manual.contains(&"green".to_string()));
    }

    #[test]
    fn resolve_mode_manual_returns_manual_strategy() {
        let cfg = WorkerModesConfig::default();
        let ResolvedMode {
            strategy,
            skills,
            model,
            ..
        } = cfg.resolve_mode("manual", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Manual);
        // All skills — union of code and design plus common
        assert!(skills.contains(&"implement".to_string()));
        assert!(skills.contains(&"design".to_string()));
        assert!(skills.contains(&"green".to_string()));
        assert_eq!(model, "opus");
    }

    #[test]
    fn resolve_mode_manual_default_model() {
        let cfg = WorkerModesConfig::default();
        let ResolvedMode { model, .. } = cfg.resolve_mode("manual", None).unwrap();
        assert_eq!(model, "opus");
    }

    #[test]
    fn from_toml_accepts_manual_as_base() {
        let toml = r#"
[worker_modes.my-manual]
base = "manual"
skills = ["implement", "design", "custom-skill"]
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            strategy,
            skills,
            model,
            ..
        } = cfg.resolve_mode("my-manual", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Manual);
        assert_eq!(skills, vec!["implement", "design", "custom-skill"]);
        assert_eq!(model, "opus");
    }

    #[test]
    fn from_toml_manual_base_with_model_override() {
        let toml = r#"
[worker_modes.haiku-manual]
base = "manual"
skills = ["implement"]
model = "haiku"
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            strategy, model, ..
        } = cfg.resolve_mode("haiku-manual", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Manual);
        assert_eq!(model, "haiku");
    }

    #[test]
    fn resolve_mode_manual_in_no_section_toml_works() {
        let toml = "server_port = 5000\n";
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            strategy,
            skills,
            model,
            ..
        } = cfg.resolve_mode("manual", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Manual);
        assert!(skills.contains(&"implement".to_string()));
        assert_eq!(model, "opus");
    }

    #[test]
    fn from_toml_overrides_builtin_strategy() {
        let toml = r#"
[worker_modes.code]
base = "code"
skills = ["only-one"]
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            strategy,
            skills,
            model,
            ..
        } = cfg.resolve_mode("code", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Code);
        assert_eq!(skills, vec!["only-one"]);
        // No explicit model override; inherits code's default ("sonnet").
        assert_eq!(model, "sonnet");
    }

    #[test]
    fn resolve_mode_code_default_model_is_sonnet() {
        let cfg = WorkerModesConfig::default();
        let ResolvedMode { model, .. } = cfg.resolve_mode("code", None).unwrap();
        assert_eq!(model, "sonnet");
    }

    #[test]
    fn resolve_mode_design_default_model() {
        let cfg = WorkerModesConfig::default();
        let ResolvedMode { model, .. } = cfg.resolve_mode("design", None).unwrap();
        assert_eq!(model, "opus");
    }

    #[test]
    fn resolve_mode_code_explicit_model_overrides_default() {
        let toml = r#"
[worker_modes.code]
base = "code"
skills = ["only-one"]
model = "opus"
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            strategy, model, ..
        } = cfg.resolve_mode("code", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Code);
        assert_eq!(model, "opus");
    }

    #[test]
    fn resolve_mode_custom_inherits_design_model() {
        let toml = r#"
[worker_modes.x]
base = "design"
skills = ["tickets"]
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            strategy, model, ..
        } = cfg.resolve_mode("x", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Design);
        assert_eq!(model, "opus");
    }

    #[test]
    fn resolve_mode_custom_explicit_haiku_model() {
        let toml = r#"
[worker_modes.x]
base = "design"
skills = ["tickets"]
model = "haiku"
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode { model, .. } = cfg.resolve_mode("x", None).unwrap();
        assert_eq!(model, "haiku");
    }

    #[test]
    fn worker_models_override_applies_to_builtin_modes() {
        let toml = r#"
[worker_models.claude]
code   = { model = "opus" }
design = { model = "haiku" }
manual = { model = "haiku" }
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            model: code_model, ..
        } = cfg.resolve_mode("code", None).unwrap();
        let ResolvedMode {
            model: design_model,
            ..
        } = cfg.resolve_mode("design", None).unwrap();
        let ResolvedMode {
            model: manual_model,
            ..
        } = cfg.resolve_mode("manual", None).unwrap();
        assert_eq!(code_model, "opus");
        assert_eq!(design_model, "haiku");
        assert_eq!(manual_model, "haiku");
    }

    #[test]
    fn worker_models_resolve_model_and_effort_independently() {
        let toml = r#"
[worker_models.claude]
code = { effort = "high" }
design = { model = "haiku" }
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();

        let ResolvedMode { model, effort, .. } = cfg.resolve_mode("code", None).unwrap();
        assert_eq!(model, "sonnet");
        assert_eq!(effort, "high");

        let ResolvedMode { model, effort, .. } = cfg.resolve_mode("design", None).unwrap();
        assert_eq!(model, "haiku");
        assert_eq!(effort, "medium");
    }

    #[test]
    fn worker_models_omitted_key_falls_back_to_default_model() {
        let toml = r#"
[worker_models.claude]
code = { model = "opus" }
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            model: code_model, ..
        } = cfg.resolve_mode("code", None).unwrap();
        let ResolvedMode {
            model: design_model,
            ..
        } = cfg.resolve_mode("design", None).unwrap();
        let ResolvedMode {
            model: manual_model,
            ..
        } = cfg.resolve_mode("manual", None).unwrap();
        assert_eq!(code_model, "opus");
        assert_eq!(
            design_model,
            ur_config::AgentType::Claude
                .default_model("design")
                .unwrap()
        );
        assert_eq!(
            manual_model,
            ur_config::AgentType::Claude
                .default_model("manual")
                .unwrap()
        );
    }

    #[test]
    fn worker_models_unknown_key_errors_naming_bad_key() {
        let toml = r#"
[worker_models.claude]
bogus-strategy = { model = "opus" }
"#;
        let result = WorkerModesConfig::from_toml(toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("bogus-strategy"));
    }

    #[test]
    fn worker_models_flat_entry_is_a_migration_error() {
        let err = WorkerModesConfig::from_toml("[worker_models]\ncode = \"sonnet\"\n").unwrap_err();
        assert!(err.contains("code"), "{err}");
        assert!(err.contains("agent table"), "{err}");
    }

    #[test]
    fn worker_models_rejects_unknown_effort_for_agent() {
        let err =
            WorkerModesConfig::from_toml("[worker_models.claude]\ncode = { effort = \"ultra\" }\n")
                .unwrap_err();
        assert!(err.contains("claude"), "{err}");
        assert!(err.contains("ultra"), "{err}");
        assert!(err.contains("xhigh"), "{err}");
    }

    #[test]
    fn worker_models_custom_mode_inherits_override() {
        let toml = r#"
[worker_models.claude]
design = { model = "haiku" }

[worker_modes.my-docs]
base = "design"
skills = ["tickets"]
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode {
            strategy, model, ..
        } = cfg.resolve_mode("my-docs", None).unwrap();
        assert_eq!(strategy, WorkerStrategy::Design);
        assert_eq!(model, "haiku");
    }

    #[test]
    fn worker_models_custom_mode_explicit_model_overrides_worker_models() {
        let toml = r#"
[worker_models.claude]
design = { model = "haiku" }

[worker_modes.my-docs]
base = "design"
skills = ["tickets"]
model = "opus"
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode { model, .. } = cfg.resolve_mode("my-docs", None).unwrap();
        assert_eq!(model, "opus");
    }

    #[test]
    fn worker_models_no_section_default_unaffected() {
        let cfg = WorkerModesConfig::default();
        let ResolvedMode {
            model: code_model, ..
        } = cfg.resolve_mode("code", None).unwrap();
        let ResolvedMode {
            model: design_model,
            ..
        } = cfg.resolve_mode("design", None).unwrap();
        let ResolvedMode {
            model: manual_model,
            ..
        } = cfg.resolve_mode("manual", None).unwrap();
        assert_eq!(
            code_model,
            ur_config::AgentType::Claude.default_model("code").unwrap()
        );
        assert_eq!(
            design_model,
            ur_config::AgentType::Claude
                .default_model("design")
                .unwrap()
        );
        assert_eq!(
            manual_model,
            ur_config::AgentType::Claude
                .default_model("manual")
                .unwrap()
        );
    }

    /// Every `WorkerStrategy` has a built-in default model for every agent that
    /// has a model concept at all. This is the invariant `effective_default_model`
    /// relies on instead of panicking: adding a `WorkerStrategy` variant without
    /// extending `AgentType::default_model` fails here, not at worker launch.
    ///
    /// An agent with no model concept returns `None` for every strategy — that
    /// is legitimate, and such an agent is skipped rather than failed.
    #[test]
    fn every_strategy_has_a_default_model() {
        for agent in ur_config::AgentType::ALL {
            let has_models = WorkerStrategy::ALL
                .iter()
                .any(|s| agent.default_model(s.name()).is_some());
            if !has_models {
                continue; // agent with no model concept
            }
            for strategy in WorkerStrategy::ALL {
                assert!(
                    agent.default_model(strategy.name()).is_some(),
                    "agent '{}' has default models but none for strategy '{}' — \
                     add it to AgentType::default_model",
                    agent.name(),
                    strategy.name(),
                );
            }
        }
    }

    /// mode `model` beats `[worker_models.<agent>]` beats `agent.default_model(strategy)`,
    /// across all three strategies.
    #[test]
    fn model_resolution_precedence() {
        for (base, worker_models_key) in
            [("code", "code"), ("design", "design"), ("manual", "manual")]
        {
            // Bottom rung: no overrides at all.
            let cfg = WorkerModesConfig::default();
            let ResolvedMode { model, .. } = cfg.resolve_mode(base, None).unwrap();
            assert_eq!(
                model,
                ur_config::AgentType::Claude.default_model(base).unwrap(),
                "bottom rung for {base}"
            );

            // Middle rung: the agent's worker_models override beats the built-in default.
            let toml = format!(
                "[worker_models.claude]\n{worker_models_key} = {{ model = \"worker-models-override\" }}\n"
            );
            let cfg = WorkerModesConfig::from_toml(&toml).unwrap();
            let ResolvedMode { model, .. } = cfg.resolve_mode(base, None).unwrap();
            assert_eq!(model, "worker-models-override", "middle rung for {base}");

            // Top rung: an explicit mode `model` beats both [worker_models] and the default.
            let toml = format!(
                "[worker_models.claude]\n{worker_models_key} = {{ model = \"worker-models-override\" }}\n\n\
                 [worker_modes.custom-{base}]\nbase = \"{base}\"\nskills = []\nmodel = \"mode-override\"\n"
            );
            let cfg = WorkerModesConfig::from_toml(&toml).unwrap();
            let ResolvedMode { model, .. } =
                cfg.resolve_mode(&format!("custom-{base}"), None).unwrap();
            assert_eq!(model, "mode-override", "top rung for {base}");
        }
    }

    /// Model and effort each resolve independently from mode override to an
    /// agent-scoped strategy entry, then to the agent's built-in default.
    #[test]
    fn model_resolution_precedence_per_agent() {
        for agent in ur_config::AgentType::ALL {
            let name = agent.name();

            // Bottom rung: no overrides at all.
            let toml = format!("agent = \"{name}\"\n");
            let cfg = WorkerModesConfig::from_toml(&toml).unwrap();
            let ResolvedMode { model, .. } = cfg.resolve_mode("code", None).unwrap();
            assert_eq!(
                model,
                agent.default_model("code").unwrap(),
                "bottom rung for {name}"
            );

            // Second rung: the agent's worker_models table beats the built-in default.
            let toml = format!(
                "agent = \"{name}\"\n[worker_models.{name}]\ncode = {{ model = \"agent-override\" }}\n"
            );
            let cfg = WorkerModesConfig::from_toml(&toml).unwrap();
            let ResolvedMode { model, .. } = cfg.resolve_mode("code", None).unwrap();
            assert_eq!(model, "agent-override", "agent rung for {name}");

            // Top rung: an explicit mode `model` beats everything else.
            let toml = format!(
                "agent = \"{name}\"\n\
                 [worker_models.{name}]\ncode = {{ model = \"agent-override\" }}\n\n\
                 [worker_modes.custom]\nbase = \"code\"\nskills = []\nmodel = \"mode-override\"\n"
            );
            let cfg = WorkerModesConfig::from_toml(&toml).unwrap();
            let ResolvedMode { model, .. } = cfg.resolve_mode("custom", None).unwrap();
            assert_eq!(model, "mode-override", "top rung for {name}");
        }
    }

    /// A per-agent table wins only for that agent — it must not leak into a
    /// launch of the other agent, which is exactly the bug this precedence
    /// scheme exists to prevent.
    #[test]
    fn worker_models_per_agent_table_does_not_leak_to_other_agent() {
        let toml = r#"
[worker_models.codex]
code = { model = "gpt-5.6-terra-custom" }
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();

        let ResolvedMode { model, .. } = cfg
            .resolve_mode("code", Some(ur_config::AgentType::Codex))
            .unwrap();
        assert_eq!(model, "gpt-5.6-terra-custom");

        let ResolvedMode { model, .. } = cfg
            .resolve_mode("code", Some(ur_config::AgentType::Claude))
            .unwrap();
        assert_eq!(
            model,
            ur_config::AgentType::Claude.default_model("code").unwrap()
        );
    }

    #[test]
    fn agy_worker_models_apply_without_leaking() {
        let toml = r#"
[worker_models.agy]
code = { model = "gemini-custom", effort = "high" }
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();

        let ResolvedMode {
            agent,
            model,
            effort,
            ..
        } = cfg
            .resolve_mode("code", Some(ur_config::AgentType::Agy))
            .unwrap();
        assert_eq!(agent, ur_config::AgentType::Agy);
        assert_eq!(model, "gemini-custom");
        assert_eq!(effort, "high");

        let ResolvedMode { model, effort, .. } = cfg
            .resolve_mode("code", Some(ur_config::AgentType::Claude))
            .unwrap();
        assert_eq!(model, "sonnet");
        assert_eq!(effort, "medium");
    }

    /// With no `[worker_models]` at all, every agent falls straight through
    /// to `agent.default_model(strategy)`.
    #[test]
    fn no_worker_models_section_every_agent_gets_its_own_default() {
        for agent in ur_config::AgentType::ALL {
            let cfg = WorkerModesConfig::from_toml("").unwrap();
            for strategy in ["code", "design", "manual"] {
                let ResolvedMode { model, .. } = cfg.resolve_mode(strategy, Some(*agent)).unwrap();
                assert_eq!(
                    model,
                    agent.default_model(strategy).unwrap(),
                    "{agent:?}/{strategy}"
                );
            }
        }
    }

    #[test]
    fn worker_models_unknown_agent_table_name_errors_naming_valid_agents() {
        let toml = r#"
[worker_models.codexx]
code = { model = "opus" }
"#;
        let err = WorkerModesConfig::from_toml(toml).unwrap_err();
        assert!(err.contains("codexx"), "{err}");
        assert!(err.contains("claude"), "{err}");
        assert!(err.contains("codex"), "{err}");
        assert!(err.contains("agy"), "{err}");
    }

    #[test]
    fn worker_models_per_agent_table_unknown_strategy_key_errors() {
        let toml = r#"
[worker_models.codex]
bogus-strategy = { model = "opus" }
"#;
        let err = WorkerModesConfig::from_toml(toml).unwrap_err();
        assert!(err.contains("bogus-strategy"), "{err}");
    }

    /// Agent resolution order: explicit `resolve_mode` param → mode's `agent`
    /// field → claude.
    #[test]
    fn mode_agent_resolution() {
        // (a) mode config has no `agent` field → claude.
        let toml = r#"
[worker_modes.no-agent]
base = "code"
skills = []
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode { agent, .. } = cfg.resolve_mode("no-agent", None).unwrap();
        assert_eq!(agent, ur_config::AgentType::Claude);

        // (b) mode config has `agent = "claude"` → claude.
        let toml = r#"
[worker_modes.explicit-claude]
base = "code"
skills = []
agent = "claude"
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode { agent, .. } = cfg.resolve_mode("explicit-claude", None).unwrap();
        assert_eq!(agent, ur_config::AgentType::Claude);

        // (c) explicit override param wins over the mode's own agent field.
        let ResolvedMode { agent, .. } = cfg
            .resolve_mode("explicit-claude", Some(ur_config::AgentType::Claude))
            .unwrap();
        assert_eq!(agent, ur_config::AgentType::Claude);
    }

    #[test]
    fn from_toml_rejects_unknown_agent() {
        let toml = r#"
[worker_modes.bad-agent]
base = "code"
skills = []
agent = "bogus"
"#;
        let result = WorkerModesConfig::from_toml(toml);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("bad-agent"), "{msg}");
        assert!(msg.contains("bogus"), "{msg}");
    }

    /// The top-level `agent` key becomes the floor for every built-in mode,
    /// and default models follow the agent — codex's `code` mode gets
    /// `gpt-5.6-terra`, not `sonnet`.
    #[test]
    fn top_level_agent_default_applies_to_builtin_modes() {
        let toml = "agent = \"codex\"\n";
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();

        for mode in ["code", "design", "manual"] {
            let ResolvedMode { agent, .. } = cfg.resolve_mode(mode, None).unwrap();
            assert_eq!(agent, ur_config::AgentType::Codex, "mode {mode}");
        }

        let ResolvedMode { model, .. } = cfg.resolve_mode("code", None).unwrap();
        assert_eq!(model, "gpt-5.6-terra");
    }

    /// `--runtime` → `worker_modes.<mode>.runtime` → project → top-level → container.
    #[test]
    fn runtime_precedence_per_level() {
        use ur_config::WorkerRuntime::{Container, Local};
        let toml = r#"
runtime = "local"

[worker_modes.boxed]
base = "code"
skills = []
runtime = "container"
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        // CLI wins over the mode.
        assert_eq!(cfg.resolve_runtime("boxed", Some(Local), None), Local);
        // Mode wins over the project and top level.
        assert_eq!(cfg.resolve_runtime("boxed", None, Some(Local)), Container);
        // Project wins over top level (built-in mode has no runtime field).
        assert_eq!(
            cfg.resolve_runtime("code", None, Some(Container)),
            Container
        );
        // Top level applies when nothing else is set; empty mode = code.
        assert_eq!(cfg.resolve_runtime("", None, None), Local);
        // Default with no runtime keys anywhere is container.
        let bare = WorkerModesConfig::from_toml("").unwrap();
        assert_eq!(bare.resolve_runtime("code", None, None), Container);
        assert_eq!(
            WorkerModesConfig::default().resolve_runtime("code", None, None),
            Container
        );
    }

    #[test]
    fn runtime_rejects_unknown_values_naming_the_key() {
        let msg = WorkerModesConfig::from_toml("runtime = \"vm\"\n").unwrap_err();
        assert!(msg.contains("runtime") && msg.contains("vm"), "{msg}");
        let toml = r#"
[worker_modes.bad-runtime]
base = "code"
skills = []
runtime = "vm"
"#;
        let msg = WorkerModesConfig::from_toml(toml).unwrap_err();
        assert!(msg.contains("runtime") && msg.contains("vm"), "{msg}");
    }

    #[test]
    fn agy_agent_precedence() {
        let toml = r#"
agent = "agy"

[worker_modes.claude-mode]
base = "code"
skills = []
agent = "claude"
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();

        let ResolvedMode { agent, model, .. } = cfg.resolve_mode("code", None).unwrap();
        assert_eq!(agent, ur_config::AgentType::Agy);
        assert_eq!(model, "gemini-3.8-flash");

        let ResolvedMode { agent, .. } = cfg.resolve_mode("claude-mode", None).unwrap();
        assert_eq!(agent, ur_config::AgentType::Claude);

        let ResolvedMode { agent, .. } = cfg
            .resolve_mode("claude-mode", Some(ur_config::AgentType::Agy))
            .unwrap();
        assert_eq!(agent, ur_config::AgentType::Agy);
    }

    /// A complete, realistic `ur.toml` (server/network/db tables, custom modes,
    /// `[worker_models]`, projects, `[skills]`) with a top-level `agent = "codex"`
    /// resolves every launch with no `--agent` flag and no mode `agent` field to
    /// codex — exactly what the server's `load_worker_modes` feeds this parser.
    #[test]
    fn top_level_agent_default_in_full_config_resolves_every_mode() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("skills/test-skill")).unwrap();
        let toml = r#"
server_port = 19870
workspace = "/tmp/ur-test/workspace"
compose_file = "/tmp/ur-test/docker-compose.yml"
workspace_brain_dir = "/tmp/ur-test/brain"
agent = "codex"

[proxy]
hostname = "ur-test-squid"

[network]
name = "ur-test"
worker_name = "ur-test-workers"
server_hostname = "ur-test-server"
worker_prefix = "ur-test-worker-"

[ticket_db]
host = "ur-test-postgres"

[workflow_db]
host = "ur-test-postgres"

[worker_modes.custommodel]
base = "design"
skills = ["implement"]
model = "my-custom-model"

[worker_models.claude]
manual = { model = "sonnet" }

[projects.agentdefaultproj]
repo = "git@github.com:test/agentdefaultproj.git"
[projects.agentdefaultproj.container]
image = "ur-worker"

[skills.code]
test-skill = "/tmp/ur-test/skills/test-skill"
"#
        .replace("/tmp/ur-test", &tmp.path().display().to_string());
        ur_config::Config::from_toml_str(&toml, tmp.path())
            .expect("fixture must be a valid ur.toml");
        let cfg = WorkerModesConfig::from_toml(&toml).unwrap();

        for mode in ["code", "design", "manual", "custommodel"] {
            let ResolvedMode { agent, .. } = cfg.resolve_mode(mode, None).unwrap();
            assert_eq!(agent, ur_config::AgentType::Codex, "mode {mode}");
        }
        // `[worker_models.claude]` must not leak into the codex default.
        let ResolvedMode { model, .. } = cfg.resolve_mode("manual", None).unwrap();
        assert_ne!(model, "sonnet");
        let ResolvedMode { model, .. } = cfg.resolve_mode("custommodel", None).unwrap();
        assert_eq!(model, "my-custom-model");
    }

    /// An omitted top-level `agent` key still defaults every built-in mode to claude.
    #[test]
    fn top_level_agent_default_omitted_is_claude() {
        let cfg = WorkerModesConfig::from_toml("").unwrap();
        for mode in ["code", "design", "manual"] {
            let ResolvedMode { agent, .. } = cfg.resolve_mode(mode, None).unwrap();
            assert_eq!(agent, ur_config::AgentType::Claude, "mode {mode}");
        }
    }

    /// A mode's own `agent` field still overrides the top-level default.
    #[test]
    fn mode_agent_field_overrides_top_level_default() {
        let toml = r#"
agent = "codex"

[worker_modes.claude-only]
base = "code"
skills = []
agent = "claude"
"#;
        let cfg = WorkerModesConfig::from_toml(toml).unwrap();
        let ResolvedMode { agent, .. } = cfg.resolve_mode("claude-only", None).unwrap();
        assert_eq!(agent, ur_config::AgentType::Claude);

        // A mode with no `agent` field of its own still picks up the top-level default.
        let ResolvedMode { agent, .. } = cfg.resolve_mode("code", None).unwrap();
        assert_eq!(agent, ur_config::AgentType::Codex);
    }

    #[test]
    fn from_toml_rejects_unknown_top_level_agent() {
        let toml = "agent = \"bogus\"\n";
        let result = WorkerModesConfig::from_toml(toml);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("bogus"), "{msg}");
        assert!(msg.contains("claude"), "{msg}");
        assert!(msg.contains("codex"), "{msg}");
    }

    #[tokio::test]
    async fn verify_worker_valid_pair_returns_true() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let wid = WorkerId("test-ab12".into());
        let secret = "my-secret-token";
        mgr.register_worker(
            wid.clone(),
            "test".into(),
            "proj".into(),
            Some(PathBuf::from("/tmp/slot")),
            WorkerStrategy::Code,
            "cid".into(),
            secret.into(),
        )
        .await;
        assert!(mgr.verify_worker("test-ab12", secret).await);
    }

    #[tokio::test]
    async fn verify_worker_wrong_secret_returns_false() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let wid = WorkerId("test-ab12".into());
        mgr.register_worker(
            wid.clone(),
            "test".into(),
            String::new(),
            None,
            WorkerStrategy::Code,
            "cid".into(),
            "correct-secret".into(),
        )
        .await;
        assert!(!mgr.verify_worker("test-ab12", "wrong-secret").await);
    }

    #[tokio::test]
    async fn verify_worker_unknown_id_returns_false() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        assert!(!mgr.verify_worker("unknown-ab12", "any-secret").await);
    }

    #[tokio::test]
    async fn verify_worker_invalid_id_format_returns_false() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        assert!(!mgr.verify_worker("nodash", "any-secret").await);
    }

    #[tokio::test]
    async fn get_worker_context_returns_context_for_registered_agent() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let wid = WorkerId("ctx-ab12".into());
        let slot = PathBuf::from("/tmp/slot");
        mgr.register_worker(
            wid.clone(),
            "ctx".into(),
            "myproject".into(),
            Some(slot.clone()),
            WorkerStrategy::Code,
            "cid".into(),
            "secret".into(),
        )
        .await;
        let ctx = mgr.get_worker_context(&wid).await.unwrap();
        assert_eq!(ctx.project_key, Some("myproject".to_string()));
        assert_eq!(ctx.slot_path, slot);
        assert_eq!(ctx.strategy, "code");
    }

    #[tokio::test]
    async fn get_worker_context_strategy_reflects_registered_strategy() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let wid = WorkerId("man-ab12".into());
        let slot = PathBuf::from("/tmp/manual-slot");
        mgr.register_worker(
            wid.clone(),
            "man".into(),
            "proj".into(),
            Some(slot.clone()),
            WorkerStrategy::Manual,
            "cid".into(),
            "secret".into(),
        )
        .await;
        let ctx = mgr.get_worker_context(&wid).await.unwrap();
        assert_eq!(ctx.strategy, "manual");
    }

    #[tokio::test]
    async fn get_worker_context_empty_project_key_maps_to_none() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let wid = WorkerId("ws-ab12".into());
        mgr.register_worker(
            wid.clone(),
            "ws".into(),
            String::new(),
            Some(PathBuf::from("/tmp/ws")),
            WorkerStrategy::Code,
            "cid".into(),
            "secret".into(),
        )
        .await;
        let ctx = mgr.get_worker_context(&wid).await.unwrap();
        assert_eq!(ctx.project_key, None);
    }

    #[tokio::test]
    async fn get_worker_context_returns_none_for_unknown_agent() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let wid = WorkerId("missing-ab12".into());
        assert!(mgr.get_worker_context(&wid).await.is_none());
    }

    #[tokio::test]
    async fn get_worker_context_returns_none_when_no_slot_path() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let wid = WorkerId("nosl-ab12".into());
        mgr.register_worker(
            wid.clone(),
            "nosl".into(),
            "proj".into(),
            None,
            WorkerStrategy::Code,
            "cid".into(),
            "secret".into(),
        )
        .await;
        assert!(mgr.get_worker_context(&wid).await.is_none());
    }

    #[test]
    fn resolve_project_instruction_returns_explicit_value() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_project_instruction(
            &Some("%PROJECT%/CLAUDE.md".into()),
            "myproj",
            tmp.path(),
            ur_config::AgentType::Claude,
        );
        assert_eq!(result.as_deref(), Some("%PROJECT%/CLAUDE.md"));
    }

    #[test]
    fn resolve_project_instruction_none_empty_project_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result =
            resolve_project_instruction(&None, "", tmp.path(), ur_config::AgentType::Claude);
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_project_instruction_convention_fallback_when_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let proj_dir = tmp.path().join("projects").join("myproj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("CLAUDE.md"), "# Project").unwrap();

        let result =
            resolve_project_instruction(&None, "myproj", tmp.path(), ur_config::AgentType::Claude);
        let expected = proj_dir.join("CLAUDE.md").to_string_lossy().into_owned();
        assert_eq!(result.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn resolve_project_instruction_convention_fallback_no_file_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result =
            resolve_project_instruction(&None, "myproj", tmp.path(), ur_config::AgentType::Claude);
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_memory_dir_returns_explicit_value() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_memory_dir(&Some("/custom/memory".into()), "myproj", tmp.path());
        assert_eq!(result.as_deref(), Some("/custom/memory"));
    }

    #[test]
    fn resolve_memory_dir_none_empty_project_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_memory_dir(&None, "", tmp.path());
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_memory_dir_convention_fallback_when_dir_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("projects").join("myproj").join("memory");
        std::fs::create_dir_all(&memory_dir).unwrap();

        let result = resolve_memory_dir(&None, "myproj", tmp.path());
        let expected = memory_dir.to_string_lossy().into_owned();
        assert_eq!(result.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn resolve_memory_dir_convention_fallback_no_dir_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_memory_dir(&None, "myproj", tmp.path());
        assert_eq!(result, None);
    }

    #[test]
    fn brain_host_path_resolves_like_the_mount() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let mut config = test_worker_config(WorkerStrategy::Manual, "");
        config.project_key = "proj".into();

        // No brain_dir and no convention dir on disk: no mount, no path.
        assert_eq!(brain_host_path(&config, dir), None);

        // Convention dir exists: that path.
        let convention = dir.join("projects").join("proj").join("brain");
        std::fs::create_dir_all(&convention).unwrap();
        assert_eq!(
            brain_host_path(&config, dir),
            Some(convention.display().to_string())
        );

        // %URCONFIG% template wins and resolves against the host config dir.
        config.brain_dir = Some("%URCONFIG%/brains/proj".into());
        assert_eq!(
            brain_host_path(&config, dir),
            Some(dir.join("brains").join("proj").display().to_string())
        );

        // Absolute brain_dir is used as-is.
        config.brain_dir = Some("/abs/brain".into());
        assert_eq!(brain_host_path(&config, dir), Some("/abs/brain".into()));
    }

    #[test]
    fn resolve_brain_dir_returns_explicit_value() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_brain_dir(&Some("/custom/brain".into()), &None, "myproj", tmp.path());
        assert_eq!(result.as_deref(), Some("/custom/brain"));
    }

    #[test]
    fn resolve_brain_dir_none_empty_project_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_brain_dir(&None, &None, "", tmp.path());
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_brain_dir_convention_fallback_when_dir_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let brain_dir = tmp.path().join("projects").join("myproj").join("brain");
        std::fs::create_dir_all(&brain_dir).unwrap();

        let result = resolve_brain_dir(&None, &None, "myproj", tmp.path());
        let expected = brain_dir.to_string_lossy().into_owned();
        assert_eq!(result.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn resolve_brain_dir_convention_fallback_no_dir_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_brain_dir(&None, &None, "myproj", tmp.path());
        assert_eq!(result, None);
    }

    /// A `-w` launch with no project falls back to the configured workspace brain.
    #[test]
    fn resolve_brain_dir_empty_project_uses_workspace_brain() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_brain_dir(&None, &Some("%URCONFIG%/brain".into()), "", tmp.path());
        assert_eq!(result.as_deref(), Some("%URCONFIG%/brain"));
    }

    /// The workspace brain never overrides a project's own `brain_dir`.
    #[test]
    fn resolve_brain_dir_project_brain_wins_over_workspace_brain() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_brain_dir(
            &Some("/custom/brain".into()),
            &Some("/workspace/brain".into()),
            "myproj",
            tmp.path(),
        );
        assert_eq!(result.as_deref(), Some("/custom/brain"));
    }

    /// A project worker with no brain of its own does not pick up the workspace brain —
    /// the workspace default applies to project-less launches only.
    #[test]
    fn resolve_brain_dir_workspace_brain_ignored_for_project_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_brain_dir(
            &None,
            &Some("/workspace/brain".into()),
            "myproj",
            tmp.path(),
        );
        assert_eq!(result, None);
    }

    /// Outside the server container (`container_config` is `None`), the check path is the
    /// host path unchanged — convention fallbacks stat the real host directory.
    #[test]
    fn remap_to_container_config_no_container_returns_host_path() {
        let host = std::path::Path::new("/Users/me/.ur/projects/pax/brain");
        let host_config = std::path::Path::new("/Users/me/.ur");
        let result = remap_to_container_config(host, host_config, None);
        assert_eq!(result, host);
    }

    /// Inside the server container, a host-config-relative path is rebased onto the
    /// container-visible bind mount so `.exists()` hits `/config`, not the invisible host path.
    #[test]
    fn remap_to_container_config_rebases_onto_bind_mount() {
        let host = std::path::Path::new("/Users/me/.ur/projects/pax/brain");
        let host_config = std::path::Path::new("/Users/me/.ur");
        let container = std::path::Path::new("/config");
        let result = remap_to_container_config(host, host_config, Some(container));
        assert_eq!(result, std::path::Path::new("/config/projects/pax/brain"));
    }

    /// A path outside `host_config_dir` (e.g. an explicit memory_dir under `~/.claude`) is
    /// left unchanged even inside the container — it is not a convention path under config.
    #[test]
    fn remap_to_container_config_unrelated_path_unchanged() {
        let host = std::path::Path::new("/Users/me/.claude/projects/foo/memory");
        let host_config = std::path::Path::new("/Users/me/.ur");
        let container = std::path::Path::new("/config");
        let result = remap_to_container_config(host, host_config, Some(container));
        assert_eq!(result, host);
    }

    #[tokio::test]
    async fn stop_by_worker_id_unknown_returns_error() {
        let (mgr, _workspace, _test_db) = test_manager().await;
        let wid = WorkerId::parse("nonexistent-ab12").unwrap();
        let result = mgr.stop_by_worker_id(&wid).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown worker"));
    }

    // ---- merge_global_skills tests ----
    //
    // merge_global_skills is a pure function on self.global_skills — no DB
    // access needed. We use test_manager() just to get a valid WorkerManager,
    // then build a fresh one with custom globals via the public constructor.

    async fn test_manager_with_globals(
        globals: GlobalSkillsConfig,
    ) -> (WorkerManager, tempfile::TempDir, ur_db_test::TestDb) {
        let workspace = tempfile::tempdir().unwrap();
        let (worker_repo, test_db) = test_worker_repo().await;
        let channel =
            tonic::transport::Channel::from_static("http://localhost:12323").connect_lazy();
        let builder_container_client = BuilderContainerClient::new(channel.clone());
        let network_manager = NetworkManager::new(
            builder_container_client.clone(),
            ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
        );
        let network_config = NetworkConfig {
            name: ur_config::DEFAULT_NETWORK_NAME.into(),
            worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
            server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
            worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.into(),
        };
        let mgr = WorkerManager::new(
            workspace.path().to_path_buf(),
            workspace.path().to_path_buf(),
            workspace.path().join("logs"),
            workspace.path().join("logs"),
            workspace.path().join("logs"),
            network_manager,
            network_config,
            ur_config::DEFAULT_SERVER_PORT + 1,
            WorkerModesConfig::default(),
            worker_repo,
            globals,
            builder_container_client,
            crate::builder_local_worker_client::BuilderLocalWorkerClient::new(
                tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy(),
            ),
        );
        (mgr, workspace, test_db)
    }

    #[tokio::test]
    async fn merge_global_skills_empty_globals_identity() {
        let (mgr, _ws, _db) = test_manager_with_globals(GlobalSkillsConfig::default()).await;
        let mode_skills = vec!["implement".to_string(), "green".to_string()];
        let (merged, extra) = mgr.merge_global_skills(WorkerStrategy::Code, mode_skills.clone());
        assert_eq!(merged, mode_skills);
        assert!(extra.is_empty());
    }

    #[tokio::test]
    async fn merge_global_skills_common_only_for_code() {
        let host = std::path::PathBuf::from("/host/skills/shared");
        let globals = GlobalSkillsConfig {
            common: vec![ur_config::GlobalSkill {
                name: "shared".into(),
                host_path: host.clone(),
            }],
            code: vec![],
            design: vec![],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        let (merged, extra) =
            mgr.merge_global_skills(WorkerStrategy::Code, vec!["implement".into()]);
        assert_eq!(merged, vec!["implement", "shared"]);
        assert_eq!(extra, vec![("shared".to_string(), host)]);
    }

    #[tokio::test]
    async fn merge_global_skills_common_and_code_for_code() {
        let common_path = std::path::PathBuf::from("/host/skills/common");
        let code_path = std::path::PathBuf::from("/host/skills/internal");
        let globals = GlobalSkillsConfig {
            common: vec![ur_config::GlobalSkill {
                name: "common-skill".into(),
                host_path: common_path.clone(),
            }],
            code: vec![ur_config::GlobalSkill {
                name: "internal".into(),
                host_path: code_path.clone(),
            }],
            design: vec![ur_config::GlobalSkill {
                name: "research".into(),
                host_path: std::path::PathBuf::from("/host/skills/research"),
            }],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        let (merged, extra) =
            mgr.merge_global_skills(WorkerStrategy::Code, vec!["implement".into()]);
        assert_eq!(merged, vec!["implement", "common-skill", "internal"]);
        assert_eq!(
            extra,
            vec![
                ("common-skill".to_string(), common_path),
                ("internal".to_string(), code_path),
            ]
        );
    }

    #[tokio::test]
    async fn merge_global_skills_common_and_design_for_design() {
        let common_path = std::path::PathBuf::from("/host/skills/common");
        let design_path = std::path::PathBuf::from("/host/skills/research");
        let globals = GlobalSkillsConfig {
            common: vec![ur_config::GlobalSkill {
                name: "common-skill".into(),
                host_path: common_path.clone(),
            }],
            code: vec![ur_config::GlobalSkill {
                name: "internal".into(),
                host_path: std::path::PathBuf::from("/host/skills/internal"),
            }],
            design: vec![ur_config::GlobalSkill {
                name: "research".into(),
                host_path: design_path.clone(),
            }],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        let (merged, extra) =
            mgr.merge_global_skills(WorkerStrategy::Design, vec!["design".into()]);
        assert_eq!(merged, vec!["design", "common-skill", "research"]);
        assert_eq!(
            extra,
            vec![
                ("common-skill".to_string(), common_path),
                ("research".to_string(), design_path),
            ]
        );
    }

    #[tokio::test]
    async fn merge_global_skills_overlap_no_duplicate_mount_or_name() {
        // "implement" is in both mode_skills and globals.code — no duplicate.
        let host = std::path::PathBuf::from("/host/skills/implement");
        let globals = GlobalSkillsConfig {
            common: vec![],
            code: vec![ur_config::GlobalSkill {
                name: "implement".into(),
                host_path: host.clone(),
            }],
            design: vec![],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        let (merged, extra) = mgr.merge_global_skills(
            WorkerStrategy::Code,
            vec!["implement".into(), "green".into()],
        );
        // "implement" already present — not duplicated.
        assert_eq!(merged, vec!["implement", "green"]);
        // No extra mount because "implement" was already in mode_skills.
        assert!(extra.is_empty());
    }

    #[tokio::test]
    async fn merge_global_skills_custom_mode_uses_code_bucket() {
        // A custom mode with base = "code" should pick up code-bucket globals.
        let code_path = std::path::PathBuf::from("/host/skills/internal");
        let globals = GlobalSkillsConfig {
            common: vec![],
            code: vec![ur_config::GlobalSkill {
                name: "internal".into(),
                host_path: code_path.clone(),
            }],
            design: vec![],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        // Custom mode with Code base.
        let (merged, extra) =
            mgr.merge_global_skills(WorkerStrategy::Code, vec!["my-skill".into()]);
        assert_eq!(merged, vec!["my-skill", "internal"]);
        assert_eq!(extra, vec![("internal".to_string(), code_path)]);
    }

    #[tokio::test]
    async fn merge_global_skills_explicit_skills_override_still_receives_globals() {
        // Even when launch uses explicit --skills (overrides mode), globals are appended.
        let host = std::path::PathBuf::from("/host/skills/common");
        let globals = GlobalSkillsConfig {
            common: vec![ur_config::GlobalSkill {
                name: "common-skill".into(),
                host_path: host.clone(),
            }],
            code: vec![],
            design: vec![],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        // Simulate explicit --skills override: mode_skills = ["custom-skill"].
        let (merged, extra) =
            mgr.merge_global_skills(WorkerStrategy::Code, vec!["custom-skill".into()]);
        assert_eq!(merged, vec!["custom-skill", "common-skill"]);
        assert_eq!(extra, vec![("common-skill".to_string(), host)]);
    }

    #[tokio::test]
    async fn merge_skills_order_mode_project_globals() {
        let global_path = std::path::PathBuf::from("/host/skills/global");
        let globals = GlobalSkillsConfig {
            common: vec![ur_config::GlobalSkill {
                name: "global-skill".into(),
                host_path: global_path.clone(),
            }],
            code: vec![],
            design: vec![],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        let mode_skills = vec!["mode-a".to_string(), "mode-b".to_string()];
        let project_skills = vec!["proj-c".to_string(), "proj-d".to_string()];

        let (merged, extra) = mgr.merge_skills(WorkerStrategy::Code, mode_skills, &project_skills);
        assert_eq!(
            merged,
            vec!["mode-a", "mode-b", "proj-c", "proj-d", "global-skill"]
        );
        assert_eq!(extra, vec![("global-skill".to_string(), global_path)]);
    }

    #[tokio::test]
    async fn merge_skills_duplicates_preserve_earliest_source() {
        let global_path = std::path::PathBuf::from("/host/skills/shared");
        let globals = GlobalSkillsConfig {
            common: vec![ur_config::GlobalSkill {
                name: "shared-b".into(),
                host_path: global_path.clone(),
            }],
            code: vec![],
            design: vec![],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        // "shared-a" is in mode and project -> mode wins (earliest)
        // "shared-b" is in project and global -> project wins (earliest)
        let mode_skills = vec!["shared-a".to_string(), "mode-only".to_string()];
        let project_skills = vec!["shared-a".to_string(), "shared-b".to_string()];

        let (merged, extra) = mgr.merge_skills(WorkerStrategy::Code, mode_skills, &project_skills);
        assert_eq!(merged, vec!["shared-a", "mode-only", "shared-b"]);
        assert!(
            extra.is_empty(),
            "shared-b already in project skills -> no extra mount"
        );
    }

    #[tokio::test]
    async fn merge_skills_explicit_override_retains_project_skills() {
        let global_path = std::path::PathBuf::from("/host/skills/global");
        let globals = GlobalSkillsConfig {
            common: vec![ur_config::GlobalSkill {
                name: "global-skill".into(),
                host_path: global_path.clone(),
            }],
            code: vec![],
            design: vec![],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        let explicit_skills = vec!["explicit-override".to_string()];
        let project_skills = vec!["gitea".to_string()];

        let (merged, extra) =
            mgr.merge_skills(WorkerStrategy::Code, explicit_skills, &project_skills);
        assert_eq!(merged, vec!["explicit-override", "gitea", "global-skill"]);
        assert_eq!(extra, vec![("global-skill".to_string(), global_path)]);
    }

    #[tokio::test]
    async fn merge_skills_empty_project_skills_preserves_behavior() {
        let global_path = std::path::PathBuf::from("/host/skills/global");
        let globals = GlobalSkillsConfig {
            common: vec![ur_config::GlobalSkill {
                name: "global-skill".into(),
                host_path: global_path.clone(),
            }],
            code: vec![],
            design: vec![],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        let mode_skills = vec!["implement".to_string()];

        let (merged_explicit_empty, extra1) =
            mgr.merge_skills(WorkerStrategy::Code, mode_skills.clone(), &[]);
        let (merged_global, extra2) = mgr.merge_global_skills(WorkerStrategy::Code, mode_skills);

        assert_eq!(merged_explicit_empty, merged_global);
        assert_eq!(extra1, extra2);
    }

    #[tokio::test]
    async fn merge_skills_applies_to_all_strategies() {
        let code_path = std::path::PathBuf::from("/host/skills/code");
        let design_path = std::path::PathBuf::from("/host/skills/design");
        let globals = GlobalSkillsConfig {
            common: vec![],
            code: vec![ur_config::GlobalSkill {
                name: "code-global".into(),
                host_path: code_path.clone(),
            }],
            design: vec![ur_config::GlobalSkill {
                name: "design-global".into(),
                host_path: design_path.clone(),
            }],
        };
        let (mgr, _ws, _db) = test_manager_with_globals(globals).await;
        let project_skills = vec!["gitea".to_string()];

        // Code strategy
        let (merged_code, _) = mgr.merge_skills(
            WorkerStrategy::Code,
            vec!["code-mode".into()],
            &project_skills,
        );
        assert_eq!(merged_code, vec!["code-mode", "gitea", "code-global"]);

        // Design strategy
        let (merged_design, _) = mgr.merge_skills(
            WorkerStrategy::Design,
            vec!["design-mode".into()],
            &project_skills,
        );
        assert_eq!(merged_design, vec!["design-mode", "gitea", "design-global"]);

        // Manual strategy (receives common + code + design globals)
        let (merged_manual, _) = mgr.merge_skills(
            WorkerStrategy::Manual,
            vec!["manual-mode".into()],
            &project_skills,
        );
        assert_eq!(
            merged_manual,
            vec!["manual-mode", "gitea", "code-global", "design-global"]
        );
    }
}
