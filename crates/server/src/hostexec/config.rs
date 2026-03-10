use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
struct RawAllowlist {
    #[serde(default)]
    commands: HashMap<String, RawCommandConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct RawCommandConfig {
    #[serde(default)]
    lua: Option<String>,
    #[serde(default)]
    default_script: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct CommandConfig {
    pub lua_source: Option<String>,
}

#[derive(Clone)]
pub struct HostExecConfigManager {
    commands: HashMap<String, CommandConfig>,
}

impl HostExecConfigManager {
    pub fn load(config_dir: &Path) -> Result<Self> {
        let mut commands = Self::defaults();

        let allowlist_path = config_dir
            .join(ur_config::HOSTEXEC_DIR)
            .join(ur_config::HOSTEXEC_ALLOWLIST_FILE);

        if !allowlist_path.exists() {
            return Ok(Self { commands });
        }

        let content = std::fs::read_to_string(&allowlist_path)
            .with_context(|| format!("reading {}", allowlist_path.display()))?;
        let raw: RawAllowlist = toml::from_str(&content)
            .with_context(|| format!("parsing {}", allowlist_path.display()))?;

        let hostexec_dir = config_dir.join(ur_config::HOSTEXEC_DIR);

        for (name, raw_cfg) in raw.commands {
            let lua_source = Self::resolve_lua_source(&name, &raw_cfg, &hostexec_dir)?;
            commands.insert(name, CommandConfig { lua_source });
        }

        Ok(Self { commands })
    }

    fn resolve_lua_source(
        name: &str,
        raw_cfg: &RawCommandConfig,
        hostexec_dir: &Path,
    ) -> Result<Option<String>> {
        if raw_cfg.default_script.unwrap_or(false) {
            return Ok(Self::default_script(name));
        }
        if let Some(lua_file) = &raw_cfg.lua {
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
        commands
    }

    fn default_script(name: &str) -> Option<String> {
        match name {
            "git" => Some(include_str!("default_scripts/git.lua").into()),
            "gh" => Some(include_str!("default_scripts/gh.lua").into()),
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

    #[test]
    fn test_defaults_include_git_and_gh() {
        let tmp = TempDir::new().unwrap();
        let mgr = HostExecConfigManager::load(tmp.path()).unwrap();

        assert!(mgr.is_allowed("git"));
        assert!(mgr.is_allowed("gh"));
        assert!(!mgr.is_allowed("tk"));
        assert_eq!(mgr.command_names(), vec!["gh", "git"]);
    }

    #[test]
    fn test_user_config_extends_defaults() {
        let tmp = TempDir::new().unwrap();
        let hostexec_dir = tmp.path().join(ur_config::HOSTEXEC_DIR);
        fs::create_dir_all(&hostexec_dir).unwrap();
        fs::write(
            hostexec_dir.join(ur_config::HOSTEXEC_ALLOWLIST_FILE),
            "[commands]\ntk = {}\n",
        )
        .unwrap();

        let mgr = HostExecConfigManager::load(tmp.path()).unwrap();

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
            hostexec_dir.join(ur_config::HOSTEXEC_ALLOWLIST_FILE),
            "[commands]\ngit = { lua = \"my-git.lua\" }\n",
        )
        .unwrap();
        fs::write(
            hostexec_dir.join("my-git.lua"),
            "function transform(c, a, w) return a end",
        )
        .unwrap();

        let mgr = HostExecConfigManager::load(tmp.path()).unwrap();

        let git_cfg = mgr.get("git").unwrap();
        assert!(git_cfg.lua_source.as_ref().unwrap().contains("return a"));
    }

    #[test]
    fn test_default_script_flag() {
        let tmp = TempDir::new().unwrap();
        let hostexec_dir = tmp.path().join(ur_config::HOSTEXEC_DIR);
        fs::create_dir_all(&hostexec_dir).unwrap();
        fs::write(
            hostexec_dir.join(ur_config::HOSTEXEC_ALLOWLIST_FILE),
            "[commands]\ngit = { default_script = true }\n",
        )
        .unwrap();

        let mgr = HostExecConfigManager::load(tmp.path()).unwrap();

        let git_cfg = mgr.get("git").unwrap();
        assert!(git_cfg.lua_source.as_ref().unwrap().contains("blocked"));
    }
}
