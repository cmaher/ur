use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use mlua::{Lua, StdLib, Value};

/// Worker context passed to Lua transform functions when available.
///
/// Contains per-worker metadata (identity, project association, host repo path)
/// needed by transforms that perform per-worker logic (e.g., git -C rewriting).
/// `None` when no worker/project is associated (e.g., raw `-w` workspace mounts).
#[derive(Debug, Clone)]
pub struct WorkerContext {
    pub worker_id: String,
    pub process_id: String,
    pub project_key: String,
    pub slot_path: PathBuf,
    pub branch: String,
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
        worker_context: Option<&WorkerContext>,
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

        // Build worker_context Lua table (nil if no context available)
        let lua_worker_context = if let Some(ctx) = worker_context {
            let tbl = lua
                .create_table()
                .map_err(|e| anyhow::anyhow!("creating worker_context table: {e}"))?;
            tbl.set("worker_id", ctx.worker_id.as_str())
                .map_err(|e| anyhow::anyhow!("setting worker_id: {e}"))?;
            tbl.set("process_id", ctx.process_id.as_str())
                .map_err(|e| anyhow::anyhow!("setting process_id: {e}"))?;
            tbl.set("project_key", ctx.project_key.as_str())
                .map_err(|e| anyhow::anyhow!("setting project_key: {e}"))?;
            tbl.set("slot_path", ctx.slot_path.to_string_lossy().as_ref())
                .map_err(|e| anyhow::anyhow!("setting slot_path: {e}"))?;
            tbl.set("branch", ctx.branch.as_str())
                .map_err(|e| anyhow::anyhow!("setting branch: {e}"))?;
            Value::Table(tbl)
        } else {
            Value::Nil
        };

