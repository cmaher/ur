use std::collections::HashMap;
use std::path::PathBuf;

use chrono::Utc;
use rand::Rng;
use serde::Deserialize;
use tracing::info;
use uuid::Uuid;

use ur_config::NetworkConfig;
use ur_db::WorkerRepo;

use container::{ContainerRuntime, NetworkManager};

use crate::RepoPoolManager;
use crate::run_opts_builder::RunOptsBuilder;
use crate::strategy::WorkerStrategy;

/// Unique identifier for a running worker, format: `{process_id}-{4 random [a-z0-9]}`.
///
/// The random suffix prevents collisions when the same process_id is reused
/// across launches (e.g. after a stop/start cycle).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkerId(pub String);

impl std::fmt::Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl WorkerId {
    /// Generate a new worker ID from a process_id by appending `-{4 random [a-z0-9]}`.
    pub fn generate(process_id: &str) -> Self {
        let mut rng = rand::rng();
        let suffix: String = (0..4)
            .map(|_| {
                let idx = rng.random_range(0..36u8);
                if idx < 10 {
                    (b'0' + idx) as char
                } else {
                    (b'a' + idx - 10) as char
                }
            })
            .collect();
        Self(format!("{process_id}-{suffix}"))
    }

    /// Validate that a string matches the expected worker ID format:
    /// non-empty prefix, a dash, then exactly 4 alphanumeric lowercase chars.
    pub fn parse(s: &str) -> Result<Self, String> {
        let Some(dash_pos) = s.rfind('-') else {
            return Err(format!("invalid worker ID (no dash): {s}"));
        };
        if dash_pos == 0 {
            return Err(format!("invalid worker ID (empty process_id): {s}"));
        }
        let suffix = &s[dash_pos + 1..];
        if suffix.len() != 4
            || !suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Err(format!(
                "invalid worker ID suffix (expected 4 [a-z0-9] chars): {s}"
            ));
        }
        Ok(Self(s.to_string()))
    }
}

/// Returns the hardcoded default prompt modes derived from WorkerStrategy variants.
fn default_prompt_modes() -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    map.insert("code".into(), WorkerStrategy::Code.skills());
    map.insert("design".into(), WorkerStrategy::Design.skills());
    map
}

/// Raw TOML representation for the `[prompt_modes]` section.
/// Each key is a mode name mapping to a table with a `skills` list.
#[derive(Debug, Default, Deserialize)]
struct RawPromptModes {
    #[serde(flatten)]
    modes: HashMap<String, RawModeEntry>,
}

/// A single prompt mode entry with its base strategy and skills list.
#[derive(Debug, Deserialize)]
struct RawModeEntry {
    /// The base worker strategy name (e.g. "code" or "design"). Required for custom modes.
    base: String,
    skills: Vec<String>,
}

/// Resolved prompt modes configuration mapping mode names to skill lists and strategies.
#[derive(Debug, Clone)]
pub struct PromptModesConfig {
    modes: HashMap<String, Vec<String>>,
    /// Maps mode names to their worker strategy. Built-in modes ("code", "design")
    /// map to their corresponding variants; custom modes map via their `base` field.
    strategies: HashMap<String, WorkerStrategy>,
}

impl Default for PromptModesConfig {
    fn default() -> Self {
        let mut strategies = HashMap::new();
        strategies.insert("code".into(), WorkerStrategy::Code);
        strategies.insert("design".into(), WorkerStrategy::Design);
        Self {
            modes: default_prompt_modes(),
            strategies,
        }
    }
}

impl PromptModesConfig {
    /// Parse prompt_modes from a TOML string.
    /// If no `[prompt_modes]` section exists, hardcoded defaults are used.
    /// Any modes defined in the config replace the defaults entirely.
    /// Custom modes must specify a valid `base` field ("code" or "design").
    pub fn from_toml(toml_content: &str) -> Result<Self, String> {
        // Parse the full TOML to extract just the prompt_modes section
        let value: toml::Value =
            toml::from_str(toml_content).map_err(|e| format!("invalid TOML: {e}"))?;

        let Some(section) = value.get("prompt_modes") else {
            return Ok(Self::default());
        };

        let raw: RawPromptModes = section
            .clone()
            .try_into()
            .map_err(|e| format!("invalid prompt_modes config: {e}"))?;
        let mut modes = default_prompt_modes();
        let mut strategies = HashMap::new();
        strategies.insert("code".into(), WorkerStrategy::Code);
        strategies.insert("design".into(), WorkerStrategy::Design);
        for (name, entry) in raw.modes {
            let strategy = WorkerStrategy::from_name(&entry.base).map_err(|_| {
                format!(
                    "invalid base '{}' for prompt mode '{}': must be 'code' or 'design'",
                    entry.base, name
                )
            })?;
            strategies.insert(name.clone(), strategy);
            modes.insert(name, entry.skills);
        }
        Ok(Self { modes, strategies })
    }

