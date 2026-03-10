use anyhow::Result;
use mlua::{Lua, StdLib, Value};

#[derive(Clone, Default)]
pub struct LuaTransformManager {
    // Lua VM is not Clone; create per-request or use a pool.
    // For simplicity, store scripts and create Lua VMs per-request.
    // Scripts are small and Lua VM creation is cheap.
}

impl LuaTransformManager {
    pub fn new() -> Self {
        Self {}
    }

    pub fn run_transform(
        &self,
        lua_source: &str,
        command: &str,
        args: &[String],
        working_dir: &str,
    ) -> Result<Vec<String>> {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8,
            mlua::LuaOptions::default(),
        )
        .map_err(|e| anyhow::anyhow!("creating lua vm: {e}"))?;

        lua.load(lua_source)
            .exec()
            .map_err(|e| anyhow::anyhow!("loading lua script: {e}"))?;

        let transform: mlua::Function = lua
            .globals()
            .get("transform")
            .map_err(|e| anyhow::anyhow!("lua script must define a transform function: {e}"))?;

        let lua_args = lua
            .create_table()
            .map_err(|e| anyhow::anyhow!("creating lua table: {e}"))?;
        for (i, arg) in args.iter().enumerate() {
            lua_args
                .set(i + 1, arg.as_str())
                .map_err(|e| anyhow::anyhow!("setting lua arg: {e}"))?;
        }

        let result = transform
            .call::<Value>((command, lua_args, working_dir))
            .map_err(|e| anyhow::anyhow!("lua transform failed: {e}"))?;

        match result {
            Value::Table(tbl) => {
                let len = tbl
                    .len()
                    .map_err(|e| anyhow::anyhow!("getting table len: {e}"))?;
                let mut out = Vec::new();
                for i in 1..=len {
                    let val: String = tbl
                        .get(i)
                        .map_err(|e| anyhow::anyhow!("reading table index {i}: {e}"))?;
                    out.push(val);
                }
                Ok(out)
            }
            _ => anyhow::bail!("lua transform must return a table"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passthrough_transform() {
        let mgr = LuaTransformManager::new();
        let script = "function transform(c, a, w) return a end";
        let result = mgr
            .run_transform(script, "git", &["status".into()], "/workspace")
            .unwrap();
        assert_eq!(result, vec!["status"]);
    }

    #[test]
    fn test_git_default_blocks_dash_c() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let result = mgr.run_transform(script, "git", &["-C".into(), "/tmp".into()], "/workspace");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked flag: -C"));
    }

    #[test]
    fn test_git_default_blocks_git_dir() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let result = mgr.run_transform(
            script,
            "git",
            &["--git-dir=/tmp".into(), "status".into()],
            "/workspace",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_git_default_blocks_worktree_config() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let result = mgr.run_transform(
            script,
            "git",
            &["-c".into(), "core.worktree=/tmp".into(), "status".into()],
            "/workspace",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_git_default_allows_normal_args() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec!["commit".into(), "-m".into(), "hello".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace")
            .unwrap();
        assert_eq!(result, args);
    }

    #[test]
    fn test_sandbox_no_io_access() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                io.open("/etc/passwd", "r")
                return a
            end
        "#;
        let result = mgr.run_transform(script, "test", &[], "/tmp");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_no_os_access() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                os.execute("whoami")
                return a
            end
        "#;
        let result = mgr.run_transform(script, "test", &[], "/tmp");
        assert!(result.is_err());
    }
}
