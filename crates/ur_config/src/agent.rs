use std::fmt;

use crate::UR_AGENT_TYPE_ENV;

/// Auth profile for agents using OAuth-style credentials backed by the host keychain.
/// Future agents may use other auth styles — those would become new variants if/when needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentAuth {
    pub keychain_service: &'static str,
    pub credentials_filename: &'static str,
    pub app_config_filename: &'static str,
}

/// Which AI agent runs a worker. Currently a single variant (`Claude`); future
/// agents (e.g. a network-hosted local model) add variants here.
///
/// This is the single source of truth for everything that varies per agent:
/// home subdir, instruction filename, spawn command, image name, default
/// models, and auth profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentType {
    Claude,
}

/// Error returned when parsing an unrecognized agent type string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseAgentError(pub String);

impl fmt::Display for ParseAgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown agent type: {}", self.0)
    }
}

impl std::error::Error for ParseAgentError {}

impl AgentType {
    /// All known agent variants. Lets callers with no dependency on the crate
    /// that parses `[worker_modes]` (e.g. the host CLI) iterate agents anyway.
    pub const ALL: &'static [AgentType] = &[AgentType::Claude];

    /// Short, stable name for this agent. Used as the `UR_AGENT_TYPE` value,
    /// the credential directory name, and in `agent_type` proto fields.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
        }
    }

    /// Home subdirectory (relative to the worker's home directory) holding
    /// this agent's config, skills, and instructions.
    pub fn home_subdir(&self) -> &'static str {
        match self {
            Self::Claude => ".claude",
        }
    }

    /// Filename of this agent's project-instruction file (e.g. `CLAUDE.md`).
    pub fn instruction_filename(&self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE.md",
        }
    }

    /// Subdirectory (relative to `home_subdir()`) holding installed skills.
    pub fn skill_subdir(&self) -> &'static str {
        match self {
            Self::Claude => "skills",
        }
    }

    /// Subdirectory (relative to `home_subdir()`) holding skill hooks.
    pub fn skill_hooks_subdir(&self) -> &'static str {
        match self {
            Self::Claude => "skill-hooks",
        }
    }

    /// Container image tag this agent runs in.
    ///
    /// Returns `"ur-worker"`, the tag that exists today, not `"ur-worker-claude"`:
    /// Claude keeps the unsuffixed legacy tag so `KNOWN_IMAGES`, existing
    /// `ur.toml` fixtures, and the `ur-worker-rust` child image are untouched.
    /// A second agent would introduce `ur-worker-<agent>` at that point.
    pub fn image_name(&self) -> &'static str {
        match self {
            Self::Claude => "ur-worker",
        }
    }

    /// Bare process/binary name, for logging and diagnostics only.
    ///
    /// Must NOT be used for foreground-process detection: the workerd exit
    /// watcher matches shell-vs-non-shell, since Claude Code's foreground
    /// process is `node`, not `claude`.
    pub fn binary_name(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
        }
    }

    /// Full shell command to launch the agent in the tmux pane, with the
    /// model flag applied if `model` is `Some` and non-blank.
    ///
    /// Owns its own model-flag syntax — a future agent may take `-m`, an env
    /// var, or nothing at all.
    pub fn spawn_command(&self, model: Option<&str>) -> String {
        match self {
            Self::Claude => match model.map(str::trim) {
                Some(model) if !model.is_empty() => format!("claude --model '{model}'"),
                _ => "claude".to_string(),
            },
        }
    }

    /// Built-in default model for the given worker strategy name (`"code"`,
    /// `"design"`, `"manual"`), or `None` for an unknown strategy or an agent
    /// with no model concept.
    ///
    /// Takes the strategy name rather than `WorkerStrategy` itself: that type
    /// lives in `crates/server`, which `ur_config` must not depend on.
    pub fn default_model(&self, strategy: &str) -> Option<&'static str> {
        match self {
            Self::Claude => match strategy {
                "code" => Some("sonnet"),
                "design" | "manual" => Some("opus"),
                _ => None,
            },
        }
    }

    /// Filename of this agent's settings file, or `None` if it has no
    /// settings-file concept.
    pub fn settings_filename(&self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("settings.json"),
        }
    }

    /// Memory-dir path relative to `home_subdir()`, or `None` if the agent has
    /// no memory-dir concept.
    ///
    /// This is Claude Code's own transcript layout, which `home_subdir()`
    /// alone does not cover.
    pub fn memory_subdir(&self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("projects/-workspace/memory"),
        }
    }

    /// Auth profile for this agent, or `None` for agents that need no
    /// credentials. Callers gate credential-bearing logic on
    /// `if let Some(auth) = agent.auth()`.
    pub fn auth(&self) -> Option<AgentAuth> {
        match self {
            Self::Claude => Some(AgentAuth {
                keychain_service: "Claude Code-credentials",
                credentials_filename: ".credentials.json",
                app_config_filename: ".claude.json",
            }),
        }
    }

    /// Parse an agent name (as returned by `name()`) into a variant.
    pub fn parse(s: &str) -> Result<Self, ParseAgentError> {
        match s {
            "claude" => Ok(Self::Claude),
            other => Err(ParseAgentError(other.to_string())),
        }
    }

    /// Same as `name()`; provided for call sites that prefer `as_str()`
    /// conventions over `name()`.
    pub fn as_str(&self) -> &'static str {
        self.name()
    }

    /// Read `UR_AGENT_TYPE` from the environment. Defaults to `Claude` when
    /// unset, empty, or unrecognized (a warning is logged in the
    /// unrecognized case).
    pub fn from_env() -> Self {
        match std::env::var(UR_AGENT_TYPE_ENV) {
            Ok(val) if !val.trim().is_empty() => match Self::parse(val.trim()) {
                Ok(agent) => agent,
                Err(err) => {
                    tracing::warn!(
                        value = %val,
                        error = %err,
                        "unrecognized UR_AGENT_TYPE, defaulting to claude"
                    );
                    Self::Claude
                }
            },
            _ => Self::Claude,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that mutate the process-wide `UR_AGENT_TYPE` env var.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn name_and_subdirs() {
        assert_eq!(AgentType::Claude.name(), "claude");
        assert_eq!(AgentType::Claude.home_subdir(), ".claude");
        assert_eq!(AgentType::Claude.instruction_filename(), "CLAUDE.md");
        assert_eq!(AgentType::Claude.skill_subdir(), "skills");
        assert_eq!(AgentType::Claude.skill_hooks_subdir(), "skill-hooks");
        assert_eq!(AgentType::Claude.binary_name(), "claude");
        assert_eq!(AgentType::Claude.image_name(), "ur-worker");
        assert_eq!(AgentType::Claude.as_str(), "claude");
    }

    #[test]
    fn spawn_command_no_model() {
        assert_eq!(AgentType::Claude.spawn_command(None), "claude");
    }

    #[test]
    fn spawn_command_with_model() {
        assert_eq!(
            AgentType::Claude.spawn_command(Some("opus")),
            "claude --model 'opus'"
        );
    }

    #[test]
    fn spawn_command_trims_model() {
        assert_eq!(
            AgentType::Claude.spawn_command(Some("  opus  ")),
            "claude --model 'opus'"
        );
    }

    #[test]
    fn spawn_command_blank_model_is_none() {
        assert_eq!(AgentType::Claude.spawn_command(Some("   ")), "claude");
    }

    #[test]
    fn default_model_per_strategy() {
        assert_eq!(AgentType::Claude.default_model("code"), Some("sonnet"));
        assert_eq!(AgentType::Claude.default_model("design"), Some("opus"));
        assert_eq!(AgentType::Claude.default_model("manual"), Some("opus"));
        assert_eq!(AgentType::Claude.default_model("bogus"), None);
    }

    #[test]
    fn settings_and_memory_subdir() {
        assert_eq!(AgentType::Claude.settings_filename(), Some("settings.json"));
        assert_eq!(
            AgentType::Claude.memory_subdir(),
            Some("projects/-workspace/memory")
        );
    }

    #[test]
    fn all_contains_claude() {
        assert_eq!(AgentType::ALL, &[AgentType::Claude]);
    }

    #[test]
    fn auth_profile() {
        let auth = AgentType::Claude.auth().expect("claude has auth");
        assert_eq!(auth.keychain_service, "Claude Code-credentials");
        assert_eq!(auth.credentials_filename, ".credentials.json");
        assert_eq!(auth.app_config_filename, ".claude.json");
    }

    #[test]
    fn parse_known_and_unknown() {
        assert_eq!(AgentType::parse("claude"), Ok(AgentType::Claude));
        assert_eq!(
            AgentType::parse("bogus"),
            Err(ParseAgentError("bogus".to_string()))
        );
    }

    #[test]
    fn from_env_defaults_when_unset() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::remove_var(UR_AGENT_TYPE_ENV);
        }
        assert_eq!(AgentType::from_env(), AgentType::Claude);
    }

    #[test]
    fn from_env_defaults_when_empty() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var(UR_AGENT_TYPE_ENV, "");
        }
        assert_eq!(AgentType::from_env(), AgentType::Claude);
        unsafe {
            std::env::remove_var(UR_AGENT_TYPE_ENV);
        }
    }

    #[test]
    fn from_env_reads_recognized_value() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var(UR_AGENT_TYPE_ENV, "claude");
        }
        assert_eq!(AgentType::from_env(), AgentType::Claude);
        unsafe {
            std::env::remove_var(UR_AGENT_TYPE_ENV);
        }
    }

    #[test]
    fn from_env_defaults_when_unrecognized() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var(UR_AGENT_TYPE_ENV, "bogus");
        }
        assert_eq!(AgentType::from_env(), AgentType::Claude);
        unsafe {
            std::env::remove_var(UR_AGENT_TYPE_ENV);
        }
    }
}
