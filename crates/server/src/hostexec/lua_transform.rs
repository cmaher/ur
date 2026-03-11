use std::path::PathBuf;

use anyhow::Result;
use mlua::{Lua, StdLib, Value};

/// Agent context passed to Lua transform functions when available.
///
/// Contains per-agent metadata (identity, project association, host repo path)
/// needed by transforms that perform per-agent logic (e.g., git -C rewriting).
/// `None` when no agent/project is associated (e.g., raw `-w` workspace mounts).
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub agent_id: String,
    pub project_key: String,
    pub slot_path: PathBuf,
}

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
        agent_context: Option<&AgentContext>,
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

        // Build agent_context Lua table (nil if no context available)
        let lua_agent_context = if let Some(ctx) = agent_context {
            let tbl = lua
                .create_table()
                .map_err(|e| anyhow::anyhow!("creating agent_context table: {e}"))?;
            tbl.set("agent_id", ctx.agent_id.as_str())
                .map_err(|e| anyhow::anyhow!("setting agent_id: {e}"))?;
            tbl.set("project_key", ctx.project_key.as_str())
                .map_err(|e| anyhow::anyhow!("setting project_key: {e}"))?;
            tbl.set("slot_path", ctx.slot_path.to_string_lossy().as_ref())
                .map_err(|e| anyhow::anyhow!("setting slot_path: {e}"))?;
            Value::Table(tbl)
        } else {
            Value::Nil
        };

        let result = transform
            .call::<Value>((command, lua_args, working_dir, lua_agent_context))
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
            .run_transform(script, "git", &["status".into()], "/workspace", None)
            .unwrap();
        assert_eq!(result, vec!["status"]);
    }

    #[test]
    fn test_git_default_blocks_dash_c() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let result =
            mgr.run_transform(script, "git", &["-C".into(), "/tmp".into()], "/workspace", None);
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
            None,
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
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_git_default_allows_normal_args() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec!["commit".into(), "-m".into(), "hello".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", None)
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
        let result = mgr.run_transform(script, "test", &[], "/tmp", None);
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
        let result = mgr.run_transform(script, "test", &[], "/tmp", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_agent_context_available_in_lua() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(command, args, working_dir, agent_context)
                if agent_context == nil then
                    error("expected agent_context")
                end
                return {
                    agent_context.agent_id,
                    agent_context.project_key,
                    agent_context.slot_path,
                }
            end
        "#;
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
        };
        let result = mgr
            .run_transform(script, "git", &[], "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(
            result,
            vec![
                "deploy-x7q2",
                "ur",
                "/home/user/.ur/workspace/pool/ur/0",
            ]
        );
    }

    #[test]
    fn test_agent_context_nil_when_none() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(command, args, working_dir, agent_context)
                if agent_context ~= nil then
                    error("expected nil agent_context")
                end
                return args
            end
        "#;
        let result = mgr
            .run_transform(script, "git", &["status".into()], "/workspace", None)
            .unwrap();
        assert_eq!(result, vec!["status"]);
    }

    #[test]
    fn test_git_dash_c_rewrite_with_project_key() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
        };
        let args: Vec<String> = vec!["-C".into(), "/some/path/ur".into(), "status".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(
            result,
            vec![
                "-C",
                "/home/user/.ur/workspace/pool/ur/0",
                "status",
            ]
        );
    }

    #[test]
    fn test_git_dash_c_rewrite_with_workspace() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
        };
        let args: Vec<String> = vec!["-C".into(), "/workspace".into(), "status".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(
            result,
            vec![
                "-C",
                "/home/user/.ur/workspace/pool/ur/0",
                "status",
            ]
        );
    }

    #[test]
    fn test_git_dash_c_rewrite_bare_project_key() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
        };
        let args: Vec<String> = vec!["-C".into(), "ur".into(), "log".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result, vec!["-C", "/pool/ur/0", "log"]);
    }

    #[test]
    fn test_git_dash_c_rewrite_trailing_slash() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
        };
        let args: Vec<String> = vec!["-C".into(), "/workspace/".into(), "status".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result, vec!["-C", "/pool/ur/0", "status"]);
    }

    #[test]
    fn test_git_dash_c_rejected_wrong_project() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
        };
        let args: Vec<String> = vec!["-C".into(), "/tmp/evil".into(), "status".into()];
        let result = mgr.run_transform(script, "git", &args, "/workspace", Some(&ctx));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("does not match project key"));
    }

    #[test]
    fn test_git_dash_c_blocked_without_agent_context() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec!["-C".into(), "/workspace".into(), "status".into()];
        let result = mgr.run_transform(script, "git", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked flag: -C"));
    }

    #[test]
    fn test_existing_scripts_ignore_extra_arg() {
        // Verify that existing Lua scripts (git.lua, gh.lua) work fine with
        // the new 4th argument — Lua silently ignores extra arguments.
        let mgr = LuaTransformManager::new();
        let ctx = AgentContext {
            agent_id: "test-ab12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
        };

        // git.lua with agent context
        let git_script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec!["status".into()];
        let result = mgr
            .run_transform(git_script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result, vec!["status"]);

        // gh.lua with agent context
        let gh_script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["pr".into(), "list".into()];
        let result = mgr
            .run_transform(gh_script, "gh", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result, vec!["pr", "list"]);
    }
}
