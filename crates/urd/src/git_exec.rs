use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use tokio::io::AsyncReadExt;
use tracing::warn;
use ur_rpc::stream::{accept_stream_sink, bind_stream_listener, send_output};
use ur_rpc::{CommandOutput, GitResponse, StreamingExecResponse};

/// Flags that could allow an agent to escape its repo directory.
const BLOCKED_FLAGS: &[&str] = &["-C", "--git-dir", "--work-tree"];

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
    fn resolve(&self, process_id: &str) -> Result<PathBuf, String> {
        let repos = self.repos.read().expect("repo registry lock poisoned");
        let repo_name = repos
            .get(process_id)
            .ok_or_else(|| format!("unknown process_id: {process_id}"))?;
        Ok(self.workspace.join(repo_name))
    }

    /// Validate git args and execute `git <args>` in the process's repo directory.
    pub async fn exec_git(&self, process_id: &str, args: &[String]) -> Result<GitResponse, String> {
        let repo_path = self.resolve(process_id)?;
        validate_args(args)?;
        run_git(&repo_path, args).await
    }

    /// Streaming variant: spawns git, creates a side-channel Unix socket,
    /// and streams stdout/stderr chunks over it.
    pub async fn exec_git_stream(
        &self,
        socket_dir: &Path,
        process_id: &str,
        args: &[String],
    ) -> Result<StreamingExecResponse, String> {
        let repo_path = self.resolve(process_id)?;
        validate_args(args)?;
        run_git_stream(socket_dir, &repo_path, args).await
    }
}

/// Reject args that could escape the repo sandbox.
fn validate_args(args: &[String]) -> Result<(), String> {
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

/// Streaming variant of `run_git`: spawns git with piped stdout/stderr,
/// creates a per-command Unix socket, and sends `CommandOutput` frames
/// as data arrives. Returns the socket path for the client to connect to.
async fn run_git_stream(
    socket_dir: &Path,
    repo_path: &Path,
    args: &[String],
) -> Result<StreamingExecResponse, String> {
    use std::process::Stdio;
    use tokio::process::Command;

    let socket_path = socket_dir.join(format!("s-{}.sock", short_id()));

    let mut child = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn git: {e}"))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    // Return just the filename — the client resolves it relative to the
    // directory containing its control socket (which is the same directory
    // on the host; inside containers it's the mounted dir).
    let socket_filename = socket_path
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| "non-UTF-8 socket path".to_string())?
        .to_string();

    // Bind the listener *before* returning so the client can connect immediately.
    let listener = bind_stream_listener(&socket_path)
        .map_err(|e| format!("failed to bind stream socket: {e}"))?;

    // Spawn a background task that accepts a single client, then streams output.
    let sp = socket_path.clone();
    tokio::spawn(async move {
        let mut sink = match accept_stream_sink(listener).await {
            Ok(s) => s,
            Err(e) => {
                warn!("stream accept failed: {e}");
                return;
            }
        };

        // Read stdout and stderr concurrently, sending chunks as they arrive.
        let mut stdout = stdout;
        let mut stderr = stderr;
        let mut stdout_buf = vec![0u8; 8192];
        let mut stderr_buf = vec![0u8; 8192];
        let mut stdout_done = false;
        let mut stderr_done = false;

        loop {
            if stdout_done && stderr_done {
                break;
            }

            tokio::select! {
                n = stdout.read(&mut stdout_buf), if !stdout_done => {
                    match n {
                        Ok(0) => stdout_done = true,
                        Ok(n) => {
                            if let Err(e) = send_output(
                                &mut sink,
                                CommandOutput::Stdout(stdout_buf[..n].to_vec()),
                            ).await {
                                warn!("stream send stdout failed: {e}");
                                return;
                            }
                        }
                        Err(e) => {
                            warn!("stream read stdout failed: {e}");
                            stdout_done = true;
                        }
                    }
                }
                n = stderr.read(&mut stderr_buf), if !stderr_done => {
                    match n {
                        Ok(0) => stderr_done = true,
                        Ok(n) => {
                            if let Err(e) = send_output(
                                &mut sink,
                                CommandOutput::Stderr(stderr_buf[..n].to_vec()),
                            ).await {
                                warn!("stream send stderr failed: {e}");
                                return;
                            }
                        }
                        Err(e) => {
                            warn!("stream read stderr failed: {e}");
                            stderr_done = true;
                        }
                    }
                }
            }
        }

        // Wait for exit code
        let exit_code = match child.wait().await {
            Ok(status) => status.code().unwrap_or(-1),
            Err(e) => {
                warn!("stream wait failed: {e}");
                -1
            }
        };

        let _ = send_output(&mut sink, CommandOutput::Exit(exit_code)).await;

        // Clean up the socket file
        let _ = tokio::fs::remove_file(&sp).await;
    });

    Ok(StreamingExecResponse {
        stream_socket: socket_filename,
    })
}

