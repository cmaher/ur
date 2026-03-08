use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tokio::task::JoinHandle;
use tracing::info;

use crate::RepoRegistry;
use crate::credential::CredentialManager;

/// Tracks a running agent process.
struct ProcessEntry {
    container_id: String,
    /// Host-side TCP port the per-agent gRPC server is bound to.
    grpc_port: u16,
    /// Handle to the per-agent gRPC server task.
    server_handle: JoinHandle<()>,
    /// Whether the repo is externally managed (mounted workspace, not git-init'd by urd).
    #[allow(dead_code)]
    externally_managed: bool,
}

/// Configuration for launching a container process.
pub struct ProcessConfig {
    pub process_id: String,
    pub image_id: String,
    pub cpus: u32,
    pub memory: String,
    pub grpc_port: u16,
    pub host_ip: String,
    pub workspace_dir: Option<PathBuf>,
}

/// Orchestrates the full lifecycle of agent processes:
/// per-agent gRPC server (TCP), repo registration, git init, container run/stop.
#[derive(Clone)]
pub struct ProcessManager {
    workspace: PathBuf,
    repo_registry: Arc<RepoRegistry>,
    credential_manager: CredentialManager,
    processes: Arc<RwLock<HashMap<String, ProcessEntry>>>,
}

impl ProcessManager {
    pub fn new(
        workspace: PathBuf,
        repo_registry: Arc<RepoRegistry>,
        credential_manager: CredentialManager,
    ) -> Self {
        Self {
            workspace,
            repo_registry,
            credential_manager,
            processes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Phase 1 of launch: create repo dir, git init, register in RepoRegistry.
    /// When `workspace_dir` is Some, the directory is used as-is (no git init)
    /// and registered via `register_absolute`.
    /// The caller is responsible for spawning the per-agent gRPC server and
    /// then calling `run_and_record`.
    pub async fn prepare(
        &self,
        process_id: &str,
        workspace_dir: Option<PathBuf>,
    ) -> Result<(), String> {
        // Check for duplicate
        {
            let procs = self.processes.read().expect("process lock poisoned");
            if procs.contains_key(process_id) {
                return Err(format!("process already running: {process_id}"));
            }
        }

        if let Some(ws_dir) = workspace_dir {
            // External workspace: register the absolute path directly, skip git init
            self.repo_registry.register_absolute(process_id, ws_dir);
        } else {
            // Default: create repo dir and git init
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

            self.repo_registry.register(process_id, process_id);
        }

        Ok(())
    }

    /// Phase 2 of launch: run the container and record the process entry.
    /// Call after spawning the per-agent gRPC server.
    pub async fn run_and_record(
        &self,
        config: ProcessConfig,
        server_handle: JoinHandle<()>,
    ) -> Result<String, String> {
        let urd_addr = format!("{}:{}", config.host_ip, config.grpc_port);

        // Build volume mounts
        let externally_managed = config.workspace_dir.is_some();
        let volumes = match &config.workspace_dir {
            Some(ws_dir) => vec![(ws_dir.clone(), PathBuf::from("/workspace"))],
            None => vec![],
        };

        // Build env vars, injecting Claude credentials when available
        let mut env_vars = vec![(ur_config::URD_ADDR_ENV.into(), urd_addr)];
        if let Some(creds) = self.credential_manager.read_claude_credentials() {
            env_vars.push((ur_config::CLAUDE_CREDENTIALS_ENV.into(), creds));
        }

        // Run the container (scoped so rt is dropped before any subsequent awaits)
        let cid = {
            let rt = container::runtime_from_env();
            let container_name = format!("ur-agent-{}", config.process_id);
            let opts = container::RunOpts {
                image: container::ImageId(config.image_id.clone()),
                name: container_name,
                cpus: config.cpus,
                memory: config.memory.clone(),
                volumes,
                port_maps: vec![],
                env_vars,
                workdir: Some(PathBuf::from("/workspace")),
                command: vec![],
            };
            rt.run(&opts).map_err(|e| e.to_string())?
        };

        info!(
            process_id = config.process_id,
            container_id = cid.0,
            grpc_port = config.grpc_port,
            "process launched"
        );

        // Record in process map
        {
            let mut procs = self.processes.write().expect("process lock poisoned");
            procs.insert(
                config.process_id,
                ProcessEntry {
                    container_id: cid.0.clone(),
                    grpc_port: config.grpc_port,
                    server_handle,
                    externally_managed,
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
        let cred_mgr = CredentialManager;
        let mgr = ProcessManager::new(workspace.path().to_path_buf(), registry, cred_mgr);
        (mgr, workspace)
    }

    #[tokio::test]
    async fn prepare_creates_repo_and_registers() {
        let (mgr, workspace) = test_manager();
        let process_id = "test-proc";

        mgr.prepare(process_id, None).await.unwrap();

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
    async fn prepare_with_workspace_skips_git_init() {
        let (mgr, _workspace) = test_manager();
        let process_id = "ws-proc";

        // Create a temp dir to act as the external workspace
        let ext_workspace = tempfile::tempdir().unwrap();
        let ext_path = ext_workspace.path().to_path_buf();

        mgr.prepare(process_id, Some(ext_path.clone()))
            .await
            .unwrap();

        // Should NOT have a .git dir — we skipped git init
        assert!(!ext_path.join(".git").exists());

        // Registry should resolve to the external path directly
        let resolved = mgr.repo_registry.resolve(process_id).unwrap();
        assert_eq!(resolved, ext_path);
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
                    externally_managed: false,
                },
            );
        }

        let result = mgr.prepare("dup-proc", None).await;
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