    /// Resolve skills for a launch request.
    ///
    /// Priority:
    /// 1. If `skills` is non-empty, use it directly.
    /// 2. If `mode` is non-empty, look up `prompt_modes.<mode>.skills`.
    /// 3. Otherwise, use `prompt_modes.code` (default).
    ///
    /// Returns an error if the requested mode name is not found.
    pub fn resolve_skills(&self, mode: &str, skills: &[String]) -> Result<Vec<String>, String> {
        if !skills.is_empty() {
            return Ok(skills.to_vec());
        }
        let mode_name = if mode.is_empty() { "code" } else { mode };
        self.modes
            .get(mode_name)
            .cloned()
            .ok_or_else(|| format!("unknown prompt mode: {mode_name}"))
    }

    /// Resolve a mode name to its worker strategy and skill list.
    ///
    /// For built-in modes ("code", "design"), returns the corresponding
    /// `WorkerStrategy` variant and its default skills. For custom modes,
    /// returns the strategy determined by the `base` field and the custom skills.
    /// An empty mode name defaults to "code".
    pub fn resolve_mode(&self, mode: &str) -> Result<(WorkerStrategy, Vec<String>), String> {
        let mode_name = if mode.is_empty() { "code" } else { mode };
        let strategy = self
            .strategies
            .get(mode_name)
            .copied()
            .ok_or_else(|| format!("unknown prompt mode: {mode_name}"))?;
        let skills = self
            .modes
            .get(mode_name)
            .cloned()
            .ok_or_else(|| format!("unknown prompt mode: {mode_name}"))?;
        Ok((strategy, skills))
    }
}

/// Context for a running worker, returned by `WorkerManager::get_worker_context`.
#[derive(Debug, Clone)]
pub struct WorkerContext {
    /// Project key if launched with `--project`, or `None` for raw workspace launches.
    pub project_key: Option<String>,
    /// Host path to the repo slot (workspace dir or pool slot).
    pub slot_path: PathBuf,
}

/// Summary of a running process, returned by `WorkerManager::list()`.
pub struct WorkerSummary {
    pub process_id: String,
    pub worker_id: String,
    pub container_id: String,
    pub project_key: String,
    pub mode: String,
}

/// Configuration for launching a container process.
pub struct WorkerConfig {
    pub process_id: String,
    pub worker_id: WorkerId,
    pub image_id: String,
    pub cpus: u32,
    pub memory: String,
    pub workspace_dir: Option<PathBuf>,
    pub proxy_hostname: String,
    /// Project key if launched with `--project` (empty string for raw workspace launches).
    pub project_key: String,
    /// Worker strategy governing slot acquisition and release behavior.
    pub strategy: WorkerStrategy,
    /// Resolved skills to pass as `UR_WORKER_SKILLS` env var (comma-separated).
    pub skills: Vec<String>,
    /// Optional git hooks directory template string from project config.
    pub git_hooks_dir: Option<String>,
    /// Additional volume mounts from project config (source:destination pairs).
    pub mounts: Vec<ur_config::MountConfig>,
}

/// Orchestrates the full lifecycle of worker processes:
/// repo registration, git init, container run/stop.
#[derive(Clone)]
pub struct WorkerManager {
    workspace: PathBuf,
    /// Host-side config directory path, used to construct volume mounts for
    /// worker containers (e.g., shared credentials file).
    host_config_dir: PathBuf,
    repo_pool_manager: RepoPoolManager,
    network_manager: NetworkManager,
    network_config: NetworkConfig,
    /// TCP port the shared worker gRPC server listens on.
    /// Injected into containers as part of `UR_SERVER_ADDR`.
    worker_port: u16,
    prompt_modes: PromptModesConfig,
    worker_repo: WorkerRepo,
}

