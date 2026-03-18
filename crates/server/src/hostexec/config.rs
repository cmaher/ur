use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result};

#[derive(Debug, Clone)]
pub struct CommandConfig {
    pub lua_source: Option<String>,
    pub long_lived: bool,
    pub bidi: bool,
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
            commands.insert(
                name.clone(),
                CommandConfig {
                    lua_source,
                    long_lived: cmd_cfg.long_lived,
                    bidi: cmd_cfg.bidi,
                },
            );
        }

        Ok(Self { commands })
    }

    /// Create a new config manager containing only the built-in default commands.
    pub fn defaults_only(&self) -> Self {
        Self {
            commands: Self::defaults(),
        }
    }

    /// Create a new config with only default commands plus project-granted commands.
    ///
    /// Per-project `hostexec` arrays in `ur.toml` grant workers access to commands.
    /// Granted commands that exist in the registry (from `[hostexec.commands]`) use
    /// their configured settings (lua, long_lived, bidi). Granted commands not in the
    /// registry are added as passthrough (no Lua, not long_lived, not bidi).
    /// Default commands (git, gh, cargo, docker, ur) are always included.
    pub fn with_project_commands(&self, granted: &[String]) -> Self {
        let mut commands = Self::defaults();
        for name in granted {
            if let Some(cfg) = self.commands.get(name) {
                commands.insert(name.clone(), cfg.clone());
            } else {
                commands.entry(name.clone()).or_insert(CommandConfig {
                    lua_source: None,
                    long_lived: false,
                    bidi: false,
                });
            }
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
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "gh".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/gh.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "cargo".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/cargo.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "docker".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/docker.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "ur".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/ur.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands
    }

    fn default_script(name: &str) -> Option<String> {
        match name {
            "git" => Some(include_str!("default_scripts/git.lua").into()),
            "gh" => Some(include_str!("default_scripts/gh.lua").into()),
            "cargo" => Some(include_str!("default_scripts/cargo.lua").into()),
            "docker" => Some(include_str!("default_scripts/docker.lua").into()),
            "ur" => Some(include_str!("default_scripts/ur.lua").into()),
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

    pub fn command_entries(&self) -> Vec<ur_rpc::proto::hostexec::HostExecCommandEntry> {
        let mut entries: Vec<_> = self
            .commands
            .iter()
            .map(
                |(name, cfg)| ur_rpc::proto::hostexec::HostExecCommandEntry {
                    name: name.clone(),
                    bidi: cfg.bidi,
                },
            )
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
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
        assert_eq!(
            mgr.command_names(),
            vec!["cargo", "docker", "gh", "git", "ur"]
        );
    }

    #[test]
    fn test_user_config_extends_defaults() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = empty_config();
        cfg.commands.insert(
            "rg".into(),
            HostExecCommandConfig {
                lua: None,
                default_script: false,
                long_lived: false,
                bidi: false,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();

        assert!(mgr.is_allowed("git"));
        assert!(mgr.is_allowed("gh"));
        assert!(mgr.is_allowed("rg"));
        assert!(mgr.get("rg").unwrap().lua_source.is_none());
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
                long_lived: false,
                bidi: false,
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
                long_lived: false,
                bidi: false,
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
    fn test_project_commands_grants_access() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path(), &empty_config()).unwrap();

        let granted = vec!["rg".into(), "jq".into()];
        let merged = mgr.with_project_commands(&granted);

        // Defaults always present
        assert!(merged.is_allowed("git"));
        assert!(merged.is_allowed("gh"));
        // Granted commands added as passthrough
        assert!(merged.is_allowed("rg"));
        assert!(merged.is_allowed("jq"));
        assert!(merged.get("rg").unwrap().lua_source.is_none());
        assert!(merged.get("jq").unwrap().lua_source.is_none());
    }

    #[test]
    fn test_project_commands_uses_registry_config() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = empty_config();
        cfg.commands.insert(
            "daemon".into(),
            HostExecCommandConfig {
                lua: None,
                default_script: false,
                long_lived: true,
                bidi: true,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();
        // Grant "daemon" — should pick up long_lived/bidi from registry
        let merged = mgr.with_project_commands(&["daemon".into()]);

        assert!(merged.is_allowed("daemon"));
        let daemon_cfg = merged.get("daemon").unwrap();
        assert!(daemon_cfg.long_lived);
        assert!(daemon_cfg.bidi);
    }

    #[test]
    fn test_project_commands_does_not_expose_ungrated_registry_entries() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = empty_config();
        cfg.commands.insert(
            "jq".into(),
            HostExecCommandConfig {
                lua: None,
                default_script: false,
                long_lived: false,
                bidi: false,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();
        // jq is in the registry but NOT granted to this project
        let merged = mgr.with_project_commands(&["rg".into()]);

        assert!(merged.is_allowed("git")); // default
        assert!(merged.is_allowed("rg")); // granted
        assert!(!merged.is_allowed("jq")); // not granted, not a default
    }

    #[test]
    fn test_project_commands_empty_returns_defaults_only() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = empty_config();
        cfg.commands.insert(
            "jq".into(),
            HostExecCommandConfig {
                lua: None,
                default_script: false,
                long_lived: false,
                bidi: false,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();
        let merged = mgr.with_project_commands(&[]);

        assert_eq!(
            merged.command_names(),
            vec!["cargo", "docker", "gh", "git", "ur"]
        );
    }

    #[test]
    fn test_project_commands_preserves_default_lua() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path(), &empty_config()).unwrap();

        // Granting "git" doesn't replace the default lua script
        let merged = mgr.with_project_commands(&["git".into()]);
        assert!(merged.get("git").unwrap().lua_source.is_some());
    }

    #[test]
    fn test_defaults_only_excludes_user_config() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = empty_config();
        cfg.commands.insert(
            "jq".into(),
            HostExecCommandConfig {
                lua: None,
                default_script: false,
                long_lived: false,
                bidi: false,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();
        let defaults = mgr.defaults_only();

        assert!(defaults.is_allowed("git"));
        assert!(!defaults.is_allowed("jq"));
    }

    #[test]
    fn test_long_lived_and_bidi_in_registry() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = empty_config();
        cfg.commands.insert(
            "daemon".into(),
            HostExecCommandConfig {
                lua: None,
                default_script: false,
                long_lived: true,
                bidi: true,
            },
        );

        let mgr = HostExecConfigManager::load(tmp.path(), &cfg).unwrap();
        let daemon_cfg = mgr.get("daemon").unwrap();
        assert!(daemon_cfg.long_lived);
        assert!(daemon_cfg.bidi);
    }

    #[test]
    fn test_defaults_have_long_lived_and_bidi_false() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path(), &empty_config()).unwrap();
        let git_cfg = mgr.get("git").unwrap();
        assert!(!git_cfg.long_lived);
        assert!(!git_cfg.bidi);
    }

    #[test]
    fn test_project_granted_passthrough_has_long_lived_and_bidi_false() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path(), &empty_config()).unwrap();
        let merged = mgr.with_project_commands(&["rg".into()]);
        let rg_cfg = merged.get("rg").unwrap();
        assert!(!rg_cfg.long_lived);
        assert!(!rg_cfg.bidi);
    }
}
