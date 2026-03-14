use std::collections::HashMap;
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

/// Structured result from a Lua transform function.
///
/// Contains the full execution spec: command, args, working directory,
/// and optional environment variables to set on the spawned process.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformResult {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: String,
    pub env: HashMap<String, String>,
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
    ) -> Result<TransformResult> {
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
                let command: String = tbl.get("command").map_err(|e| {
                    anyhow::anyhow!("missing or invalid 'command' field (expected string): {e}")
                })?;

                let args_value: Value = tbl
                    .get("args")
                    .map_err(|e| anyhow::anyhow!("missing 'args' field: {e}"))?;
                let args = extract_args(args_value)?;

                let working_dir: String = tbl.get("working_dir").map_err(|e| {
                    anyhow::anyhow!("missing or invalid 'working_dir' field (expected string): {e}")
                })?;

                let env_value: Value = tbl
                    .get("env")
                    .map_err(|e| anyhow::anyhow!("reading 'env' field: {e}"))?;
                let env = extract_env(env_value)?;

                Ok(TransformResult {
                    command,
                    args,
                    working_dir,
                    env,
                })
            }
            _ => anyhow::bail!("lua transform must return a table"),
        }
    }
}

fn extract_args(value: Value) -> Result<Vec<String>> {
    match value {
        Value::Table(args_tbl) => {
            let len = args_tbl
                .len()
                .map_err(|e| anyhow::anyhow!("getting args table len: {e}"))?;
            let mut out = Vec::new();
            for i in 1..=len {
                let val: String = args_tbl
                    .get(i)
                    .map_err(|e| anyhow::anyhow!("args[{i}] must be a string: {e}"))?;
                out.push(val);
            }
            Ok(out)
        }
        _ => anyhow::bail!("'args' field must be a table"),
    }
}

