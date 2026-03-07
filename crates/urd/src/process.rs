use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use tokio::task::JoinHandle;
use tracing::info;

use crate::RepoRegistry;

/// Tracks a running agent process.
struct ProcessEntry {
    container_id: String,
    socket_path: PathBuf,
    /// Handle to the per-agent accept_loop task.
    accept_handle: JoinHandle<()>,
}

/// Orchestrates the full lifecycle of agent processes:
/// per-agent socket, repo registration, git init, container run/stop.
#[derive(Clone)]
pub struct ProcessManager {
    config_dir: PathBuf,
    workspace: PathBuf,
    repo_registry: Arc<RepoRegistry>,
    processes: Arc<RwLock<HashMap<String, ProcessEntry>>>,
}

impl ProcessManager {
    pub fn new(config_dir: PathBuf, workspace: PathBuf, repo_registry: Arc<RepoRegistry>) -> Self {
        Self {
            config_dir,
            workspace,
            repo_registry,
            processes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Per-agent socket directory. All sockets for this process (control + stream)
    /// live here. This directory is mounted into the container.
    pub fn socket_dir(&self, process_id: &str) -> PathBuf {
        self.config_dir.join(process_id)
    }

    /// Path to the per-agent control socket.
    pub fn socket_path(&self, process_id: &str) -> PathBuf {
        self.socket_dir(process_id).join("ur.sock")
    }

    /// Phase 1 of launch: create repo dir, git init, register in RepoRegistry.
    /// Returns the per-agent socket path. The caller is responsible for
    /// spawning the accept_loop and then calling `run_and_record`.
    pub async fn prepare(&self, process_id: &str) -> Result<PathBuf, String> {
        // Check for duplicate
        {
            let procs = self.processes.read().expect("process lock poisoned");
            if procs.contains_key(process_id) {
                return Err(format!("process already running: {process_id}"));
            }
        }

        // 1. Create per-agent socket dir + repo dir
        let socket_dir = self.socket_dir(process_id);
        tokio::fs::create_dir_all(&socket_dir)
            .await
            .map_err(|e| format!("failed to create socket dir: {e}"))?;

        let repo_dir = self.workspace.join(process_id);
        tokio::fs::create_dir_all(&repo_dir)
            .await
            .map_err(|e| format!("failed to create repo dir: {e}"))?;

        let git_init = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(&repo_dir)
            .output()
            .await
            .map_err(|e| format!("failed to run git init: {e}"))?;
        if !git_init.status.success() {
            return Err(format!(
                "git init failed: {}",
                String::from_utf8_lossy(&git_init.stderr)
            ));
        }

        // 2. Register in RepoRegistry
        self.repo_registry.register(process_id, process_id);

        Ok(self.socket_path(process_id))
    }

    /// Phase 2 of launch: wait for the per-agent socket, run the container,
    /// and record the process entry. Call after spawning the accept_loop.
    pub async fn run_and_record(
        &self,
        process_id: &str,
        image_id: &str,
        cpus: u32,
        memory: &str,
        socket_path: PathBuf,
        accept_handle: JoinHandle<()>,
    ) -> Result<String, String> {
        // Wait for the socket file to appear
        wait_for_socket(&socket_path).await?;

        // Run the container (scoped so rt is dropped before any subsequent awaits)
        let cid = {
            let rt = container::runtime_from_env();
            let container_name = format!("ur-agent-{process_id}");
            // Mount the per-agent socket directory so the control socket and
            // any stream sockets created during exec are visible inside.
            let socket_dir = socket_path
                .parent()
                .expect("socket_path must have a parent dir")
                .to_path_buf();
            let opts = container::RunOpts {
                image: container::ImageId(image_id.to_string()),
                name: container_name,
                cpus,
                memory: memory.to_string(),
                volumes: vec![(socket_dir, PathBuf::from("/var/run/ur"))],
                socket_mounts: vec![],
                workdir: Some(PathBuf::from("/workspace")),
                command: vec![],
            };
            rt.run(&opts).map_err(|e| e.to_string())?
        };

        info!(process_id, container_id = cid.0, "process launched");

        // Record in process map
        {
            let mut procs = self.processes.write().expect("process lock poisoned");
            procs.insert(
                process_id.to_string(),
                ProcessEntry {
                    container_id: cid.0.clone(),
                    socket_path,
                    accept_handle,
                },
            );
        }

        Ok(cid.0)
    }

    /// Stop a running agent process. Stops + removes the container,
    /// unregisters from RepoRegistry, tears down the per-agent socket.
    pub async fn stop(&self, process_id: &str) -> Result<(), String> {
        let entry = {
            let mut procs = self.processes.write().expect("process lock poisoned");
            procs
                .remove(process_id)
                .ok_or_else(|| format!("unknown process: {process_id}"))?
        };

        // 1. Stop + remove container (scoped so rt is dropped before await)
        {
            let rt = container::runtime_from_env();
            let cid = container::ContainerId(entry.container_id);
            rt.stop(&cid).map_err(|e| e.to_string())?;
            rt.rm(&cid).map_err(|e| e.to_string())?;
        }

        // 2. Unregister from RepoRegistry
        self.repo_registry.unregister(process_id);

        // 3. Abort the accept_loop task
        entry.accept_handle.abort();

        // 4. Remove the socket directory (contains control + stream sockets)
        if let Some(socket_dir) = entry.socket_path.parent() {
            let _ = tokio::fs::remove_dir_all(socket_dir).await;
        }

        info!(process_id, "process stopped");

        Ok(())
    }
}

/// Poll for a socket file to appear, with a 5-second timeout.
async fn wait_for_socket(path: &Path) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    while !path.exists() {
        if tokio::time::Instant::now() > deadline {
            return Err("per-agent socket did not appear within 5s".into());
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager() -> (ProcessManager, tempfile::TempDir, tempfile::TempDir) {
        let config_dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let registry = Arc::new(RepoRegistry::new(workspace.path().to_path_buf()));
        let mgr = ProcessManager::new(
            config_dir.path().to_path_buf(),
            workspace.path().to_path_buf(),
            registry,
        );
        (mgr, config_dir, workspace)
    }

    /// Fake accept_loop that creates the socket file and waits until aborted.
    fn fake_accept_loop(socket_path: &Path) -> JoinHandle<()> {
        let socket_path = socket_path.to_path_buf();
        tokio::spawn(async move {
            let _listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        })
    }

    #[tokio::test]
    async fn socket_path_uses_process_id() {
        let (mgr, _config_dir, _workspace) = test_manager();
        let path = mgr.socket_path("my-agent");
        assert!(path.to_str().unwrap().ends_with("my-agent/ur.sock"));
    }

    #[tokio::test]
    async fn prepare_creates_repo_and_registers() {
        let (mgr, _config_dir, workspace) = test_manager();
        let process_id = "test-proc";

        let socket_path = mgr.prepare(process_id).await.unwrap();

        // Verify repo dir exists and has .git
        let repo_dir = workspace.path().join(process_id);
        assert!(repo_dir.join(".git").exists());

        // Verify registry resolves and git works
        let resp = mgr
            .repo_registry
            .exec_git(process_id, &["status".into()])
            .await;
        assert!(resp.is_ok());

        // Verify socket path is in per-process directory
        assert!(socket_path.to_str().unwrap().ends_with("test-proc/ur.sock"));
    }

    #[tokio::test]
    async fn prepare_and_accept_loop() {
        let (mgr, _config_dir, _workspace) = test_manager();
        let process_id = "socket-test";

        let socket_path = mgr.prepare(process_id).await.unwrap();
        let handle = fake_accept_loop(&socket_path);
        wait_for_socket(&socket_path).await.unwrap();
        assert!(socket_path.exists());

        handle.abort();
    }

    #[tokio::test]
    async fn prepare_duplicate_returns_error() {
        let (mgr, _config_dir, _workspace) = test_manager();

        // Manually insert a process entry
        let noop_handle = tokio::spawn(std::future::ready(()));
        {
            let mut procs = mgr.processes.write().unwrap();
            procs.insert(
                "dup-proc".into(),
                ProcessEntry {
                    container_id: "fake-cid".into(),
                    socket_path: PathBuf::from("/tmp/fake.sock"),
                    accept_handle: noop_handle,
                },
            );
        }

        let result = mgr.prepare("dup-proc").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already running"));
    }

    #[tokio::test]
    async fn stop_unknown_process_returns_error() {
        let (mgr, _config_dir, _workspace) = test_manager();
        let result = mgr.stop("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown process"));
    }
}
