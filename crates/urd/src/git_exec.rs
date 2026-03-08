use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Non-streaming git command result.
#[derive(Debug, Clone)]
pub struct GitResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Flags that could allow an agent to escape its repo directory.
const BLOCKED_FLAGS: &[&str] = &["--git-dir", "--work-tree"];

/// Blocked `-c` config keys (case-insensitive prefix match).
const BLOCKED_CONFIG_KEYS: &[&str] = &["core.worktree"];

/// In-memory map of process_id → repo directory (relative to workspace root).
/// TEMPORARY: will be replaced by CozoDB.
pub struct RepoRegistry {
    workspace: PathBuf,
    /// process_id → repo subdirectory name within workspace
    repos: RwLock<HashMap<String, String>>,
}

impl RepoRegistry {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            repos: RwLock::new(HashMap::new()),
        }
    }

    /// Register a process with its repo subdirectory within the workspace.
    pub fn register(&self, process_id: &str, repo_name: &str) {
        self.repos
            .write()
            .expect("repo registry lock poisoned")
            .insert(process_id.to_string(), repo_name.to_string());
    }

    /// Remove a process from the registry.
    pub fn unregister(&self, process_id: &str) {
        self.repos
            .write()
            .expect("repo registry lock poisoned")
            .remove(process_id);
    }

    /// Resolve a process_id to its full repo path within the workspace.
    pub(crate) fn resolve(&self, process_id: &str) -> Result<PathBuf, String> {
        let repos = self.repos.read().expect("repo registry lock poisoned");
        let repo_name = repos
            .get(process_id)
            .ok_or_else(|| format!("unknown process_id: {process_id}"))?;
        Ok(self.workspace.join(repo_name))
    }

    /// Sanitize, validate, and execute `git <args>` in the process's repo directory.
    pub async fn exec_git(&self, process_id: &str, args: &[String]) -> Result<GitResponse, String> {
        let repo_path = self.resolve(process_id)?;
        let args = sanitize_args(args);
        validate_args(&args)?;
        run_git(&repo_path, &args).await
    }
}

/// Strip `-C <path>` arguments, which are unnecessary since urd already
/// sets the working directory. Returns the filtered argument list.
pub(crate) fn sanitize_args(args: &[String]) -> Vec<String> {
    let mut result = Vec::with_capacity(args.len());
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-C" {
            // Skip -C and its value
            let _ = iter.next();
            continue;
        }
        result.push(arg.clone());
    }
    result
}

/// Reject args that could escape the repo sandbox.
pub(crate) fn validate_args(args: &[String]) -> Result<(), String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        // Block path-escaping flags
        for &flag in BLOCKED_FLAGS {
            if arg == flag {
                return Err(format!("blocked flag: {flag}"));
            }
            // Handle --flag=value form for long flags
            if flag.starts_with("--") && arg.starts_with(&format!("{flag}=")) {
                return Err(format!("blocked flag: {flag}"));
            }
        }

        // Block `-c core.worktree=...`
        if arg == "-c"
            && let Some(next) = iter.next()
        {
            check_config_key(&next.to_lowercase())?;
        }

        // Handle `-c<key>=<value>` (no space)
        if arg.starts_with("-c") && arg.len() > 2 {
            check_config_key(&arg[2..].to_lowercase())?;
        }
    }
    Ok(())
}

/// Check if a lowercased config value starts with any blocked key.
fn check_config_key(lowered: &str) -> Result<(), String> {
    for &key in BLOCKED_CONFIG_KEYS {
        if lowered.starts_with(key) {
            return Err(format!("blocked config key: {key}"));
        }
    }
    Ok(())
}