fn extract_env(value: Value) -> Result<HashMap<String, String>> {
    match value {
        Value::Nil => Ok(HashMap::new()),
        Value::Table(env_tbl) => {
            let mut map = HashMap::new();
            for pair in env_tbl.pairs::<String, String>() {
                let (k, v) = pair.map_err(|e| {
                    anyhow::anyhow!("env entries must be string key-value pairs: {e}")
                })?;
                map.insert(k, v);
            }
            Ok(map)
        }
        _ => anyhow::bail!("'env' field must be a table or nil"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passthrough_transform() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                return { command = c, args = a, working_dir = w }
            end
        "#;
        let result = mgr
            .run_transform(script, "git", &["status".into()], "/workspace", None)
            .unwrap();
        assert_eq!(result.command, "git");
        assert_eq!(result.args, vec!["status"]);
        assert_eq!(result.working_dir, "/workspace");
        assert!(result.env.is_empty());
    }

    #[test]
    fn test_git_default_blocks_dash_c() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let result = mgr.run_transform(
            script,
            "git",
            &["-C".into(), "/tmp".into()],
            "/workspace",
            None,
        );
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
        assert_eq!(result.args, args);
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
                    command = command,
                    args = {
                        agent_context.agent_id,
                        agent_context.project_key,
                        agent_context.slot_path,
                    },
                    working_dir = working_dir,
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
            result.args,
            vec!["deploy-x7q2", "ur", "/home/user/.ur/workspace/pool/ur/0",]
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
                return { command = command, args = args, working_dir = working_dir }
            end
        "#;
        let result = mgr
            .run_transform(script, "git", &["status".into()], "/workspace", None)
            .unwrap();
        assert_eq!(result.args, vec!["status"]);
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
            result.args,
            vec!["-C", "/home/user/.ur/workspace/pool/ur/0", "status",]
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
            result.args,
            vec!["-C", "/home/user/.ur/workspace/pool/ur/0", "status",]
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
        assert_eq!(result.args, vec!["-C", "/pool/ur/0", "log"]);
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
        assert_eq!(result.args, vec!["-C", "/pool/ur/0", "status"]);
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
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("does not match project key")
        );
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
        // NOTE: This test will fail at runtime until scripts are updated to
        // return structured tables (ur-ami7). It compiles correctly.
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
        assert_eq!(result.args, vec!["status"]);

        // gh.lua with agent context
        let gh_script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["pr".into(), "list".into()];
        let result = mgr
            .run_transform(gh_script, "gh", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result.args, vec!["pr", "list"]);
    }

    #[test]
    fn test_gh_dash_c_blocks_without_agent_context() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["-C".into(), "/workspace".into(), "pr".into(), "list".into()];
        let result = mgr.run_transform(script, "gh", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked flag: -C"));
    }

    #[test]
    fn test_gh_dash_c_rewrite_with_project_key() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
        };
        let args: Vec<String> = vec![
            "-C".into(),
            "/some/path/ur".into(),
            "pr".into(),
            "list".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(
            result.args,
            vec!["-C", "/home/user/.ur/workspace/pool/ur/0", "pr", "list"]
        );
    }

    #[test]
    fn test_gh_dash_c_rewrite_with_workspace() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
        };
        let args: Vec<String> = vec!["-C".into(), "/workspace".into(), "pr".into(), "list".into()];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(
            result.args,
            vec!["-C", "/home/user/.ur/workspace/pool/ur/0", "pr", "list"]
        );
    }

    #[test]
    fn test_gh_dash_c_rejected_wrong_project() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
        };
        let args: Vec<String> = vec!["-C".into(), "/tmp/evil".into(), "pr".into(), "list".into()];
        let result = mgr.run_transform(script, "gh", &args, "/workspace", Some(&ctx));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("does not match project key")
        );
    }

    #[test]
    fn test_gh_allows_normal_args() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["pr".into(), "list".into(), "--state".into(), "open".into()];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    // --- cargo.lua tests ---

    #[test]
    fn test_cargo_allows_normal_args() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec!["build".into(), "--release".into()];
        let result = mgr
            .run_transform(script, "cargo", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_cargo_allows_test() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec!["test".into(), "--workspace".into()];
        let result = mgr
            .run_transform(script, "cargo", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_cargo_blocks_install() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec!["install".into(), "ripgrep".into()];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked cargo subcommand: install")
        );
    }

    #[test]
    fn test_cargo_blocks_uninstall() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec!["uninstall".into(), "ripgrep".into()];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked cargo subcommand: uninstall")
        );
    }

    #[test]
    fn test_cargo_blocks_publish() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec!["publish".into()];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked cargo subcommand: publish")
        );
    }

    #[test]
    fn test_cargo_blocks_login() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec!["login".into()];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked cargo subcommand: login")
        );
    }

    #[test]
    fn test_cargo_blocks_yank() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec!["yank".into(), "--version".into(), "1.0.0".into()];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked cargo subcommand: yank")
        );
    }

    #[test]
    fn test_cargo_blocks_owner() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec!["owner".into(), "--add".into(), "user".into()];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked cargo subcommand: owner")
        );
    }

    #[test]
    fn test_cargo_blocks_manifest_path() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec![
            "build".into(),
            "--manifest-path".into(),
            "/tmp/evil/Cargo.toml".into(),
        ];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked flag: --manifest-path")
        );
    }

    #[test]
    fn test_cargo_blocks_manifest_path_equals() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec![
            "build".into(),
            "--manifest-path=/tmp/evil/Cargo.toml".into(),
        ];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked flag: --manifest-path=")
        );
    }

    #[test]
    fn test_cargo_dash_c_blocks_without_agent_context() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec!["-C".into(), "/workspace".into(), "build".into()];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked flag: -C"));
    }

    #[test]
    fn test_cargo_dash_c_rewrite_with_project_key() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
        };
        let args: Vec<String> = vec!["-C".into(), "/some/path/ur".into(), "build".into()];
        let result = mgr
            .run_transform(script, "cargo", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(
            result.args,
            vec!["-C", "/home/user/.ur/workspace/pool/ur/0", "build"]
        );
    }

    #[test]
    fn test_cargo_dash_c_rejected_wrong_project() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let ctx = AgentContext {
            agent_id: "deploy-x7q2".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
        };
        let args: Vec<String> = vec!["-C".into(), "/tmp/evil".into(), "build".into()];
        let result = mgr.run_transform(script, "cargo", &args, "/workspace", Some(&ctx));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("does not match project key")
        );
    }

    // --- TransformResult field tests ---

    #[test]
    fn test_env_extraction() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                return { command = c, args = a, working_dir = w, env = { FOO = "bar" } }
            end
        "#;
        let result = mgr
            .run_transform(script, "test", &["arg1".into()], "/tmp", None)
            .unwrap();
        assert_eq!(result.env.len(), 1);
        assert_eq!(result.env.get("FOO").unwrap(), "bar");
    }

    #[test]
    fn test_nil_env() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                return { command = c, args = a, working_dir = w }
            end
        "#;
        let result = mgr
            .run_transform(script, "test", &["arg1".into()], "/tmp", None)
            .unwrap();
        assert!(result.env.is_empty());
    }

    #[test]
    fn test_command_override() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                return { command = "overridden", args = a, working_dir = w }
            end
        "#;
        let result = mgr
            .run_transform(script, "original", &[], "/tmp", None)
            .unwrap();
        assert_eq!(result.command, "overridden");
        assert_ne!(result.command, "original");
    }

    #[test]
    fn test_working_dir_passthrough() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                return { command = c, args = a, working_dir = w }
            end
        "#;
        let result = mgr
            .run_transform(script, "test", &[], "/my/working/dir", None)
            .unwrap();
        assert_eq!(result.working_dir, "/my/working/dir");
    }

    #[test]
    fn test_missing_required_field_command() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                return { args = a, working_dir = w }
            end
        "#;
        let result = mgr.run_transform(script, "test", &[], "/tmp", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("command"));
    }

    #[test]
    fn test_wrong_env_type() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(c, a, w)
                return { command = c, args = a, working_dir = w, env = { FOO = { nested = "table" } } }
            end
        "#;
        let result = mgr.run_transform(script, "test", &[], "/tmp", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("env"));
    }

    #[test]
    fn test_git_lua_git_editor_env() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let result = mgr
            .run_transform(script, "git", &["status".into()], "/workspace", None)
            .unwrap();
        assert_eq!(result.env.get("GIT_EDITOR").unwrap(), "true");
    }

    #[test]
    fn test_cargo_allows_flags_before_subcommand() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/cargo.lua");
        let args: Vec<String> = vec![
            "--color".into(),
            "never".into(),
            "check".into(),
            "--message-format".into(),
            "short".into(),
        ];
        let result = mgr
            .run_transform(script, "cargo", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }
}
