use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result};

#[derive(Debug, Clone)]
pub struct CommandConfig {
    pub lua_source: Option<String>,
}

#[derive(Clone)]
pub struct HostExecConfigManager {
    commands: HashMap<String, CommandConfig>,
}

impl HostExecConfigManager {
    /// Build from the parsed `[hostexec]` section of `ur.toml`.
    ///
    /// Built-in defaults (git, gh) are loaded first, then user commands from
    /// the config are merged on top.
    pub fn load(config_dir: &Path, hostexec_cfg: &ur_config::HostExecConfig) -> Result<Self> {
        let mut commands = Self::defaults();

        let hostexec_dir = config_dir.join(ur_config::HOSTEXEC_DIR);

        for (name, cmd_cfg) in &hostexec_cfg.commands {
            let lua_source = Self::resolve_lua_source(name, cmd_cfg, &hostexec_dir)?;
            commands.insert(name.clone(), CommandConfig { lua_source });
        }

        Ok(Self { commands })
    }

    /// Create a new config manager with additional passthrough commands merged in.
    ///
    /// Per-project passthrough commands (from `ur.toml` `[projects.<key>]` `hostexec` list)
    /// are added as passthrough (no Lua transform). Existing commands are not overridden.
    pub fn with_passthrough_commands(&self, extra_commands: &[String]) -> Self {
        if extra_commands.is_empty() {
            return self.clone();
        }
        let mut commands = self.commands.clone();
        for name in extra_commands {
            commands
                .entry(name.clone())
                .or_insert(CommandConfig { lua_source: None });
        }
        Self { commands }
    }

    fn resolve_lua_source(
        name: &str,
        cmd_cfg: &ur_config::HostExecCommandConfig,
        hostexec_dir: &Path,
    ) -> Result<Option<String>> {
        if cmd_cfg.default_script {
            return Ok(Self::default_script(name));
        }
        if let Some(lua_file) = &cmd_cfg.lua {
            let lua_path = hostexec_dir.join(lua_file);
            let src = std::fs::read_to_string(&lua_path)
                .with_context(|| format!("reading lua script {}", lua_path.display()))?;
            return Ok(Some(src));
        }
        Ok(None)
    }

    fn defaults() -> HashMap<String, CommandConfig> {
        let mut commands = HashMap::new();
        commands.insert(
            "git".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/git.lua").into()),
            },
        );
        commands.insert(
            "gh".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/gh.lua").into()),
            },
        );
        commands.insert(
            "cargo".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/cargo.lua").into()),
            },
        );
        commands
    }

    fn default_script(name: &str) -> Option<String> {
        match name {
            "git" => Some(include_str!("default_scripts/git.lua").into()),
            "gh" => Some(include_str!("default_scripts/gh.lua").into()),
            "cargo" => Some(include_str!("default_scripts/cargo.lua").into()),
            _ => None,
        }
    }

    pub fn is_allowed(&self, command: &str) -> bool {
        self.commands.contains_key(command)
    }

    pub fn get(&self, command: &str) -> Option<&CommandConfig> {
        self.commands.get(command)
    }

    pub fn command_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.commands.keys().cloned().collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use ur_config::{HostExecCommandConfig, HostExecConfig};

    fn empty_config() -> HostExecConfig {
        HostExecConfig::default()
    }

    #[test]
    fn test_defaults_include_git_and_gh() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path(), &empty_config()).unwrap();

        assert!(mgr.is_allowed("git"));
        assert!(mgr.is_allowed("gh"));
        assert!(!mgr.is_allowed("tk"));
        assert_eq!(mgr.command_names(), vec!["cargo", "gh", "git"]);
    }

    #[test]
    fn test_user_config_extends_defaults() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = empty_config();
        cfg.commands.insert(
            "tk".into(),
            HostExecCommandConfig {
                lua: None,
                default_script: false,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();

        assert!(mgr.is_allowed("git"));
        assert!(mgr.is_allowed("gh"));
        assert!(mgr.is_allowed("tk"));
        assert!(mgr.get("tk").unwrap().lua_source.is_none());
    }

    #[test]
    fn test_user_config_overrides_default_with_custom_lua() {
        let tmp = TempDir::new().unwrap();
        let hostexec_dir = tmp.path().join(ur_config::HOSTEXEC_DIR);
        fs::create_dir_all(&hostexec_dir).unwrap();
        fs::write(
            hostexec_dir.join("my-git.lua"),
            "function transform(c, a, w) return a end",
        )
        .unwrap();

        let mut cfg = empty_config();
        cfg.commands.insert(
            "git".into(),
            HostExecCommandConfig {
                lua: Some("my-git.lua".into()),
                default_script: false,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();

        let git_cfg = mgr.get("git").unwrap();
        assert!(git_cfg.lua_source.as_ref().unwrap().contains("return a"));
    }

    #[test]
    fn test_default_script_flag() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = empty_config();
        cfg.commands.insert(
            "git".into(),
            HostExecCommandConfig {
                lua: None,
                default_script: true,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();

        let git_cfg = mgr.get("git").unwrap();
        assert!(git_cfg.lua_source.as_ref().unwrap().contains("blocked"));
    }

    #[test]
    fn test_defaults_include_cargo_with_lua() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path(), &empty_config()).unwrap();

        assert!(mgr.is_allowed("cargo"));
        let cargo_cfg = mgr.get("cargo").unwrap();
        assert!(cargo_cfg.lua_source.is_some());
        assert!(cargo_cfg.lua_source.as_ref().unwrap().contains("blocked"));
    }

    #[test]
    fn test_with_passthrough_commands_adds_new() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path(), &empty_config()).unwrap();

        let extra = vec!["tk".into(), "make".into()];
        let merged = mgr.with_passthrough_commands(&extra);

        assert!(merged.is_allowed("git"));
        assert!(merged.is_allowed("gh"));
        assert!(merged.is_allowed("tk"));
        assert!(merged.is_allowed("make"));
        assert!(merged.get("tk").unwrap().lua_source.is_none());
        assert!(merged.get("make").unwrap().lua_source.is_none());
    }

    #[test]
    fn test_with_passthrough_commands_does_not_override_existing() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path(), &empty_config()).unwrap();

        // git already exists with a Lua script — passthrough should not replace it
        let extra = vec!["git".into(), "tk".into()];
        let merged = mgr.with_passthrough_commands(&extra);

        assert!(merged.get("git").unwrap().lua_source.is_some());
        assert!(merged.get("tk").unwrap().lua_source.is_none());
    }

    #[test]
    fn test_with_passthrough_commands_empty_is_noop() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path(), &empty_config()).unwrap();
        let merged = mgr.with_passthrough_commands(&[]);

        assert_eq!(merged.command_names(), mgr.command_names());
    }

    #[test]
    fn test_with_passthrough_commands_preserves_global_user_config() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = empty_config();
        cfg.commands.insert(
            "tk".into(),
            HostExecCommandConfig {
                lua: None,
                default_script: false,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();
        let extra = vec!["make".into()];
        let merged = mgr.with_passthrough_commands(&extra);

        assert!(merged.is_allowed("git"));
        assert!(merged.is_allowed("gh"));
        assert!(merged.is_allowed("tk"));
        assert!(merged.is_allowed("make"));
    }
}