/// Generate a short unique hex ID for stream socket names.
fn short_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static CTR: AtomicU32 = AtomicU32::new(0);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32;
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    format!("{t:08x}{c:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect all stdout chunks and the exit code from a command stream.
    async fn collect_stream(
        stream: &mut ur_rpc::stream::CommandStream,
    ) -> (Vec<Vec<u8>>, Option<i32>) {
        use ur_rpc::stream::recv_output;
        let mut stdout_chunks = Vec::new();
        let mut exit_code = None;
        while let Some(output) = recv_output(stream).await {
            match output.unwrap() {
                CommandOutput::Stdout(data) => stdout_chunks.push(data),
                CommandOutput::Stderr(_) => {}
                CommandOutput::Exit(code) => exit_code = Some(code),
            }
        }
        (stdout_chunks, exit_code)
    }

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
    fn validate_blocks_dash_c_flag() {
        let args: Vec<String> = vec!["-C".into(), "/tmp".into(), "status".into()];
        let err = validate_args(&args).unwrap_err();
        assert!(err.contains("-C"), "error should mention -C: {err}");
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
    async fn exec_git_blocked_flag() {
        let reg = RepoRegistry::new(PathBuf::from("/workspace"));
        reg.register("p1", "repo");
        let result = reg
            .exec_git("p1", &["-C".into(), "/tmp".into(), "status".into()])
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("-C"));
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

    #[tokio::test]
    async fn exec_git_stream_delivers_chunks() {
        use ur_rpc::stream::connect_stream;

        let tmp = tempfile::tempdir().unwrap();
        let repo_name = "stream-repo";
        let repo_dir = tmp.path().join(repo_name);
        std::fs::create_dir_all(&repo_dir).unwrap();

        let init = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(init.status.success(), "git init failed");

        let socket_tmp = tempfile::tempdir().unwrap();
        let socket_dir = socket_tmp.path().to_path_buf();

        let reg = RepoRegistry::new(tmp.path().to_path_buf());
        reg.register("p1", repo_name);

        let resp = reg
            .exec_git_stream(&socket_dir, "p1", &["status".into()])
            .await
            .unwrap();

        // Stream socket is returned as a filename; resolve relative to socket_dir
        let stream_path = socket_dir.join(&resp.stream_socket);
        let mut stream = connect_stream(&stream_path).await.unwrap();
        let (stdout_chunks, exit_code) = collect_stream(&mut stream).await;

        assert_eq!(exit_code, Some(0), "git status should exit 0");
        let all_stdout: Vec<u8> = stdout_chunks.into_iter().flatten().collect();
        let stdout_str = String::from_utf8_lossy(&all_stdout);
        assert!(
            stdout_str.contains("branch") || stdout_str.contains("No commits"),
            "unexpected streaming stdout: {stdout_str}"
        );
    }

    #[tokio::test]
    async fn exec_git_stream_unknown_process() {
        let tmp = tempfile::tempdir().unwrap();
        let socket_dir = tmp.path().join("sockets");
        std::fs::create_dir_all(&socket_dir).unwrap();

        let reg = RepoRegistry::new(tmp.path().to_path_buf());
        let result = reg
            .exec_git_stream(&socket_dir, "nope", &["status".into()])
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown process_id"));
    }

    #[tokio::test]
    async fn exec_git_stream_blocked_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let socket_dir = tmp.path().join("sockets");
        std::fs::create_dir_all(&socket_dir).unwrap();

        let reg = RepoRegistry::new(tmp.path().to_path_buf());
        reg.register("p1", "repo");
        let result = reg
            .exec_git_stream(
                &socket_dir,
                "p1",
                &["-C".into(), "/tmp".into(), "status".into()],
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("-C"));
    }
}
