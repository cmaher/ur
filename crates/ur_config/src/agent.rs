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
    /// Credentials are created by the agent inside the container and persist
    /// through the bind-mounted credentials file; there is no host source to seed.
    InContainer,
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

/// Smallest a seeded credentials file can plausibly be. Anything shorter is a
/// stub — an empty file the server's Docker bind-mount setup created so the
/// mount would succeed, or a truncated seed — and counts as "not seeded".
///
/// Single definition on purpose: the CLI's per-agent seeding (`crates/ur`), its
/// `ur start` warning, and the server's pre-launch gate (`check_credentials_seeded`)
/// must all agree on the boundary, and previously each carried its own literal
/// with a different comparison.
pub const MIN_SEEDED_CREDENTIALS_BYTES: u64 = 10;

/// Whether `path` holds credentials that look actually seeded (exists and is at
/// least [`MIN_SEEDED_CREDENTIALS_BYTES`]). Never reads the file's contents.
pub fn credentials_file_is_seeded(path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() >= MIN_SEEDED_CREDENTIALS_BYTES)
}

/// Which AI agent runs a worker. `Claude` (Claude Code) and `Codex` (OpenAI
/// Codex CLI) today; future agents add variants here.
///
/// This is the single source of truth for everything that varies per agent:
/// home subdir, instruction filename, spawn command, image name, default
/// models, and auth profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentType {
    Claude,
    Codex,
    Agy,
}

/// Error returned when parsing an unrecognized agent type string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseAgentError(pub String);

impl fmt::Display for ParseAgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let valid: Vec<&str> = AgentType::ALL.iter().map(AgentType::name).collect();
        write!(f, "unknown agent type: {}. Valid agents: {valid:?}", self.0)
    }
}

impl std::error::Error for ParseAgentError {}

/// Error returned when [`AgentType::resolve_image`] is given a value that is
/// neither a known alias nor a full image reference (`:` or `/`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownAliasError {
    pub raw: String,
    pub agent: AgentType,
}

impl fmt::Display for UnknownAliasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown image alias '{}' for agent '{}'. Valid aliases: {:?}. \
             Use a full image reference (e.g. 'myimage:tag') for custom images.",
            self.raw,
            self.agent.name(),
            crate::IMAGE_ALIASES
        )
    }
}

impl std::error::Error for UnknownAliasError {}

impl AgentType {
    /// All known agent variants. Lets callers with no dependency on the crate
    /// that parses `[worker_modes]` (e.g. the host CLI) iterate agents anyway.
    pub const ALL: &'static [AgentType] = &[AgentType::Claude, AgentType::Codex, AgentType::Agy];