        let result = transform
            .call::<Value>((command, lua_args, working_dir, lua_worker_context))
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
    fn test_git_commit_prepends_ticket_id() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "deploy-x7q2".into(),
        };
        let args: Vec<String> = vec!["commit".into(), "-m".into(), "fix the bug".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result.args, vec!["commit", "-m", "[ur-abc12] fix the bug"]);
    }

    #[test]
    fn test_git_commit_no_double_prefix() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "deploy-x7q2".into(),
        };
        let args: Vec<String> = vec![
            "commit".into(),
            "-m".into(),
            "[ur-abc12] fix the bug".into(),
        ];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result.args, vec!["commit", "-m", "[ur-abc12] fix the bug"]);
    }

    #[test]
    fn test_git_commit_no_prefix_without_context() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec!["commit".into(), "-m".into(), "hello".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, vec!["commit", "-m", "hello"]);
    }

    #[test]
    fn test_git_commit_no_prefix_empty_process_id() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: String::new(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "deploy-x7q2".into(),
        };
        let args: Vec<String> = vec!["commit".into(), "-m".into(), "hello".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result.args, vec!["commit", "-m", "hello"]);
    }

    #[test]
    fn test_git_non_commit_not_modified() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "deploy-x7q2".into(),
        };
        let args: Vec<String> = vec!["push".into(), "origin".into(), "main".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result.args, vec!["push", "origin", "main"]);
    }

    #[test]
    fn test_git_blocks_no_verify_on_commit() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec![
            "commit".into(),
            "--no-verify".into(),
            "-m".into(),
            "msg".into(),
        ];
        let result = mgr.run_transform(script, "git", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked flag: --no-verify")
        );
    }

    #[test]
    fn test_git_blocks_no_verify_on_push() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec!["push".into(), "--no-verify".into()];
        let result = mgr.run_transform(script, "git", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked flag: --no-verify")
        );
    }

    #[test]
    fn test_git_allows_commit_without_no_verify() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec!["commit".into(), "-m".into(), "msg".into()];
        let result = mgr
            .run_transform(script, "git", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_git_blocks_no_verify_anywhere_in_args() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec![
            "push".into(),
            "origin".into(),
            "main".into(),
            "--no-verify".into(),
        ];
        let result = mgr.run_transform(script, "git", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked flag: --no-verify")
        );
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
    fn test_worker_context_available_in_lua() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(command, args, working_dir, worker_context)
                if worker_context == nil then
                    error("expected worker_context")
                end
                return {
                    command = command,
                    args = {
                        worker_context.worker_id,
                        worker_context.project_key,
                        worker_context.slot_path,
                    },
                    working_dir = working_dir,
                }
            end
        "#;
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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
    fn test_worker_context_branch_accessible_in_lua() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(command, args, working_dir, worker_context)
                return {
                    command = command,
                    args = { worker_context.branch },
                    working_dir = working_dir,
                }
            end
        "#;
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "ur-deploy-x7q2".into(),
        };
        let result = mgr
            .run_transform(script, "git", &[], "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result.args, vec!["ur-deploy-x7q2"]);
    }

    #[test]
    fn test_worker_context_nil_when_none() {
        let mgr = LuaTransformManager::new();
        let script = r#"
            function transform(command, args, working_dir, worker_context)
                if worker_context ~= nil then
                    error("expected nil worker_context")
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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
    fn test_git_dash_c_blocked_without_worker_context() {
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
        let ctx = WorkerContext {
            worker_id: "test-ab12".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "test-ab12".into(),
        };

        // git.lua with worker context
        let git_script = include_str!("default_scripts/git.lua");
        let args: Vec<String> = vec!["status".into()];
        let result = mgr
            .run_transform(git_script, "git", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result.args, vec!["status"]);

        // gh.lua with worker context
        let gh_script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["pr".into(), "list".into()];
        let result = mgr
            .run_transform(gh_script, "gh", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result.args, vec!["pr", "list"]);
    }

    #[test]
    fn test_gh_dash_c_blocks_without_worker_context() {
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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

    #[test]
    fn test_gh_blocks_pr_merge() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["pr".into(), "merge".into(), "123".into()];
        let result = mgr.run_transform(script, "gh", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not allowed (read-only access only)")
        );
    }

    #[test]
    fn test_gh_blocks_pr_merge_with_flags() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "-R".into(),
            "owner/repo".into(),
            "pr".into(),
            "merge".into(),
            "--squash".into(),
        ];
        let result = mgr.run_transform(script, "gh", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not allowed (read-only access only)")
        );
    }

    #[test]
    fn test_gh_allows_pr_create() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["pr".into(), "create".into(), "--title".into(), "foo".into()];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, vec!["pr", "create", "--title", "foo"]);
    }

    #[test]
    fn test_gh_blocks_pr_close() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["pr".into(), "close".into(), "123".into()];
        let result = mgr.run_transform(script, "gh", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not allowed (read-only access only)")
        );
    }

    #[test]
    fn test_gh_allows_pr_checks() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["pr".into(), "checks".into(), "123".into()];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_run_view() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["run".into(), "view".into(), "12345".into()];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_run_view_log_failed() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "run".into(),
            "view".into(),
            "12345".into(),
            "--log-failed".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_api_get() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["api".into(), "/repos/owner/repo/pulls".into()];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_blocks_api_post_non_comment() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "api".into(),
            "-X".into(),
            "POST".into(),
            "/repos/owner/repo/pulls".into(),
        ];
        let result = mgr.run_transform(script, "gh", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("only comment/review endpoints permitted")
        );
    }

    #[test]
    fn test_gh_blocks_api_delete() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "api".into(),
            "--method".into(),
            "DELETE".into(),
            "/repos/owner/repo/pulls/1".into(),
        ];
        let result = mgr.run_transform(script, "gh", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("DELETE method is not allowed")
        );
    }

    #[test]
    fn test_gh_blocks_api_method_equals_form_non_comment() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "api".into(),
            "--method=PATCH".into(),
            "/repos/owner/repo/pulls/1".into(),
        ];
        let result = mgr.run_transform(script, "gh", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("only comment/review endpoints permitted")
        );
    }

    #[test]
    fn test_gh_blocks_unknown_top_level_command() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["issue".into(), "create".into()];
        let result = mgr.run_transform(script, "gh", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("is not allowed"));
    }

    #[test]
    fn test_gh_allows_pr_comment() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "pr".into(),
            "comment".into(),
            "123".into(),
            "--body".into(),
            "LGTM".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_pr_edit() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "pr".into(),
            "edit".into(),
            "123".into(),
            "--title".into(),
            "new title".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_api_post_issue_comments() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "api".into(),
            "-X".into(),
            "POST".into(),
            "/repos/owner/repo/issues/42/comments".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_api_post_pr_comments() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "api".into(),
            "-X".into(),
            "POST".into(),
            "/repos/owner/repo/pulls/7/comments".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_api_post_pr_review_comments() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "api".into(),
            "-X".into(),
            "POST".into(),
            "/repos/owner/repo/pulls/7/reviews/99/comments".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_api_patch_issue_comment() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "api".into(),
            "--method=PATCH".into(),
            "/repos/owner/repo/issues/comments/456".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_api_patch_pr_comment() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "api".into(),
            "--method=PATCH".into(),
            "/repos/owner/repo/pulls/comments/789".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_api_post_pr_reviews() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec![
            "api".into(),
            "-X".into(),
            "POST".into(),
            "/repos/owner/repo/pulls/7/reviews".into(),
        ];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_run_list() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["run".into(), "list".into()];
        let result = mgr
            .run_transform(script, "gh", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_gh_allows_pr_view() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/gh.lua");
        let args: Vec<String> = vec!["pr".into(), "view".into(), "123".into()];
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
    fn test_cargo_dash_c_blocks_without_worker_context() {
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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
        let ctx = WorkerContext {
            worker_id: "deploy-x7q2".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: PathBuf::from("/pool/ur/0"),
            branch: "deploy-x7q2".into(),
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

    // --- docker.lua tests ---

    #[test]
    fn test_docker_allows_ps() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["ps".into(), "-a".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_images() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["images".into(), "--format".into(), "json".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_inspect() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["inspect".into(), "my-container".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_logs() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec![
            "logs".into(),
            "--tail".into(),
            "100".into(),
            "my-ctr".into(),
        ];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_version() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["version".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_info() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["info".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_container_ls() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["container".into(), "ls".into(), "-a".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_container_inspect() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["container".into(), "inspect".into(), "my-ctr".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_container_logs() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["container".into(), "logs".into(), "my-ctr".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_image_ls() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["image".into(), "ls".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_network_inspect() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["network".into(), "inspect".into(), "bridge".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_volume_ls() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["volume".into(), "ls".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_compose_ps() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["compose".into(), "ps".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_compose_logs() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["compose".into(), "logs".into(), "-f".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_system_df() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["system".into(), "df".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_stats() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["stats".into(), "--no-stream".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_blocks_run() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["run".into(), "--rm".into(), "alpine".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: run")
        );
    }

    #[test]
    fn test_docker_blocks_exec() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["exec".into(), "my-ctr".into(), "bash".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: exec")
        );
    }

    #[test]
    fn test_docker_blocks_rm() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["rm".into(), "my-ctr".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: rm")
        );
    }

    #[test]
    fn test_docker_blocks_rmi() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["rmi".into(), "alpine:latest".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: rmi")
        );
    }

    #[test]
    fn test_docker_blocks_build() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["build".into(), ".".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: build")
        );
    }

    #[test]
    fn test_docker_blocks_stop() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["stop".into(), "my-ctr".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: stop")
        );
    }

    #[test]
    fn test_docker_blocks_kill() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["kill".into(), "my-ctr".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: kill")
        );
    }

    #[test]
    fn test_docker_blocks_pull() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["pull".into(), "alpine:latest".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: pull")
        );
    }

    #[test]
    fn test_docker_blocks_push() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["push".into(), "myimage:latest".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: push")
        );
    }

    #[test]
    fn test_docker_blocks_container_rm() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["container".into(), "rm".into(), "my-ctr".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: container rm")
        );
    }

    #[test]
    fn test_docker_blocks_container_exec() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["container".into(), "exec".into(), "my-ctr".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: container exec")
        );
    }

    #[test]
    fn test_docker_blocks_container_run() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["container".into(), "run".into(), "alpine".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: container run")
        );
    }

    #[test]
    fn test_docker_blocks_compose_up() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["compose".into(), "up".into(), "-d".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: compose up")
        );
    }

    #[test]
    fn test_docker_blocks_compose_down() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["compose".into(), "down".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: compose down")
        );
    }

    #[test]
    fn test_docker_blocks_network_create() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["network".into(), "create".into(), "my-net".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: network create")
        );
    }

    #[test]
    fn test_docker_blocks_volume_rm() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["volume".into(), "rm".into(), "my-vol".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: volume rm")
        );
    }

    #[test]
    fn test_docker_blocks_image_rm() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["image".into(), "rm".into(), "alpine".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: image rm")
        );
    }

    #[test]
    fn test_docker_allows_flags_before_command() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec![
            "-H".into(),
            "unix:///var/run/docker.sock".into(),
            "ps".into(),
            "-a".into(),
        ];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_blocks_run_with_flags_before() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec![
            "-H".into(),
            "unix:///var/run/docker.sock".into(),
            "run".into(),
            "alpine".into(),
        ];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: run")
        );
    }

    #[test]
    fn test_docker_allows_bare_invocation() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec![];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_allows_bare_management_command() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        // `docker container` with no subcommand prints help
        let args: Vec<String> = vec!["container".into()];
        let result = mgr
            .run_transform(script, "docker", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_docker_blocks_system_prune() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/docker.lua");
        let args: Vec<String> = vec!["system".into(), "prune".into(), "-a".into()];
        let result = mgr.run_transform(script, "docker", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked docker command: system prune")
        );
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

    // ── ur.lua tests ──

    #[test]
    fn test_ur_allows_all_ticket_subcommands() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        for sub in &[
            "create",
            "list",
            "show",
            "update",
            "set-meta",
            "delete-meta",
            "add-activity",
            "list-activities",
            "add-block",
            "remove-block",
            "add-link",
            "remove-link",
            "dispatchable",
            "status",
        ] {
            let args: Vec<String> = vec!["ticket".into(), (*sub).into(), "ur-abc12".into()];
            let result = mgr
                .run_transform(script, "ur", &args, "/workspace", None)
                .unwrap();
            assert_eq!(
                result.args, args,
                "ticket subcommand '{sub}' should be allowed"
            );
        }
    }

    #[test]
    fn test_ur_allows_readonly_worker_commands() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        for sub in &["list", "describe", "dir"] {
            let args: Vec<String> = vec!["worker".into(), (*sub).into()];
            let result = mgr
                .run_transform(script, "ur", &args, "/workspace", None)
                .unwrap();
            assert_eq!(result.args, args, "worker {sub} should be allowed");
        }
    }

    #[test]
    fn test_ur_blocks_worker_launch() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        let args: Vec<String> = vec!["worker".into(), "launch".into(), "test-1".into()];
        let result = mgr.run_transform(script, "ur", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not allowed"));
    }

    #[test]
    fn test_ur_blocks_start_stop() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        for sub in &["server", "init"] {
            let args: Vec<String> = vec![(*sub).into()];
            let result = mgr.run_transform(script, "ur", &args, "/workspace", None);
            assert!(result.is_err(), "ur {sub} should be blocked");
        }
    }

    #[test]
    fn test_ur_blocks_no_subcommand() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        let args: Vec<String> = vec![];
        let result = mgr.run_transform(script, "ur", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("subcommand required")
        );
    }

    #[test]
    fn test_ur_allows_readonly_project_list() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        let args: Vec<String> = vec!["project".into(), "list".into()];
        let result = mgr
            .run_transform(script, "ur", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_ur_blocks_project_add() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        let args: Vec<String> = vec!["project".into(), "add".into(), ".".into()];
        let result = mgr.run_transform(script, "ur", &args, "/workspace", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not allowed"));
    }

    #[test]
    fn test_ur_allows_bare_management_command() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        // `ur worker` with no subcommand prints help
        let args: Vec<String> = vec!["worker".into()];
        let result = mgr
            .run_transform(script, "ur", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args);
    }

    #[test]
    fn test_ur_ticket_sets_project_env_from_worker_context() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        let ctx = WorkerContext {
            worker_id: "w-1".into(),
            process_id: "ur-abc12".into(),
            project_key: "ur".into(),
            slot_path: "/pool/ur/0".into(),
            branch: "w-1".into(),
        };
        let args: Vec<String> = vec!["ticket".into(), "list".into()];
        let result = mgr
            .run_transform(script, "ur", &args, "/workspace", Some(&ctx))
            .unwrap();
        assert_eq!(result.args, args, "args should be unchanged");
        assert_eq!(
            result.env.get("UR_PROJECT").map(String::as_str),
            Some("ur"),
            "UR_PROJECT env var should be set from worker context"
        );
    }

    #[test]
    fn test_ur_ticket_no_project_env_without_worker_context() {
        let mgr = LuaTransformManager::new();
        let script = include_str!("default_scripts/ur.lua");
        let args: Vec<String> = vec!["ticket".into(), "show".into(), "ur-abc12".into()];
        let result = mgr
            .run_transform(script, "ur", &args, "/workspace", None)
            .unwrap();
        assert_eq!(result.args, args, "args should be unchanged");
        assert!(
            !result.env.contains_key("UR_PROJECT"),
            "no UR_PROJECT without worker_context"
        );
    }
}
