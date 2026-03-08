use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tokio::task::JoinHandle;
use tracing::info;

use crate::RepoRegistry;

/// Tracks a running agent process.
struct ProcessEntry {
    container_id: String,
    /// Host-side TCP port the per-agent gRPC server is bound to.
    grpc_port: u16,
    /// Handle to the per-agent gRPC server task.
    server_handle: JoinHandle<()>,
}

/// Orchestrates the full lifecycle of agent processes:
/// per-agent gRPC server (TCP), repo registration, git init, container run/stop.
#[derive(Clone)]
pub struct ProcessManager {
    workspace: PathBuf,
    repo_registry: Arc<RepoRegistry>,
    processes: Arc<RwLock<HashMap<String, ProcessEntry>>>,
}

impl ProcessManager {
    pub fn new(workspace: PathBuf, repo_registry: Arc<RepoRegistry>) -> Self {
        Self {
            workspace,
            repo_registry,
            processes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Phase 1 of launch: create repo dir, git init, register in RepoRegistry.
    /// The caller is responsible for spawning the per-agent gRPC server and
    /// then calling `run_and_record`.
    pub async fn prepare(&self, process_id: &str) -> Result<(), String> {
        // Check for duplicate
        {
            let procs = self.processes.read().expect("process lock poisoned");
            if procs.contains_key(process_id) {
                return Err(format!("process already running: {process_id}"));
            }
        }

        // 1. Create repo dir
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

        Ok(())
    }

    /// Phase 2 of launch: run the container and record the process entry.
    /// Call after spawning the per-agent gRPC server.
    ///
    /// `grpc_port` is the host-side TCP port the per-agent gRPC server is bound to.
    /// `host_ip` is the host gateway IP the container uses to connect back.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_and_record(
        &self,
        process_id: &str,
        image_id: &str,
        cpus: u32,
        memory: &str,
        grpc_port: u16,
        host_ip: &str,
        server_handle: JoinHandle<()>,
    ) -> Result<String, String> {
        // Run the container (scoped so rt is dropped before any subsequent awaits)
        let cid = {
            let rt = container::runtime_from_env();
            let container_name = format!("ur-agent-{process_id}");
            let opts = container::RunOpts {
                image: container::ImageId(image_id.to_string()),
                name: container_name,
                cpus,
                memory: memory.to_string(),
                volumes: vec![],
                port_maps: vec![],
                env_vars: vec![
                    (ur_config::UR_GRPC_HOST_ENV.into(), host_ip.to_string()),
                    (ur_config::UR_GRPC_PORT_ENV.into(), grpc_port.to_string()),
                ],
                workdir: Some(PathBuf::from("/workspace")),
                command: vec![],
            };
            rt.run(&opts).map_err(|e| e.to_string())?
        };

        info!(
            process_id,
            container_id = cid.0,
            grpc_port,
            "process launched"
        );

        // Record in process map
        {
            let mut procs = self.processes.write().expect("process lock poisoned");
            procs.insert(
                process_id.to_string(),
                ProcessEntry {
                    container_id: cid.0.clone(),
                    grpc_port,
                    server_handle,
                },
            );
        }

        Ok(cid.0)
    }

    /// Stop a running agent process. Stops + removes the container,
    /// unregisters from RepoRegistry, aborts the per-agent gRPC server task.
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

        // 3. Abort the per-agent gRPC server task
        entry.server_handle.abort();

        info!(process_id, grpc_port = entry.grpc_port, "process stopped");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager() -> (ProcessManager, tempfile::TempDir) {
        let workspace = tempfile::tempdir().unwrap();
        let registry = Arc::new(RepoRegistry::new(workspace.path().to_path_buf()));
        let mgr = ProcessManager::new(workspace.path().to_path_buf(), registry);
        (mgr, workspace)
    }

    #[tokio::test]
    async fn prepare_creates_repo_and_registers() {
        let (mgr, workspace) = test_manager();
        let process_id = "test-proc";

        mgr.prepare(process_id).await.unwrap();

        // Verify repo dir exists and has .git
        let repo_dir = workspace.path().join(process_id);
        assert!(repo_dir.join(".git").exists());

        // Verify registry resolves and git works
        let resp = mgr
            .repo_registry
            .exec_git(process_id, &["status".into()])
            .await;
        assert!(resp.is_ok());
    }

    #[tokio::test]
    async fn prepare_duplicate_returns_error() {
        let (mgr, _workspace) = test_manager();

        // Manually insert a process entry
        let noop_handle = tokio::spawn(std::future::ready(()));
        {
            let mut procs = mgr.processes.write().unwrap();
            procs.insert(
                "dup-proc".into(),
                ProcessEntry {
                    container_id: "fake-cid".into(),
                    grpc_port: 0,
                    server_handle: noop_handle,
                },
            );
        }

        let result = mgr.prepare("dup-proc").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already running"));
    }

    #[tokio::test]
    async fn stop_unknown_process_returns_error() {
        let (mgr, _workspace) = test_manager();
        let result = mgr.stop("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown process"));
    }
}