impl WorkerManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace: PathBuf,
        host_config_dir: PathBuf,
        repo_pool_manager: RepoPoolManager,
        network_manager: NetworkManager,
        network_config: NetworkConfig,
        worker_port: u16,
        prompt_modes: PromptModesConfig,
        worker_repo: WorkerRepo,
    ) -> Self {
        Self {
            workspace,
            host_config_dir,
            repo_pool_manager,
            network_manager,
            network_config,
            worker_port,
            prompt_modes,
            worker_repo,
        }
    }

    /// Resolve skills for a launch request using the configured prompt modes.
    pub fn resolve_skills(&self, mode: &str, skills: &[String]) -> Result<Vec<String>, String> {
        self.prompt_modes.resolve_skills(mode, skills)
    }

    /// Resolve a mode name to its worker strategy and skill list.
    pub fn resolve_mode(&self, mode: &str) -> Result<(WorkerStrategy, Vec<String>), String> {
        self.prompt_modes.resolve_mode(mode)
    }

    /// Generate a new unique worker ID for the given process_id.
    pub fn generate_worker_id(&self, process_id: &str) -> WorkerId {
        WorkerId::generate(process_id)
    }

    /// Look up a worker by worker ID and return the associated process_id.
    pub async fn resolve_process_id(&self, worker_id: &WorkerId) -> Result<String, String> {
        let worker = self
            .worker_repo
            .get_worker(&worker_id.0)
            .await
            .map_err(|e| format!("db error: {e}"))?;
        worker
            .map(|w| w.process_id)
            .ok_or_else(|| format!("unknown worker: {worker_id}"))
    }

    /// Look up worker context (project_key, slot_path) by worker ID.
    /// Returns `None` if the worker is not registered or has no workspace_path.
    pub async fn get_worker_context(&self, worker_id: &WorkerId) -> Option<WorkerContext> {
        let worker = self.worker_repo.get_worker(&worker_id.0).await.ok()??;
        let workspace_path = worker.workspace_path?;
        let project_key = if worker.project_key.is_empty() {
            None
        } else {
            Some(worker.project_key)
        };
        Some(WorkerContext {
            project_key,
            slot_path: PathBuf::from(workspace_path),
        })
    }

    /// Verify that the given worker_id and secret match a registered worker.
    pub async fn verify_worker(&self, worker_id: &str, secret: &str) -> bool {
        let Ok(_parsed) = WorkerId::parse(worker_id) else {
            return false;
        };
        self.worker_repo
            .verify_worker(worker_id, secret)
            .await
            .unwrap_or(false)
    }

    /// Register a worker in the database without running a container.
    ///
    /// Used by tests that need a registered worker but cannot (or should not) spawn
    /// a real container. The caller supplies the worker_secret and container_id
    /// directly.
    #[allow(clippy::too_many_arguments)]
    pub async fn register_worker(
        &self,
        worker_id: WorkerId,
        process_id: String,
        project_key: String,
        slot_path: Option<PathBuf>,
        strategy: WorkerStrategy,
        container_id: String,
        worker_secret: String,
    ) {
        let now = Utc::now().to_rfc3339();
        let worker = ur_db::model::Worker {
            worker_id: worker_id.0,
            process_id,
            project_key,
            slot_id: None,
            container_id,
            worker_secret,
            strategy: strategy.name().to_owned(),
            status: "running".to_owned(),
            workspace_path: slot_path.map(|p| p.display().to_string()),
            created_at: now.clone(),
            updated_at: now,
        };
        self.worker_repo
            .insert_worker(&worker)
            .await
            .expect("failed to register worker");
    }

    /// Phase 1 of launch: create repo dir and git init.
    /// When `workspace_dir` is Some, the directory is used as-is (no git init).
    /// The caller is responsible for calling `run_and_record` after `prepare`.
    pub async fn prepare(
        &self,
        process_id: &str,
        worker_id: &WorkerId,
        workspace_dir: Option<PathBuf>,
    ) -> Result<(), String> {
        // Check for duplicate process ID via database
        let running = self
            .worker_repo
            .list_workers_by_status("running")
            .await
            .map_err(|e| format!("db error: {e}"))?;
        let provisioning = self
            .worker_repo
            .list_workers_by_status("provisioning")
            .await
            .map_err(|e| format!("db error: {e}"))?;
        let has_duplicate = running
            .iter()
            .chain(provisioning.iter())
            .any(|a| a.process_id == process_id);
        if has_duplicate {
            return Err(format!("process already running: {process_id}"));
        }

        if let Some(ws_dir) = workspace_dir {
            // External workspace: skip git init (worker.workspace_path in DB handles CWD resolution)
            info!(process_id, %worker_id, workspace_dir = %ws_dir.display(), "registering external workspace");
        } else {
            // Default: create repo dir and git init
            info!(process_id, %worker_id, "creating repo directory and initializing git");
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
        }

        Ok(())
    }

    /// Phase 2 of launch: run the container and record the worker in the database.
    /// Generates and stores a worker secret (UUID v4) for auth.
    /// Returns `(container_id, worker_secret)`.
    pub async fn run_and_record(&self, config: WorkerConfig) -> Result<(String, String), String> {
        // Ensure the Docker network exists before launching the container
        self.network_manager
            .ensure()
            .map_err(|e| format!("failed to ensure Docker network: {e}"))?;

        // Generate worker secret for worker auth
        let worker_secret = Uuid::new_v4().to_string();

        // Build env vars
        let server_addr = format!(
            "{}:{}",
            self.network_config.server_hostname, self.worker_port
        );
        let mut env_vars = vec![
            (ur_config::UR_SERVER_ADDR_ENV.into(), server_addr),
            (
                ur_config::UR_WORKER_ID_ENV.into(),
                config.worker_id.0.clone(),
            ),
            (
                ur_config::UR_WORKER_SECRET_ENV.into(),
                worker_secret.clone(),
            ),
        ];

        // Inject proxy env vars (Squid proxy reachable via Docker DNS on the internal network)
        env_vars.extend(proxy_env_vars(&config.proxy_hostname));

        // Inject resolved skills as comma-separated list
        if !config.skills.is_empty() {
            env_vars.push(("UR_WORKER_SKILLS".into(), config.skills.join(",")));
        }

        // Inject project key so workers can resolve project context via env
        if !config.project_key.is_empty() {
            env_vars.push(("UR_PROJECT".into(), config.project_key.clone()));
        }

        // Build RunOpts via the builder
        let container_name = format!("{}{}", self.network_config.worker_prefix, config.process_id);
        let opts = RunOptsBuilder::new(
            config.image_id.clone(),
            container_name,
            self.network_manager.network_name().to_string(),
        )
        .cpus(config.cpus)
        .memory(config.memory.clone())
        .workdir("/workspace")
        .add_workspace(&config.workspace_dir)
        .add_credentials(&self.host_config_dir)?
        .add_git_hooks(&config.git_hooks_dir, &self.host_config_dir)?
        .add_mounts(&config.mounts, &self.host_config_dir)?
        .add_env_vars(env_vars)
        .build();

        // Run the container on the shared Docker network
        let cid = {
            let rt = container::runtime_from_env();
            rt.run(&opts).map_err(|e| e.to_string())?
        };

        info!(
            process_id = config.process_id,
            worker_id = %config.worker_id,
            container_id = cid.0,
            "process launched"
        );

        // Record in database
        let now = Utc::now().to_rfc3339();
        let worker = ur_db::model::Worker {
            worker_id: config.worker_id.0,
            process_id: config.process_id,
            project_key: config.project_key,
            slot_id: None,
            container_id: cid.0.clone(),
            worker_secret: worker_secret.clone(),
            strategy: config.strategy.name().to_owned(),
            status: "running".to_owned(),
            workspace_path: config.workspace_dir.map(|p| p.display().to_string()),
            created_at: now.clone(),
            updated_at: now,
        };
        self.worker_repo
            .insert_worker(&worker)
            .await
            .map_err(|e| format!("failed to record worker: {e}"))?;

        Ok((cid.0, worker_secret))
    }

    /// Stop a running worker process by worker ID. Stops + removes the container.
    pub async fn stop_by_worker_id(&self, worker_id: &WorkerId) -> Result<(), String> {
        let worker = self
            .worker_repo
            .get_worker(&worker_id.0)
            .await
            .map_err(|e| format!("db error: {e}"))?
            .ok_or_else(|| format!("unknown worker: {worker_id}"))?;

        info!(
            process_id = worker.process_id,
            %worker_id,
            container_id = worker.container_id,
            "stopping container"
        );

        // 1. Stop + remove container (scoped so rt is dropped before await)
        {
            let rt = container::runtime_from_env();
            let cid = container::ContainerId(worker.container_id.clone());
            rt.stop(&cid).map_err(|e| e.to_string())?;
            rt.rm(&cid).map_err(|e| e.to_string())?;
        }

        // 2. Release pool slot if this was a project-based launch
        let strategy = WorkerStrategy::from_name(&worker.strategy)
            .map_err(|e| format!("invalid strategy in db: {e}"))?;
        if !worker.project_key.is_empty()
            && let Some(ref workspace_path) = worker.workspace_path
        {
            let slot_path = PathBuf::from(workspace_path);
            info!(
                process_id = worker.process_id,
                project_key = worker.project_key,
                slot_path = %slot_path.display(),
                strategy = strategy.name(),
                "releasing pool slot"
            );
            strategy
                .release_slot(&self.repo_pool_manager, &slot_path)
                .await?;
        }

        // 3. Update status to stopped in database
        self.worker_repo
            .update_worker_status(&worker_id.0, "stopped")
            .await
            .map_err(|e| format!("failed to update worker status: {e}"))?;

        info!(
            process_id = worker.process_id,
            %worker_id,
            "process stopped"
        );

        Ok(())
    }

    /// List all running processes with their metadata.
    pub async fn list(&self) -> Vec<WorkerSummary> {
        let workers = self
            .worker_repo
            .list_workers_by_status("running")
            .await
            .unwrap_or_default();
        let mut result: Vec<WorkerSummary> = workers
            .into_iter()
            .map(|worker| WorkerSummary {
                process_id: worker.process_id,
                worker_id: worker.worker_id,
                container_id: worker.container_id,
                project_key: worker.project_key,
                mode: worker.strategy,
            })
            .collect();
        result.sort_by(|a, b| a.process_id.cmp(&b.process_id));
        result
    }

    /// Look up the workspace/slot directory for a running process by its process ID.
    pub async fn get_workspace_dir(&self, process_id: &str) -> Result<Option<PathBuf>, String> {
        let workers = self
            .worker_repo
            .list_workers_by_status("running")
            .await
            .map_err(|e| format!("db error: {e}"))?;
        let worker = workers
            .iter()
            .find(|w| w.process_id == process_id)
            .ok_or_else(|| format!("unknown process: {process_id}"))?;
        Ok(worker.workspace_path.as_ref().map(PathBuf::from))
    }

    /// Stop a running worker process by process_id (searches all entries).
    /// Used by the CLI which only knows the process_id, not the worker_id.
    pub async fn stop(&self, process_id: &str) -> Result<(), String> {
        let workers = self
            .worker_repo
            .list_workers_by_status("running")
            .await
            .map_err(|e| format!("db error: {e}"))?;
        let worker = workers
            .iter()
            .find(|w| w.process_id == process_id)
            .ok_or_else(|| format!("unknown process: {process_id}"))?;
        let worker_id = WorkerId::parse(&worker.worker_id)?;
        self.stop_by_worker_id(&worker_id).await
    }
}

