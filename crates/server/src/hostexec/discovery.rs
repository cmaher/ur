use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

use super::config::CommandConfig;
use super::lua_transform::LuaTransformManager;

#[derive(Clone)]
pub struct LuaDiscoveryManager {
    hostexec_dir: PathBuf,
    lua: LuaTransformManager,
}

impl LuaDiscoveryManager {
    pub fn new(hostexec_dir: PathBuf, lua: LuaTransformManager) -> Self {
        Self { hostexec_dir, lua }
    }

    /// Discover and validate top-level Lua transforms by filename.
    pub fn discover(&self) -> Result<HashMap<String, CommandConfig>> {
        let entries = match std::fs::read_dir(&self.hostexec_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("reading hostexec directory {}", self.hostexec_dir.display())
                });
            }
        };

        let mut entries = entries
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| {
                format!("reading hostexec directory {}", self.hostexec_dir.display())
            })?;
        entries.sort_by_key(std::fs::DirEntry::path);

        let mut commands = HashMap::new();
        for entry in entries {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("reading file type for {}", path.display()))?;
            if !file_type.is_file() || path.extension() != Some(OsStr::new("lua")) {
                continue;
            }

            let name = command_name(&path)?;
            let lua_source = std::fs::read_to_string(&path)
                .with_context(|| format!("reading lua script {}", path.display()))?;
            let metadata = self
                .lua
                .read_metadata(&lua_source)
                .with_context(|| format!("validating lua script {}", path.display()))?;
            commands.insert(
                name,
                CommandConfig {
                    lua_source: Some(lua_source),
                    long_lived: metadata.long_lived,
                    bidi: metadata.bidi,
                },
            );
        }

        Ok(commands)
    }
}

fn command_name(path: &std::path::Path) -> Result<String> {
    let Some(name) = path.file_stem().and_then(OsStr::to_str) else {
        anyhow::bail!("invalid hostexec command filename {}", path.display());
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        anyhow::bail!("invalid hostexec command filename {}", path.display());
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    const TRANSFORM: &str = r#"
        function transform(command, args, working_dir)
            return { command = command, args = args, working_dir = working_dir }
        end
    "#;

    fn manager(path: &std::path::Path) -> LuaDiscoveryManager {
        LuaDiscoveryManager::new(path.to_path_buf(), LuaTransformManager::new())
    }

    #[test]
    fn discovers_only_top_level_lua_files() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("foo.lua"), TRANSFORM).unwrap();
        fs::write(temp.path().join("bar.txt"), "ignored").unwrap();
        fs::write(temp.path().join("script-shim.sh"), "ignored").unwrap();
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::write(temp.path().join("nested/hidden.lua"), TRANSFORM).unwrap();

        let discovered = manager(temp.path()).discover().unwrap();

        assert_eq!(discovered.len(), 1);
        assert!(discovered.contains_key("foo"));
    }

    #[test]
    fn reads_lua_metadata_into_command_config() {
        let temp = TempDir::new().unwrap();
        let script = format!("long_lived = true\n{TRANSFORM}");
        fs::write(temp.path().join("daemon.lua"), &script).unwrap();

        let discovered = manager(temp.path()).discover().unwrap();
        let command = discovered.get("daemon").unwrap();

        assert_eq!(command.lua_source.as_deref(), Some(script.as_str()));
        assert!(command.long_lived);
        assert!(!command.bidi);
    }

    #[test]
    fn rejects_invalid_stem_and_names_file() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("my command.lua"), TRANSFORM).unwrap();

        let error = manager(temp.path()).discover().unwrap_err();

        assert!(error.to_string().contains("my command.lua"));
    }

    #[test]
    fn reports_invalid_files_deterministically() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("z bad.lua"), TRANSFORM).unwrap();
        fs::write(temp.path().join("a bad.lua"), TRANSFORM).unwrap();

        let error = manager(temp.path()).discover().unwrap_err();

        assert!(error.to_string().contains("a bad.lua"));
    }

    #[test]
    fn lua_error_names_file() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("broken.lua"), "function transform(").unwrap();

        let error = manager(temp.path()).discover().unwrap_err();

        assert!(error.to_string().contains("broken.lua"));
    }

    #[test]
    fn missing_directory_is_empty() {
        let temp = TempDir::new().unwrap();
        let discovered = manager(&temp.path().join("missing")).discover().unwrap();

        assert!(discovered.is_empty());
    }

    #[test]
    fn empty_directory_is_empty() {
        let temp = TempDir::new().unwrap();
        let discovered = manager(temp.path()).discover().unwrap();

        assert!(discovered.is_empty());
    }
}