    /// Short, stable name for this agent. Used as the `UR_AGENT_TYPE` value,
    /// the credential directory name, and in `agent_type` proto fields.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    /// Home subdirectory (relative to the worker's home directory) holding
    /// this agent's runtime settings and state.
    pub fn home_subdir(&self) -> &'static str {
        match self {
            Self::Claude => ".claude",
            Self::Codex => ".codex",
            Self::Agy => ".gemini/antigravity-cli",
        }
    }

    /// Home subdirectory holding customizations such as skills and instructions.
    ///
    /// AGY keeps these separate from its runtime settings and state. Existing
    /// agents use one root for both concepts.
    pub fn customization_root(&self) -> &'static str {
        match self {
            Self::Agy => ".gemini/config",
            Self::Claude | Self::Codex => self.home_subdir(),
        }
    }

    /// Filename of this agent's project-instruction file (e.g. `CLAUDE.md`).
    pub fn instruction_filename(&self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE.md",
            Self::Codex | Self::Agy => "AGENTS.md",
        }
    }

    /// Subdirectory (relative to `home_subdir()`) holding installed skills.
    pub fn skill_subdir(&self) -> &'static str {
        match self {
            Self::Claude | Self::Codex | Self::Agy => "skills",
        }
    }

    /// Subdirectory (relative to `home_subdir()`) holding skill hooks.
    pub fn skill_hooks_subdir(&self) -> &'static str {
        match self {
            Self::Claude | Self::Codex | Self::Agy => "skill-hooks",
        }
    }

    // No `binary_name()`: the workerd exit watcher matches shell-vs-non-shell
    // foreground processes, never a binary name (Claude Code's foreground
    // process is `node`, not `claude`), and `spawn_command` already carries
    // the launch line. Add one when a caller needs it.

    /// Resolve a config `container.image` value for this agent.
    ///
    /// A known alias (one of [`crate::IMAGE_ALIASES`]) becomes
    /// `<alias>-<agent-name>:latest`. The retired `ur-worker-rust` alias maps to
    /// the base per-agent image for compatibility. A value containing `:` or `/`
    /// is a full image reference and is returned unchanged.
    pub fn resolve_image(&self, raw: &str) -> Result<String, UnknownAliasError> {
        if raw.contains(':') || raw.contains('/') {
            return Ok(raw.to_string());
        }
        if raw == crate::LEGACY_RUST_IMAGE_ALIAS {
            return Ok(format!("ur-worker-{}:latest", self.name()));
        }
        if crate::IMAGE_ALIASES.contains(&raw) {
            return Ok(format!("{raw}-{}:latest", self.name()));
        }
        Err(UnknownAliasError {
            raw: raw.to_string(),
            agent: *self,
        })
    }

    /// Image used when a project configures none.
    ///
    /// Toolchain-specific behavior is supplied by per-project startup hooks.
    pub fn fallback_image(&self) -> String {
        self.resolve_image("ur-worker")
            .expect("'ur-worker' is always a valid alias")
    }

    /// Full shell command to launch the agent in the tmux pane, with the
    /// model flag applied if `model` is `Some` and non-blank and the effort
    /// flag always applied.
    ///
    /// Owns its own model- and effort-flag syntax — a future agent may take
    /// `-m`, an env var, or nothing at all.
    pub fn spawn_command(&self, model: Option<&str>, effort: &str) -> String {
        match self {
            Self::Claude => match model.map(str::trim) {
                Some(model) if !model.is_empty() => {
                    format!("claude --model '{model}' --effort '{effort}'")
                }
                _ => format!("claude --effort '{effort}'"),
            },
            Self::Codex => match model.map(str::trim) {
                Some(model) if !model.is_empty() => {
                    format!("codex -m '{model}' -c model_reasoning_effort=\"{effort}\"")
                }
                _ => format!("codex -c model_reasoning_effort=\"{effort}\""),
            },
            Self::Agy => match model.map(str::trim) {
                Some(model) if !model.is_empty() => format!(
                    "agy --model '{model}' --effort '{effort}' --dangerously-skip-permissions"
                ),
                _ => format!("agy --effort '{effort}' --dangerously-skip-permissions"),
            },
        }
    }

    /// Reasoning-effort values accepted by this agent's currently supported
    /// models. Model-specific restrictions remain the agent CLI's concern.
    pub fn supported_efforts(&self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["low", "medium", "high", "xhigh", "max"],
            Self::Codex => &["low", "medium", "high", "xhigh", "max", "ultra"],
            Self::Agy => &["low", "medium", "high", "xhigh", "max", "ultra"],
        }
    }

    /// Built-in reasoning effort used when no mode or worker-model override
    /// supplies one. Unlike [`Self::default_model`], it is not strategy-keyed.
    pub fn default_effort(&self) -> &'static str {
        match self {
            Self::Claude | Self::Codex | Self::Agy => "medium",
        }
    }

    /// Command that resets the agent's context (typed into the tmux pane).
    pub fn clear_command(&self) -> &'static str {
        match self {
            Self::Claude => "/clear",
            Self::Codex => "/new",
            Self::Agy => "/clear",
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
            Self::Claude | Self::Agy => {
                let mut invocation = format!("/{skill}");
                for arg in args {
                    invocation.push(' ');
                    invocation.push_str(arg);
                }
                invocation
            }
            Self::Codex => {
                let design_skill_prohibition = if skill == "implement" {
                    " DO NOT USE DESIGN SKILL."
                } else {
                    ""
                };
                if args.is_empty() {
                    format!("Run the `{skill}` skill.{design_skill_prohibition}")
                } else {
                    format!(
                        "Run the `{skill}` skill.{design_skill_prohibition} Arguments: {}",
                        args.join(", ")
                    )
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
            Self::Agy => match strategy {
                "code" | "design" | "manual" => Some("gemini-3.8-flash"),
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
            Self::Agy => Some("settings.json"),
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
            Self::Codex | Self::Agy => None,
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
            Self::Agy => Some(AgentAuth {
                source: AuthSource::InContainer,
                credentials_path: ".gemini/antigravity-cli/antigravity-oauth-token",
                app_config_path: ".gemini/antigravity-cli/settings.json",
            }),
        }
    }

    /// Host-side path to this agent's seeded credentials file, under
    /// `$UR_CONFIG/<agent-name>/`, or `None` for an agent with no auth
    /// profile. `config_dir` is the locally-reachable `$UR_CONFIG` root (the
    /// server sees it bind-mounted at `/config`; the `ur` CLI sees it
    /// directly) — `credential_manager_for` (`crates/ur/src/credential/`)
    /// resolves this exact path for the CLI side; this is the server-side
    /// equivalent, since `crates/server` cannot depend on `crates/ur`.
    pub fn host_credentials_path(
        &self,
        config_dir: &std::path::Path,
    ) -> Option<std::path::PathBuf> {
        let auth = self.auth()?;
        let filename = std::path::Path::new(auth.credentials_path).file_name()?;
        Some(config_dir.join(self.name()).join(filename))
    }

    /// How to log into this agent on the host, for use in
    /// [`AgentType::credentials_remediation`].
    fn login_instruction(&self) -> &'static str {
        match self {
            Self::Claude => "log in to Claude Code on this machine",
            Self::Codex => "run `codex login` on this machine",
            Self::Agy => "launch a worker and complete the sign-in in the pane",
        }
    }

    /// Remediation message for a launch attempted with no seeded credentials
    /// for this agent: names the agent, how to log in, and the reseed
    /// command that picks the login up. Callers should only reach for this
    /// when `auth()` is `Some` — an agent with no auth profile has nothing to
    /// remediate.
    pub fn credentials_remediation(&self) -> String {
        if matches!(self, Self::Agy) {
            return format!(
                "no credentials for agent '{}' — {}",
                self.name(),
                self.login_instruction()
            );
        }
        format!(
            "no credentials for agent '{name}' — {login}, then run \
             `ur worker reseed-credentials --agent {name}`",
            name = self.name(),
            login = self.login_instruction(),
        )
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
            Self::Agy => &[
                "oauth2.googleapis.com",
                "www.googleapis.com",
                "cloudcode-pa.googleapis.com",
                "daily-cloudcode-pa.googleapis.com",
                "lh3.googleusercontent.com",
                "accounts.google.com",
            ],
        }
    }

    /// Parse an agent name (as returned by `name()`) into a variant.
    pub fn parse(s: &str) -> Result<Self, ParseAgentError> {
        match s {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "agy" => Ok(Self::Agy),
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
    fn agy_name_and_subdirs() {
        assert_eq!(AgentType::Agy.name(), "agy");
        assert_eq!(AgentType::Agy.home_subdir(), ".gemini/antigravity-cli");
        assert_eq!(AgentType::Agy.customization_root(), ".gemini/config");
        assert_eq!(AgentType::Agy.instruction_filename(), "AGENTS.md");
        assert_eq!(AgentType::Agy.skill_subdir(), "skills");
        assert_eq!(AgentType::Agy.skill_hooks_subdir(), "skill-hooks");
        assert_eq!(AgentType::Claude.customization_root(), ".claude");
        assert_eq!(AgentType::Codex.customization_root(), ".codex");
    }

    #[test]
    fn spawn_command_no_model_keeps_effort() {
        assert_eq!(
            AgentType::Claude.spawn_command(None, "high"),
            "claude --effort 'high'"
        );
        assert_eq!(
            AgentType::Codex.spawn_command(None, "high"),
            "codex -c model_reasoning_effort=\"high\""
        );
        assert_eq!(
            AgentType::Agy.spawn_command(None, "high"),
            "agy --effort 'high' --dangerously-skip-permissions"
        );
    }

    #[test]
    fn spawn_command_with_model() {
        assert_eq!(
            AgentType::Claude.spawn_command(Some("opus"), "xhigh"),
            "claude --model 'opus' --effort 'xhigh'"
        );
        assert_eq!(
            AgentType::Codex.spawn_command(Some("gpt-5.6-sol"), "max"),
            "codex -m 'gpt-5.6-sol' -c model_reasoning_effort=\"max\""
        );
        assert_eq!(
            AgentType::Agy.spawn_command(Some("gemini-3.8-flash"), "medium"),
            "agy --model 'gemini-3.8-flash' --effort 'medium' --dangerously-skip-permissions"
        );
    }

    #[test]
    fn spawn_command_trims_model() {
        assert_eq!(
            AgentType::Claude.spawn_command(Some("  opus  "), "medium"),
            "claude --model 'opus' --effort 'medium'"
        );
        assert_eq!(
            AgentType::Codex.spawn_command(Some("  gpt-5.6-sol  "), "medium"),
            "codex -m 'gpt-5.6-sol' -c model_reasoning_effort=\"medium\""
        );
    }

    #[test]
    fn spawn_command_blank_model_is_none() {
        assert_eq!(
            AgentType::Claude.spawn_command(Some("   "), "low"),
            "claude --effort 'low'"
        );
        assert_eq!(
            AgentType::Codex.spawn_command(Some("   "), "low"),
            "codex -c model_reasoning_effort=\"low\""
        );
        assert_eq!(
            AgentType::Agy.spawn_command(Some("   "), "low"),
            "agy --effort 'low' --dangerously-skip-permissions"
        );
    }

    #[test]
    fn image_resolution_per_agent() {
        assert_eq!(
            AgentType::Agy.resolve_image("ur-worker").unwrap(),
            "ur-worker-agy:latest"
        );
        assert_eq!(AgentType::Agy.fallback_image(), "ur-worker-agy:latest");
        assert_eq!(
            AgentType::Agy
                .resolve_image("registry.example/worker:v1")
                .unwrap(),
            "registry.example/worker:v1"
        );
    }

    #[test]
    fn clear_command_per_agent() {
        assert_eq!(AgentType::Claude.clear_command(), "/clear");
        assert_eq!(AgentType::Codex.clear_command(), "/new");
        assert_eq!(AgentType::Agy.clear_command(), "/clear");
    }

    #[test]
    fn skill_invocation_zero_args() {
        assert_eq!(
            AgentType::Claude.skill_invocation("implement", &[]),
            "/implement"
        );
        assert_eq!(
            AgentType::Codex.skill_invocation("implement", &[]),
            "Run the `implement` skill. DO NOT USE DESIGN SKILL."
        );
        assert_eq!(
            AgentType::Agy.skill_invocation("implement", &[]),
            "/implement"
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
            "Run the `implement` skill. DO NOT USE DESIGN SKILL. Arguments: ur-x"
        );
        assert_eq!(
            AgentType::Agy.skill_invocation("implement", &["ur-x"]),
            "/implement ur-x"
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
        assert_eq!(
            AgentType::Agy.skill_invocation("address-feedback", &["ur-x", "123"]),
            "/address-feedback ur-x 123"
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

        for strategy in ["code", "design", "manual"] {
            assert_eq!(
                AgentType::Agy.default_model(strategy),
                Some("gemini-3.8-flash")
            );
        }
        assert_eq!(AgentType::Agy.default_model("bogus"), None);
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
    fn default_effort_is_supported_by_every_agent() {
        for agent in AgentType::ALL {
            assert!(
                agent.supported_efforts().contains(&agent.default_effort()),
                "{agent:?}'s default effort must be supported"
            );
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
        assert_eq!(AgentType::Agy.settings_filename(), Some("settings.json"));
        assert_eq!(AgentType::Agy.memory_subdir(), None);
    }

    #[test]
    fn all_contains_every_agent() {
        assert_eq!(
            AgentType::ALL,
            &[AgentType::Claude, AgentType::Codex, AgentType::Agy]
        );
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
    fn agy_auth_profile() {
        let auth = AgentType::Agy.auth().expect("agy has auth");
        assert_eq!(
            auth.credentials_path,
            ".gemini/antigravity-cli/antigravity-oauth-token"
        );
        assert_eq!(
            auth.app_config_path,
            ".gemini/antigravity-cli/settings.json"
        );
        assert_eq!(auth.source, AuthSource::InContainer);
    }

    #[test]
    fn credentials_file_is_seeded_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.json");
        assert!(!credentials_file_is_seeded(&missing), "missing file");

        let empty = tmp.path().join("empty.json");
        std::fs::write(&empty, "").unwrap();
        assert!(!credentials_file_is_seeded(&empty), "empty stub");

        let short = tmp.path().join("short.json");
        std::fs::write(
            &short,
            "x".repeat(MIN_SEEDED_CREDENTIALS_BYTES as usize - 1),
        )
        .unwrap();
        assert!(!credentials_file_is_seeded(&short), "truncated seed");

        let exact = tmp.path().join("exact.json");
        std::fs::write(&exact, "x".repeat(MIN_SEEDED_CREDENTIALS_BYTES as usize)).unwrap();
        assert!(credentials_file_is_seeded(&exact), "at the threshold");
    }

    #[test]
    fn host_credentials_path_per_agent() {
        let config_dir = std::path::Path::new("/config");
        assert_eq!(
            AgentType::Claude.host_credentials_path(config_dir).unwrap(),
            std::path::PathBuf::from("/config/claude/.credentials.json")
        );
        assert_eq!(
            AgentType::Codex.host_credentials_path(config_dir).unwrap(),
            std::path::PathBuf::from("/config/codex/auth.json")
        );
        assert_eq!(
            AgentType::Agy.host_credentials_path(config_dir).unwrap(),
            std::path::PathBuf::from("/config/agy/antigravity-oauth-token")
        );
    }

    #[test]
    fn credentials_remediation_is_per_agent() {
        let claude_msg = AgentType::Claude.credentials_remediation();
        assert!(claude_msg.contains("claude"), "{claude_msg}");
        assert!(claude_msg.contains("Claude Code"), "{claude_msg}");
        assert!(
            claude_msg.contains("ur worker reseed-credentials --agent claude"),
            "{claude_msg}"
        );

        let codex_msg = AgentType::Codex.credentials_remediation();
        assert!(codex_msg.contains("codex login"), "{codex_msg}");
        assert!(
            codex_msg.contains("ur worker reseed-credentials --agent codex"),
            "{codex_msg}"
        );

        let agy_msg = AgentType::Agy.credentials_remediation();
        assert!(agy_msg.contains("sign-in in the pane"), "{agy_msg}");
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
        assert_eq!(
            AgentType::Agy.proxy_domains(),
            &[
                "oauth2.googleapis.com",
                "www.googleapis.com",
                "cloudcode-pa.googleapis.com",
                "daily-cloudcode-pa.googleapis.com",
                "lh3.googleusercontent.com",
                "accounts.google.com"
            ]
        );
    }

    #[test]
    fn parse_known_and_unknown() {
        assert_eq!(AgentType::parse("claude"), Ok(AgentType::Claude));
        assert_eq!(AgentType::parse("codex"), Ok(AgentType::Codex));
        assert_eq!(AgentType::parse("agy"), Ok(AgentType::Agy));
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
    fn from_env_reads_agy() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var(UR_AGENT_TYPE_ENV, "agy");
        }
        assert_eq!(AgentType::from_env(), AgentType::Agy);
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