/// Ensure a file exists on disk, creating it (and parent dirs) if missing.
/// Docker bind-mounts require the source to exist as a file; if missing, Docker
/// creates a directory instead, causing an OCI runtime error.
pub(crate) fn ensure_file_exists(path: &PathBuf) -> Result<(), std::io::Error> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, "{}")?;
    Ok(())
}

/// Build `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` env var pairs for container injection.
///
/// Uses `http://` scheme even for `HTTPS_PROXY` — this tells the client to speak plain HTTP
/// to the proxy, which then tunnels TLS traffic via CONNECT.
fn proxy_env_vars(proxy_hostname: &str) -> Vec<(String, String)> {
    let proxy_url = format!("http://{proxy_hostname}:{}", ur_config::SQUID_PORT);
    vec![
        ("HTTP_PROXY".into(), proxy_url.clone()),
        ("HTTPS_PROXY".into(), proxy_url),
        ("NO_PROXY".into(), String::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(workspace_path: &std::path::Path) -> ur_config::Config {
        ur_config::Config {
            config_dir: workspace_path.to_path_buf(),
            workspace: workspace_path.to_path_buf(),
            daemon_port: ur_config::DEFAULT_DAEMON_PORT,
            builderd_port: ur_config::DEFAULT_DAEMON_PORT + 2,
            compose_file: workspace_path.join("docker-compose.yml"),
            proxy: ur_config::ProxyConfig {
                hostname: ur_config::DEFAULT_PROXY_HOSTNAME.into(),
                allowlist: vec![],
            },
            network: NetworkConfig {
                name: ur_config::DEFAULT_NETWORK_NAME.into(),
                worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
                server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
                worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.into(),
            },
            hostexec: ur_config::HostExecConfig::default(),
            rag: ur_config::RagConfig {
                qdrant_hostname: ur_config::DEFAULT_QDRANT_HOSTNAME.into(),
                embedding_model: ur_config::DEFAULT_EMBEDDING_MODEL.into(),
                docs: ur_config::RagDocsConfig::default(),
            },
            backup: ur_config::BackupConfig {
                path: None,
                interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                enabled: true,
                retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
            },
            worker_port: ur_config::DEFAULT_DAEMON_PORT + 1,
            projects: std::collections::HashMap::new(),
        }
    }

    async fn test_worker_repo() -> WorkerRepo {
        let db = ur_db::DatabaseManager::open(":memory:")
            .await
            .expect("failed to open in-memory db");
        WorkerRepo::new(db.pool().clone())
    }

    async fn test_manager() -> (WorkerManager, tempfile::TempDir) {
        let workspace = tempfile::tempdir().unwrap();
        let config = test_config(workspace.path());
        let worker_repo = test_worker_repo().await;
        let repo_pool_manager = RepoPoolManager::new(
            &config,
            workspace.path().to_path_buf(),
            workspace.path().to_path_buf(),
            crate::BuilderdClient::new("http://localhost:42070".into()),
            worker_repo.clone(),
        );
        let network_manager = NetworkManager::new(
            "docker".into(),
            ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
        );
        let network_config = NetworkConfig {
            name: ur_config::DEFAULT_NETWORK_NAME.into(),
            worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
            server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
            worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.into(),
        };
        let mgr = WorkerManager::new(
            workspace.path().to_path_buf(),
            workspace.path().to_path_buf(),
            repo_pool_manager,
            network_manager,
            network_config,
            ur_config::DEFAULT_DAEMON_PORT + 1,
            PromptModesConfig::default(),
            worker_repo,
        );
        (mgr, workspace)
    }

    #[tokio::test]
    async fn prepare_creates_repo_and_registers() {
        let (mgr, workspace) = test_manager().await;
        let process_id = "test-proc";
        let wid = mgr.generate_worker_id(process_id);

        mgr.prepare(process_id, &wid, None).await.unwrap();

        // Verify repo dir exists and has .git
        let repo_dir = workspace.path().join(process_id);
        assert!(repo_dir.join(".git").exists());
    }

    #[tokio::test]
    async fn prepare_with_workspace_skips_git_init() {
        let (mgr, _workspace) = test_manager().await;
        let process_id = "ws-proc";
        let wid = mgr.generate_worker_id(process_id);

        // Create a temp dir to act as the external workspace
        let ext_workspace = tempfile::tempdir().unwrap();
        let ext_path = ext_workspace.path().to_path_buf();

        mgr.prepare(process_id, &wid, Some(ext_path.clone()))
            .await
            .unwrap();

        // Should NOT have a .git dir — we skipped git init
        assert!(!ext_path.join(".git").exists());
    }

    #[tokio::test]
    async fn prepare_duplicate_process_id_returns_error() {
        let (mgr, _workspace) = test_manager().await;

        let existing_wid = WorkerId("dup-proc-ab12".into());
        // Insert a running worker into the database
        mgr.register_worker(
            existing_wid,
            "dup-proc".into(),
            String::new(),
            None,
            WorkerStrategy::Code,
            "fake-cid".into(),
            Uuid::new_v4().to_string(),
        )
        .await;

        // A new worker_id with a different suffix should still be rejected
        // because the process_id matches.
        let new_wid = WorkerId("dup-proc-zz99".into());
        let result = mgr.prepare("dup-proc", &new_wid, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("process already running"));
    }

    #[tokio::test]
    async fn stop_unknown_process_returns_error() {
        let (mgr, _workspace) = test_manager().await;
        let result = mgr.stop("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown process"));
    }

    #[test]
    fn worker_id_generate_format() {
        let id = WorkerId::generate("deploy");
        assert!(
            id.0.starts_with("deploy-"),
            "expected deploy- prefix: {}",
            id.0
        );
        let suffix = &id.0["deploy-".len()..];
        assert_eq!(suffix.len(), 4, "expected 4-char suffix: {suffix}");
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "suffix must be [a-z0-9]: {suffix}"
        );
    }

    #[test]
    fn worker_id_parse_valid() {
        let id = WorkerId::parse("deploy-x7q2").unwrap();
        assert_eq!(id.0, "deploy-x7q2");
    }

    #[test]
    fn worker_id_parse_rejects_bad_suffix() {
        assert!(WorkerId::parse("deploy-ABCD").is_err());
        assert!(WorkerId::parse("deploy-abc").is_err());
        assert!(WorkerId::parse("deploy-abcde").is_err());
        assert!(WorkerId::parse("nodash").is_err());
        assert!(WorkerId::parse("-ab12").is_err());
    }

    #[test]
    fn worker_id_parse_with_multiple_dashes() {
        // process_id itself can contain dashes; we use rfind for the last dash
        let id = WorkerId::parse("my-proc-x7q2").unwrap();
        assert_eq!(id.0, "my-proc-x7q2");
    }

    #[tokio::test]
    async fn resolve_process_id_works() {
        let (mgr, _workspace) = test_manager().await;
        let wid = WorkerId("test-ab12".into());
        mgr.register_worker(
            wid.clone(),
            "test".into(),
            "myproject".into(),
            None,
            WorkerStrategy::Code,
            "cid".into(),
            Uuid::new_v4().to_string(),
        )
        .await;
        assert_eq!(mgr.resolve_process_id(&wid).await.unwrap(), "test");
        assert!(
            mgr.resolve_process_id(&WorkerId("unknown-ab12".into()))
                .await
                .is_err()
        );
    }

    #[test]
    fn proxy_env_vars_uses_squid_hostname() {
        let vars = proxy_env_vars("ur-squid");
        assert_eq!(vars.len(), 3);
        assert_eq!(
            vars[0],
            ("HTTP_PROXY".into(), "http://ur-squid:3128".into())
        );
        assert_eq!(
            vars[1],
            ("HTTPS_PROXY".into(), "http://ur-squid:3128".into())
        );
        assert_eq!(vars[2], ("NO_PROXY".into(), String::new()));
    }

    #[test]
    fn proxy_env_vars_uses_http_scheme_for_https() {
        let vars = proxy_env_vars("ur-squid");
        let https_val = &vars[1].1;
        assert!(
            https_val.starts_with("http://"),
            "HTTPS_PROXY must use http:// scheme (proxy speaks plain HTTP, tunnels via CONNECT)"
        );
    }

    #[test]
    fn prompt_modes_default_has_code_and_design() {
        let cfg = PromptModesConfig::default();
        let code = cfg.resolve_skills("", &[]).unwrap();
        assert!(code.contains(&"tickets".to_string()));
        assert!(code.contains(&"ship".to_string()));
        let design = cfg.resolve_skills("design", &[]).unwrap();
        assert!(design.contains(&"design".to_string()));
    }

    #[test]
    fn prompt_modes_explicit_skills_override() {
        let cfg = PromptModesConfig::default();
        let skills = vec!["custom-skill".to_string()];
        let resolved = cfg.resolve_skills("code", &skills).unwrap();
        assert_eq!(resolved, vec!["custom-skill"]);
    }

    #[test]
    fn prompt_modes_unknown_mode_errors() {
        let cfg = PromptModesConfig::default();
        let result = cfg.resolve_skills("nonexistent", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown prompt mode"));
    }

    #[test]
    fn prompt_modes_from_toml_overrides_defaults() {
        let toml = r#"
[prompt_modes.code]
base = "code"
skills = ["only-one"]

[prompt_modes.custom]
base = "design"
skills = ["a", "b"]
"#;
        let cfg = PromptModesConfig::from_toml(toml).unwrap();
        let code = cfg.resolve_skills("code", &[]).unwrap();
        assert_eq!(code, vec!["only-one"]);
        let custom = cfg.resolve_skills("custom", &[]).unwrap();
        assert_eq!(custom, vec!["a", "b"]);
        // design default should still be present
        let design = cfg.resolve_skills("design", &[]).unwrap();
        assert!(design.contains(&"design".to_string()));
    }

    #[test]
    fn prompt_modes_from_toml_no_section_uses_defaults() {
        let toml = "daemon_port = 5000\n";
        let cfg = PromptModesConfig::from_toml(toml).unwrap();
        let code = cfg.resolve_skills("", &[]).unwrap();
        assert!(code.contains(&"tickets".to_string()));
    }

    #[test]
    fn resolve_mode_default_returns_code_strategy() {
        let cfg = PromptModesConfig::default();
        let (strategy, skills) = cfg.resolve_mode("").unwrap();
        assert_eq!(strategy, WorkerStrategy::Code);
        assert!(skills.contains(&"implement".to_string()));
    }

    #[test]
    fn resolve_mode_design_returns_design_strategy() {
        let cfg = PromptModesConfig::default();
        let (strategy, skills) = cfg.resolve_mode("design").unwrap();
        assert_eq!(strategy, WorkerStrategy::Design);
        assert!(skills.contains(&"design".to_string()));
    }

    #[test]
    fn resolve_mode_custom_inherits_base_strategy() {
        let toml = r#"
[prompt_modes.my-docs]
base = "design"
skills = ["tickets", "my-custom-skill"]
"#;
        let cfg = PromptModesConfig::from_toml(toml).unwrap();
        let (strategy, skills) = cfg.resolve_mode("my-docs").unwrap();
        assert_eq!(strategy, WorkerStrategy::Design);
        assert_eq!(skills, vec!["tickets", "my-custom-skill"]);
    }

    #[test]
    fn resolve_mode_unknown_errors() {
        let cfg = PromptModesConfig::default();
        let result = cfg.resolve_mode("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn from_toml_rejects_invalid_base() {
        let toml = r#"
[prompt_modes.bad]
base = "invalid"
skills = ["tickets"]
"#;
        let result = PromptModesConfig::from_toml(toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid base"));
    }

    #[test]
    fn from_toml_overrides_builtin_strategy() {
        let toml = r#"
[prompt_modes.code]
base = "code"
skills = ["only-one"]
"#;
        let cfg = PromptModesConfig::from_toml(toml).unwrap();
        let (strategy, skills) = cfg.resolve_mode("code").unwrap();
        assert_eq!(strategy, WorkerStrategy::Code);
        assert_eq!(skills, vec!["only-one"]);
    }

    #[tokio::test]
    async fn verify_worker_valid_pair_returns_true() {
        let (mgr, _workspace) = test_manager().await;
        let wid = WorkerId("test-ab12".into());
        let secret = "my-secret-token";
        mgr.register_worker(
            wid.clone(),
            "test".into(),
            "proj".into(),
            Some(PathBuf::from("/tmp/slot")),
            WorkerStrategy::Code,
            "cid".into(),
            secret.into(),
        )
        .await;
        assert!(mgr.verify_worker("test-ab12", secret).await);
    }

    #[tokio::test]
    async fn verify_worker_wrong_secret_returns_false() {
        let (mgr, _workspace) = test_manager().await;
        let wid = WorkerId("test-ab12".into());
        mgr.register_worker(
            wid.clone(),
            "test".into(),
            String::new(),
            None,
            WorkerStrategy::Code,
            "cid".into(),
            "correct-secret".into(),
        )
        .await;
        assert!(!mgr.verify_worker("test-ab12", "wrong-secret").await);
    }

    #[tokio::test]
    async fn verify_worker_unknown_id_returns_false() {
        let (mgr, _workspace) = test_manager().await;
        assert!(!mgr.verify_worker("unknown-ab12", "any-secret").await);
    }

    #[tokio::test]
    async fn verify_worker_invalid_id_format_returns_false() {
        let (mgr, _workspace) = test_manager().await;
        assert!(!mgr.verify_worker("nodash", "any-secret").await);
    }

    #[tokio::test]
    async fn get_worker_context_returns_context_for_registered_agent() {
        let (mgr, _workspace) = test_manager().await;
        let wid = WorkerId("ctx-ab12".into());
        let slot = PathBuf::from("/tmp/slot");
        mgr.register_worker(
            wid.clone(),
            "ctx".into(),
            "myproject".into(),
            Some(slot.clone()),
            WorkerStrategy::Code,
            "cid".into(),
            "secret".into(),
        )
        .await;
        let ctx = mgr.get_worker_context(&wid).await.unwrap();
        assert_eq!(ctx.project_key, Some("myproject".to_string()));
        assert_eq!(ctx.slot_path, slot);
    }

    #[tokio::test]
    async fn get_worker_context_empty_project_key_maps_to_none() {
        let (mgr, _workspace) = test_manager().await;
        let wid = WorkerId("ws-ab12".into());
        mgr.register_worker(
            wid.clone(),
            "ws".into(),
            String::new(),
            Some(PathBuf::from("/tmp/ws")),
            WorkerStrategy::Code,
            "cid".into(),
            "secret".into(),
        )
        .await;
        let ctx = mgr.get_worker_context(&wid).await.unwrap();
        assert_eq!(ctx.project_key, None);
    }

    #[tokio::test]
    async fn get_worker_context_returns_none_for_unknown_agent() {
        let (mgr, _workspace) = test_manager().await;
        let wid = WorkerId("missing-ab12".into());
        assert!(mgr.get_worker_context(&wid).await.is_none());
    }

    #[tokio::test]
    async fn get_worker_context_returns_none_when_no_slot_path() {
        let (mgr, _workspace) = test_manager().await;
        let wid = WorkerId("nosl-ab12".into());
        mgr.register_worker(
            wid.clone(),
            "nosl".into(),
            "proj".into(),
            None,
            WorkerStrategy::Code,
            "cid".into(),
            "secret".into(),
        )
        .await;
        assert!(mgr.get_worker_context(&wid).await.is_none());
    }
}