/// Run `git <args>` in the given directory via tokio::process::Command.
async fn run_git(repo_path: &Path, args: &[String]) -> Result<GitResponse, String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| format!("failed to spawn git: {e}"))?;

    Ok(GitResponse {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_allows_normal_args() {
        let args: Vec<String> = vec!["status".into()];
        assert!(validate_args(&args).is_ok());

        let args: Vec<String> = vec!["commit".into(), "-m".into(), "msg".into()];
        assert!(validate_args(&args).is_ok());

        let args: Vec<String> = vec!["log".into(), "--oneline".into(), "-10".into()];
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn sanitize_strips_dash_c_flag() {
        let args: Vec<String> = vec!["-C".into(), "/tmp".into(), "status".into()];
        let sanitized = sanitize_args(&args);
        assert_eq!(sanitized, vec!["status".to_string()]);
    }

    #[test]
    fn sanitize_strips_multiple_dash_c() {
        let args: Vec<String> = vec![
            "-C".into(),
            "/tmp".into(),
            "-C".into(),
            "/var".into(),
            "log".into(),
        ];
        let sanitized = sanitize_args(&args);
        assert_eq!(sanitized, vec!["log".to_string()]);
    }

    #[test]
    fn sanitize_preserves_other_args() {
        let args: Vec<String> = vec!["commit".into(), "-m".into(), "message".into()];
        let sanitized = sanitize_args(&args);
        assert_eq!(args, sanitized);
    }

    #[test]
    fn validate_blocks_git_dir() {
        let args: Vec<String> = vec!["--git-dir=/tmp/repo".into(), "status".into()];
        let err = validate_args(&args).unwrap_err();
        assert!(
            err.contains("--git-dir"),
            "error should mention --git-dir: {err}"
        );
    }

    #[test]
    fn validate_blocks_git_dir_separate() {
        let args: Vec<String> = vec!["--git-dir".into(), "/tmp/repo".into(), "status".into()];
        let err = validate_args(&args).unwrap_err();
        assert!(
            err.contains("--git-dir"),
            "error should mention --git-dir: {err}"
        );
    }

    #[test]
    fn validate_blocks_work_tree() {
        let args: Vec<String> = vec!["--work-tree".into(), "/tmp".into(), "log".into()];
        let err = validate_args(&args).unwrap_err();
        assert!(
            err.contains("--work-tree"),
            "error should mention --work-tree: {err}"
        );
    }

    #[test]
    fn validate_blocks_work_tree_equals() {
        let args: Vec<String> = vec!["--work-tree=/tmp".into(), "log".into()];
        let err = validate_args(&args).unwrap_err();
        assert!(
            err.contains("--work-tree"),
            "error should mention --work-tree: {err}"
        );
    }

    #[test]
    fn validate_blocks_config_core_worktree() {
        let args: Vec<String> = vec!["-c".into(), "core.worktree=/tmp".into(), "status".into()];
        let err = validate_args(&args).unwrap_err();
        assert!(
            err.contains("core.worktree"),
            "error should mention core.worktree: {err}"
        );
    }

    #[test]
    fn validate_blocks_config_core_worktree_no_space() {
        let args: Vec<String> = vec!["-ccore.worktree=/tmp".into(), "status".into()];
        let err = validate_args(&args).unwrap_err();
        assert!(
            err.contains("core.worktree"),
            "error should mention core.worktree: {err}"
        );
    }

    #[test]
    fn validate_blocks_config_core_worktree_case_insensitive() {
        let args: Vec<String> = vec!["-c".into(), "Core.Worktree=/tmp".into(), "status".into()];
        let err = validate_args(&args).unwrap_err();
        assert!(err.contains("core.worktree"));
    }

    #[test]
    fn validate_allows_other_config() {
        let args: Vec<String> = vec!["-c".into(), "user.name=Test".into(), "commit".into()];
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn registry_resolve_unknown_process() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        let err = reg.resolve("unknown").unwrap_err();
        assert!(err.contains("unknown process_id"));
    }

    #[test]
    fn registry_resolve_known_process() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        reg.register("p1", "my-repo");
        let path = reg.resolve("p1").unwrap();
        assert_eq!(path, PathBuf::from("/workspace/my-repo"));
    }

    #[test]
    fn registry_unregister() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        reg.register("p1", "my-repo");
        reg.unregister("p1");
        assert!(reg.resolve("p1").is_err());
    }

    #[tokio::test]
    async fn exec_git_unknown_process() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        let result = reg.exec_git("nope", &["status".into()]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown process_id"));
    }

    #[tokio::test]
    async fn exec_git_strips_dash_c_and_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_name = "test-repo";
        let repo_dir = tmp.path().join(repo_name);
        std::fs::create_dir_all(&repo_dir).unwrap();

        let init = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(init.status.success(), "git init failed");

        let reg = RepoRegistry::new(tmp.path().to_path_buf());
        reg.register("p1", repo_name);

        // -C /tmp should be stripped, leaving just "status"
        let resp = reg
            .exec_git("p1", &["-C".into(), "/tmp".into(), "status".into()])
            .await
            .unwrap();
        assert_eq!(resp.exit_code, 0);
    }

    #[tokio::test]
    async fn exec_git_blocks_git_dir() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        reg.register("p1", "repo");
        let result = reg
            .exec_git("p1", &["--git-dir".into(), "/tmp".into(), "status".into()])
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("--git-dir"));
    }

    #[tokio::test]
    async fn exec_git_runs_in_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_name = "test-repo";
        let repo_dir = tmp.path().join(repo_name);
        std::fs::create_dir_all(&repo_dir).unwrap();

        // Initialize a git repo so `git status` succeeds
        let init = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(init.status.success(), "git init failed");

        let reg = RepoRegistry::new(tmp.path().to_path_buf());
        reg.register("p1", repo_name);

        let resp = reg.exec_git("p1", &["status".into()]).await.unwrap();
        assert_eq!(resp.exit_code, 0);
        // git status output should contain common markers
        assert!(
            resp.stdout.contains("branch") || resp.stdout.contains("No commits"),
            "unexpected stdout: {}",
            resp.stdout
        );
    }
}
