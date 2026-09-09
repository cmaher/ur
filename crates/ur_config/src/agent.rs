use std::fmt;

use crate::UR_AGENT_TYPE_ENV;

/// Where an agent's host-side credentials come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthSource {
    /// macOS keychain, falling back to a home-relative file on Linux.
    Keychain {
        service: &'static str,
        linux_fallback: &'static str,
    },
    /// Plain file under the host user's home directory.
    HostFile { path_from_home: &'static str },
}

/// Auth profile for an agent. Future agents may use other auth styles —
/// those become new [`AuthSource`] variants if/when needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentAuth {
    pub source: AuthSource,
    /// Credentials file, relative to the worker home. Bind-mounted into the container.
    pub credentials_path: &'static str,
    /// App config, relative to the worker home. Baked into the image.
    pub app_config_path: &'static str,
}

/// Which AI agent runs a worker. `Claude` (Claude Code) and `Codex` (OpenAI
/// Codex CLI) today; future agents add variants here.
///
/// This is the single source of truth for everything that varies per agent:
/// home subdir, instruction filename, spawn command, image name, default
/// models, and auth profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentType {
    Claude,
    Codex,
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
    pub const ALL: &'static [AgentType] = &[AgentType::Claude, AgentType::Codex];

    /// Short, stable name for this agent. Used as the `UR_AGENT_TYPE` value,
    /// the credential directory name, and in `agent_type` proto fields.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// Home subdirectory (relative to the worker's home directory) holding
    /// this agent's config, skills, and instructions.
    pub fn home_subdir(&self) -> &'static str {
        match self {
            Self::Claude => ".claude",
            Self::Codex => ".codex",
        }
    }

    /// Filename of this agent's project-instruction file (e.g. `CLAUDE.md`).
    pub fn instruction_filename(&self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE.md",
            Self::Codex => "AGENTS.md",
        }
    }

    /// Subdirectory (relative to `home_subdir()`) holding installed skills.
    pub fn skill_subdir(&self) -> &'static str {
        match self {
            Self::Claude | Self::Codex => "skills",
        }
    }

    /// Subdirectory (relative to `home_subdir()`) holding skill hooks.
    pub fn skill_hooks_subdir(&self) -> &'static str {
        match self {
            Self::Claude | Self::Codex => "skill-hooks",
        }
    }

    // No `image_name()`: image selection is not agent-derived today. Project
    // images come from `ur.toml` (`IMAGE_ALIASES`) and the no-project fallback
    // is `DEFAULT_FALLBACK_IMAGE` (`ur-worker-rust:latest`, deliberately the
    // rust-toolchain image). A second agent adds `ur-worker-<agent>` and an
    // accessor here at the point something actually resolves an image per agent.
    //
    // No `binary_name()` either: the workerd exit watcher matches
    // shell-vs-non-shell foreground processes, never a binary name (Claude
    // Code's foreground process is `node`, not `claude`), and `spawn_command`
    // already carries the launch line. Add one when a caller needs it.

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
            Self::Codex => match model.map(str::trim) {
                Some(model) if !model.is_empty() => format!("codex -m '{model}'"),
                _ => "codex".to_string(),
            },
        }
    }

    /// Command that resets the agent's context (typed into the tmux pane).
    pub fn clear_command(&self) -> &'static str {
        match self {
            Self::Claude => "/clear",
            Self::Codex => "/new",
        }
    }

    /// How to ask this agent to run a named skill with arguments, as a single
    /// line typed into the tmux pane and submitted.
    ///
    /// Claude has a custom slash command per skill (`/implement ur-x`). Codex
    /// 0.153.4 has no custom slash commands — its skills are discovered by the
    /// model through the `skill_search` tool from the same `SKILL.md` directory
    /// format Claude uses — so the invocation instead names the skill
    /// explicitly and unambiguously enough for `skill_search` to find it.
    ///
    /// Takes `args: &[&str]` rather than a single pre-joined string because a
    /// skill like `address-feedback` takes two arguments (a ticket and a PR
    /// number); joining at the call site would push formatting knowledge back
    /// out of `AgentType`.
    pub fn skill_invocation(&self, skill: &str, args: &[&str]) -> String {
        match self {
            Self::Claude => {
                let mut invocation = format!("/{skill}");
                for arg in args {
                    invocation.push(' ');
                    invocation.push_str(arg);
                }
                invocation
            }
            Self::Codex => {
                if args.is_empty() {
                    format!("Run the `{skill}` skill.")
                } else {
                    format!("Run the `{skill}` skill. Arguments: {}", args.join(", "))
                }
            }
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
            Self::Codex => match strategy {
                "code" => Some("gpt-5.6-terra"),
                "design" | "manual" => Some("gpt-5.6-sol"),
                _ => None,
            },
        }
    }

    /// Filename of this agent's settings file, or `None` if it has no
    /// settings-file concept.
    pub fn settings_filename(&self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("settings.json"),
            Self::Codex => Some("config.toml"),
        }
    }

    /// Memory-dir path relative to `home_subdir()`, or `None` if the agent has
    /// no memory-dir concept.
    ///
    /// This is Claude Code's own transcript layout, which `home_subdir()`
    /// alone does not cover. Codex has no directory equivalent — it keeps
    /// memories in a sqlite database — so this is `None` and callers (e.g.
    /// `RunOptsBuilder::add_memory_dir`) no-op the mount.
    pub fn memory_subdir(&self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("projects/-workspace/memory"),
            Self::Codex => None,
        }
    }

    /// Auth profile for this agent, or `None` for agents that need no
    /// credentials. Callers gate credential-bearing logic on
    /// `if let Some(auth) = agent.auth()`.
    pub fn auth(&self) -> Option<AgentAuth> {
        match self {
            Self::Claude => Some(AgentAuth {
                source: AuthSource::Keychain {
                    service: "Claude Code-credentials",
                    linux_fallback: ".claude/.credentials.json",
                },
                credentials_path: ".claude/.credentials.json",
                app_config_path: ".claude.json",
            }),
            Self::Codex => Some(AgentAuth {
                source: AuthSource::HostFile {
                    path_from_home: ".codex/auth.json",
                },
                credentials_path: ".codex/auth.json",
                app_config_path: ".codex/config.toml",
            }),
        }
    }

    /// Domains this agent needs through the forward proxy.
    pub fn proxy_domains(&self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[
                "api.anthropic.com",
                "platform.claude.com",
                "downloads.claude.ai",
            ],
            Self::Codex => &["chatgpt.com", "api.openai.com", "auth.openai.com"],
        }
    }

    /// Parse an agent name (as returned by `name()`) into a variant.
    pub fn parse(s: &str) -> Result<Self, ParseAgentError> {
        match s {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            other => Err(ParseAgentError(other.to_string())),
        }
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
    }

    #[test]
    fn codex_name_and_subdirs() {
        assert_eq!(AgentType::Codex.name(), "codex");
        assert_eq!(AgentType::Codex.home_subdir(), ".codex");
        assert_eq!(AgentType::Codex.instruction_filename(), "AGENTS.md");
        assert_eq!(AgentType::Codex.skill_subdir(), "skills");
        assert_eq!(AgentType::Codex.skill_hooks_subdir(), "skill-hooks");
    }

    #[test]
    fn spawn_command_no_model() {
        assert_eq!(AgentType::Claude.spawn_command(None), "claude");
        assert_eq!(AgentType::Codex.spawn_command(None), "codex");
    }

    #[test]
    fn spawn_command_with_model() {
        assert_eq!(
            AgentType::Claude.spawn_command(Some("opus")),
            "claude --model 'opus'"
        );
        assert_eq!(
            AgentType::Codex.spawn_command(Some("gpt-5.6-sol")),
            "codex -m 'gpt-5.6-sol'"
        );
    }

    #[test]
    fn spawn_command_trims_model() {
        assert_eq!(
            AgentType::Claude.spawn_command(Some("  opus  ")),
            "claude --model 'opus'"
        );
        assert_eq!(
            AgentType::Codex.spawn_command(Some("  gpt-5.6-sol  ")),
            "codex -m 'gpt-5.6-sol'"
        );
    }

    #[test]
    fn spawn_command_blank_model_is_none() {
        assert_eq!(AgentType::Claude.spawn_command(Some("   ")), "claude");
        assert_eq!(AgentType::Codex.spawn_command(Some("   ")), "codex");
    }

    #[test]
    fn clear_command_per_agent() {
        assert_eq!(AgentType::Claude.clear_command(), "/clear");
        assert_eq!(AgentType::Codex.clear_command(), "/new");
    }

    #[test]
    fn skill_invocation_zero_args() {
        assert_eq!(
            AgentType::Claude.skill_invocation("implement", &[]),
            "/implement"
        );
        assert_eq!(
            AgentType::Codex.skill_invocation("implement", &[]),
            "Run the `implement` skill."
        );
    }

    #[test]
    fn skill_invocation_one_arg() {
        assert_eq!(
            AgentType::Claude.skill_invocation("implement", &["ur-x"]),
            "/implement ur-x"
        );
        assert_eq!(
            AgentType::Codex.skill_invocation("implement", &["ur-x"]),
            "Run the `implement` skill. Arguments: ur-x"
        );
    }

    #[test]
    fn skill_invocation_multiple_args() {
        assert_eq!(
            AgentType::Claude.skill_invocation("address-feedback", &["ur-x", "123"]),
            "/address-feedback ur-x 123"
        );
        assert_eq!(
            AgentType::Codex.skill_invocation("address-feedback", &["ur-x", "123"]),
            "Run the `address-feedback` skill. Arguments: ur-x, 123"
        );
    }

    #[test]
    fn default_model_per_strategy() {
        assert_eq!(AgentType::Claude.default_model("code"), Some("sonnet"));
        assert_eq!(AgentType::Claude.default_model("design"), Some("opus"));
        assert_eq!(AgentType::Claude.default_model("manual"), Some("opus"));
        assert_eq!(AgentType::Claude.default_model("bogus"), None);

        assert_eq!(
            AgentType::Codex.default_model("code"),
            Some("gpt-5.6-terra")
        );
        assert_eq!(
            AgentType::Codex.default_model("design"),
            Some("gpt-5.6-sol")
        );
        assert_eq!(
            AgentType::Codex.default_model("manual"),
            Some("gpt-5.6-sol")
        );
        assert_eq!(AgentType::Codex.default_model("bogus"), None);
    }

    /// Every agent must have a default model for every strategy the workflow
    /// coordinator dispatches — a `None` here would silently drop the
    /// `--model` flag for that agent/strategy combination.
    #[test]
    fn every_strategy_has_a_default_model() {
        for agent in AgentType::ALL {
            for strategy in ["code", "design", "manual"] {
                assert!(
                    agent.default_model(strategy).is_some(),
                    "{agent:?} has no default model for strategy {strategy:?}"
                );
            }
        }
    }

    #[test]
    fn settings_and_memory_subdir() {
        assert_eq!(AgentType::Claude.settings_filename(), Some("settings.json"));
        assert_eq!(
            AgentType::Claude.memory_subdir(),
            Some("projects/-workspace/memory")
        );

        assert_eq!(AgentType::Codex.settings_filename(), Some("config.toml"));
        assert_eq!(AgentType::Codex.memory_subdir(), None);
    }

    #[test]
    fn all_contains_claude_and_codex() {
        assert_eq!(AgentType::ALL, &[AgentType::Claude, AgentType::Codex]);
    }

    #[test]
    fn auth_profile() {
        let auth = AgentType::Claude.auth().expect("claude has auth");
        assert_eq!(auth.credentials_path, ".claude/.credentials.json");
        assert_eq!(auth.app_config_path, ".claude.json");
        assert_eq!(
            auth.source,
            AuthSource::Keychain {
                service: "Claude Code-credentials",
                linux_fallback: ".claude/.credentials.json",
            }
        );
    }

    #[test]
    fn codex_auth_profile() {
        let auth = AgentType::Codex.auth().expect("codex has auth");
        assert_eq!(auth.credentials_path, ".codex/auth.json");
        assert_eq!(auth.app_config_path, ".codex/config.toml");
        assert_eq!(
            auth.source,
            AuthSource::HostFile {
                path_from_home: ".codex/auth.json",
            }
        );
    }

    #[test]
    fn proxy_domains_per_agent() {
        assert_eq!(
            AgentType::Claude.proxy_domains(),
            &[
                "api.anthropic.com",
                "platform.claude.com",
                "downloads.claude.ai"
            ]
        );
        assert_eq!(
            AgentType::Codex.proxy_domains(),
            &["chatgpt.com", "api.openai.com", "auth.openai.com"]
        );
    }

    #[test]
    fn parse_known_and_unknown() {
        assert_eq!(AgentType::parse("claude"), Ok(AgentType::Claude));
        assert_eq!(AgentType::parse("codex"), Ok(AgentType::Codex));
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
    fn from_env_reads_codex() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var(UR_AGENT_TYPE_ENV, "codex");
        }
        assert_eq!(AgentType::from_env(), AgentType::Codex);
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
