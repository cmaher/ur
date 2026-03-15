use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use rand::Rng;
use serde::Deserialize;
use tracing::info;
use uuid::Uuid;

use ur_config::NetworkConfig;

use container::{ContainerRuntime, NetworkManager};

use crate::run_opts_builder::RunOptsBuilder;
use crate::strategy::WorkerStrategy;
use crate::{RepoPoolManager, RepoRegistry};

/// Unique identifier for a running agent, format: `{process_id}-{4 random [a-z0-9]}`.
///
/// The random suffix prevents collisions when the same process_id is reused
/// across launches (e.g. after a stop/start cycle).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentId(pub String);

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AgentId {
    /// Generate a new agent ID from a process_id by appending `-{4 random [a-z0-9]}`.
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

    /// Validate that a string matches the expected agent ID format:
    /// non-empty prefix, a dash, then exactly 4 alphanumeric lowercase chars.
    pub fn parse(s: &str) -> Result<Self, String> {
        let Some(dash_pos) = s.rfind('-') else {
            return Err(format!("invalid agent ID (no dash): {s}"));
        };
        if dash_pos == 0 {
            return Err(format!("invalid agent ID (empty process_id): {s}"));
        }
        let suffix = &s[dash_pos + 1..];
        if suffix.len() != 4
            || !suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Err(format!(
                "invalid agent ID suffix (expected 4 [a-z0-9] chars): {s}"
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

/// Context for a running agent, returned by `ProcessManager::get_agent_context`.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// Project key if launched with `--project`, or `None` for raw workspace launches.
    pub project_key: Option<String>,
    /// Host path to the repo slot (workspace dir or pool slot).
    pub slot_path: PathBuf,
}

/// Tracks a running agent process, keyed by `AgentId` in the process table.
struct ProcessEntry {
    /// The original process_id (without random suffix).
    process_id: String,
    /// Project key if launched with `--project`, or empty for raw workspace launches.
    project_key: String,
    /// Host path to the repo slot (workspace dir or pool slot).
    slot_path: Option<PathBuf>,
    /// Worker strategy governing slot acquisition and release behavior.
    strategy: WorkerStrategy,
    container_id: String,
    /// Secret token (UUID v4) for authenticating agent requests.
    agent_secret: String,
}

/// Summary of a running process, returned by `ProcessManager::list()`.
pub struct ProcessSummary {
    pub process_id: String,
    pub agent_id: String,
    pub container_id: String,
    pub project_key: String,
    pub mode: String,
}

/// Configuration for launching a container process.
pub struct ProcessConfig {
    pub process_id: String,
    pub agent_id: AgentId,
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

/// Orchestrates the full lifecycle of agent processes:
/// repo registration, git init, container run/stop.
#[derive(Clone)]
pub struct ProcessManager {
    workspace: PathBuf,
    /// Host-side config directory path, used to construct volume mounts for
    /// agent containers (e.g., shared credentials file).
    host_config_dir: PathBuf,
    repo_registry: Arc<RepoRegistry>,
    repo_pool_manager: RepoPoolManager,
    network_manager: NetworkManager,
    network_config: NetworkConfig,
    /// TCP port the shared worker gRPC server listens on.
    /// Injected into containers as part of `UR_SERVER_ADDR`.
    worker_port: u16,
    prompt_modes: PromptModesConfig,
    processes: Arc<RwLock<HashMap<AgentId, ProcessEntry>>>,
}

impl ProcessManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace: PathBuf,
        host_config_dir: PathBuf,
        repo_registry: Arc<RepoRegistry>,
        repo_pool_manager: RepoPoolManager,
        network_manager: NetworkManager,
        network_config: NetworkConfig,
        worker_port: u16,
        prompt_modes: PromptModesConfig,
    ) -> Self {
        Self {
            workspace,
            host_config_dir,
            repo_registry,
            repo_pool_manager,
            network_manager,
            network_config,
            worker_port,
            prompt_modes,
            processes: Arc::new(RwLock::new(HashMap::new())),
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

    /// Generate a new unique agent ID for the given process_id.
    pub fn generate_agent_id(&self, process_id: &str) -> AgentId {
        AgentId::generate(process_id)
    }

    /// Look up a process entry by agent ID and return the associated process_id.
    pub fn resolve_process_id(&self, agent_id: &AgentId) -> Result<String, String> {
        let procs = self.processes.read().expect("process lock poisoned");
        procs
            .get(agent_id)
            .map(|entry| entry.process_id.clone())
            .ok_or_else(|| format!("unknown agent: {agent_id}"))
    }

    /// Look up agent context (project_key, slot_path) by agent ID.
    /// Returns `None` if the agent is not registered or has no slot_path.
    pub fn get_agent_context(&self, agent_id: &AgentId) -> Option<AgentContext> {
        let procs = self.processes.read().expect("process lock poisoned");
        let entry = procs.get(agent_id)?;
        let slot_path = entry.slot_path.clone()?;
        let project_key = if entry.project_key.is_empty() {
            None
        } else {
            Some(entry.project_key.clone())
        };
        Some(AgentContext {
            project_key,
            slot_path,
        })
    }

    /// Verify that the given agent_id and secret match a registered agent.
    pub fn verify_agent(&self, agent_id: &str, secret: &str) -> bool {
        let Ok(parsed) = AgentId::parse(agent_id) else {
            return false;
        };
        let procs = self.processes.read().expect("process lock poisoned");
        procs
            .get(&parsed)
            .is_some_and(|entry| entry.agent_secret == secret)
    }

    /// Register an agent in the process table without running a container.
    ///
    /// Used by tests that need a registered agent but cannot (or should not) spawn
    /// a real container. The caller supplies the agent_secret and container_id
    /// directly.
    #[allow(clippy::too_many_arguments)]
    pub fn register_agent(
        &self,
        agent_id: AgentId,
        process_id: String,
        project_key: String,
        slot_path: Option<PathBuf>,
        strategy: WorkerStrategy,
        container_id: String,
        agent_secret: String,
    ) {
        let mut procs = self.processes.write().expect("process lock poisoned");
        procs.insert(
            agent_id,
            ProcessEntry {
                process_id,
                project_key,
                slot_path,
                strategy,
                container_id,
                agent_secret,
            },
        );
    }

    /// Phase 1 of launch: create repo dir, git init, register in RepoRegistry.
    /// When `workspace_dir` is Some, the directory is used as-is (no git init)
    /// and registered via `register_absolute`.
    /// The caller is responsible for calling `run_and_record` after `prepare`.
    pub async fn prepare(
        &self,
        process_id: &str,
        agent_id: &AgentId,
        workspace_dir: Option<PathBuf>,
    ) -> Result<(), String> {
        // Check for duplicate process ID
        {
            let procs = self.processes.read().expect("process lock poisoned");
            if procs.values().any(|e| e.process_id == process_id) {
                return Err(format!("process already running: {process_id}"));
            }
        }

        if let Some(ws_dir) = workspace_dir {
            // External workspace: register the absolute path directly, skip git init
            info!(process_id, %agent_id, workspace_dir = %ws_dir.display(), "registering external workspace");
            self.repo_registry.register_absolute(process_id, ws_dir);
        } else {
            // Default: create repo dir and git init
            info!(process_id, %agent_id, "creating repo directory and initializing git");
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
    /// Generates and stores an agent secret (UUID v4) for auth.
    /// Returns `(container_id, agent_secret)`.
    pub async fn run_and_record(&self, config: ProcessConfig) -> Result<(String, String), String> {
        // Ensure the Docker network exists before launching the container
        self.network_manager
            .ensure()
            .map_err(|e| format!("failed to ensure Docker network: {e}"))?;

        // Generate agent secret for worker auth
        let agent_secret = Uuid::new_v4().to_string();

        // Build env vars
        let server_addr = format!(
            "{}:{}",
            self.network_config.server_hostname, self.worker_port
        );
        let mut env_vars = vec![
            (ur_config::UR_SERVER_ADDR_ENV.into(), server_addr),
            (ur_config::UR_AGENT_ID_ENV.into(), config.agent_id.0.clone()),
            (ur_config::UR_AGENT_SECRET_ENV.into(), agent_secret.clone()),
        ];

        // Inject proxy env vars (Squid proxy reachable via Docker DNS on the internal network)
        env_vars.extend(proxy_env_vars(&config.proxy_hostname));

        // Inject resolved skills as comma-separated list
        if !config.skills.is_empty() {
            env_vars.push(("UR_WORKER_SKILLS".into(), config.skills.join(",")));
        }

        // Build RunOpts via the builder
        let container_name = format!("{}{}", self.network_config.agent_prefix, config.process_id);
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
            agent_id = %config.agent_id,
            container_id = cid.0,
            "process launched"
        );

        // Record in process map keyed by agent ID
        {
            let mut procs = self.processes.write().expect("process lock poisoned");
            procs.insert(
                config.agent_id,
                ProcessEntry {
                    process_id: config.process_id,
                    project_key: config.project_key,
                    slot_path: config.workspace_dir,
                    strategy: config.strategy,
                    container_id: cid.0.clone(),
                    agent_secret: agent_secret.clone(),
                },
            );
        }

        Ok((cid.0, agent_secret))
    }

    /// Stop a running agent process by agent ID. Stops + removes the container,
    /// unregisters from RepoRegistry.
    pub async fn stop_by_agent_id(&self, agent_id: &AgentId) -> Result<(), String> {
        let entry = {
            let mut procs = self.processes.write().expect("process lock poisoned");
            procs
                .remove(agent_id)
                .ok_or_else(|| format!("unknown agent: {agent_id}"))?
        };

        info!(
            process_id = entry.process_id,
            %agent_id,
            container_id = entry.container_id,
            "stopping container"
        );

        // 1. Stop + remove container (scoped so rt is dropped before await)
        {
            let rt = container::runtime_from_env();
            let cid = container::ContainerId(entry.container_id);
            rt.stop(&cid).map_err(|e| e.to_string())?;
            rt.rm(&cid).map_err(|e| e.to_string())?;
        }

        // 2. Release pool slot if this was a project-based launch
        if !entry.project_key.is_empty()
            && let Some(ref slot_path) = entry.slot_path
        {
            info!(
                process_id = entry.process_id,
                project_key = entry.project_key,
                slot_path = %slot_path.display(),
                strategy = entry.strategy.name(),
                "releasing pool slot"
            );
            entry
                .strategy
                .release_slot(&self.repo_pool_manager, slot_path)
                .await?;
        }

        // 3. Unregister from RepoRegistry
        self.repo_registry.unregister(&entry.process_id);

        info!(
            process_id = entry.process_id,
            %agent_id,
            "process stopped"
        );

        Ok(())
    }

    /// List all running processes with their metadata.
    pub fn list(&self) -> Vec<ProcessSummary> {
        let procs = self.processes.read().expect("process lock poisoned");
        let mut result: Vec<ProcessSummary> = procs
            .iter()
            .map(|(agent_id, entry)| ProcessSummary {
                process_id: entry.process_id.clone(),
                agent_id: agent_id.0.clone(),
                container_id: entry.container_id.clone(),
                project_key: entry.project_key.clone(),
                mode: entry.strategy.name().to_owned(),
            })
            .collect();
        result.sort_by(|a, b| a.process_id.cmp(&b.process_id));
        result
    }

    /// Stop a running agent process by process_id (searches all entries).
    /// Used by the CLI which only knows the process_id, not the agent_id.
    /// Look up the workspace/slot directory for a running process by its process ID.
    pub fn get_workspace_dir(&self, process_id: &str) -> Result<Option<PathBuf>, String> {
        let procs = self.processes.read().expect("process lock poisoned");
        let entry = procs
            .iter()
            .find(|(_, entry)| entry.process_id == process_id)
            .map(|(_, entry)| entry)
            .ok_or_else(|| format!("unknown process: {process_id}"))?;
        Ok(entry.slot_path.clone())
    }

    pub async fn stop(&self, process_id: &str) -> Result<(), String> {
        let agent_id = {
            let procs = self.processes.read().expect("process lock poisoned");
            procs
                .iter()
                .find(|(_, entry)| entry.process_id == process_id)
                .map(|(id, _)| id.clone())
                .ok_or_else(|| format!("unknown process: {process_id}"))?
        };
        self.stop_by_agent_id(&agent_id).await
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
            hostd_port: ur_config::DEFAULT_HOSTD_PORT,
            compose_file: workspace_path.join("docker-compose.yml"),
            proxy: ur_config::ProxyConfig {
                hostname: ur_config::DEFAULT_PROXY_HOSTNAME.into(),
                allowlist: vec![],
            },
            network: NetworkConfig {
                name: ur_config::DEFAULT_NETWORK_NAME.into(),
                worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
                server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
                agent_prefix: ur_config::DEFAULT_AGENT_PREFIX.into(),
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

    fn test_manager() -> (ProcessManager, tempfile::TempDir) {
        let workspace = tempfile::tempdir().unwrap();
        let registry = Arc::new(RepoRegistry::new(workspace.path().to_path_buf()));
        let config = test_config(workspace.path());
        let repo_pool_manager = RepoPoolManager::new(
            &config,
            workspace.path().to_path_buf(),
            workspace.path().to_path_buf(),
            crate::BuilderdClient::new("http://localhost:42070".into()),
        );
        let network_manager = NetworkManager::new(
            "docker".into(),
            ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
        );
        let network_config = NetworkConfig {
            name: ur_config::DEFAULT_NETWORK_NAME.into(),
            worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
            server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
            agent_prefix: ur_config::DEFAULT_AGENT_PREFIX.into(),
        };
        let mgr = ProcessManager::new(
            workspace.path().to_path_buf(),
            workspace.path().to_path_buf(),
            registry,
            repo_pool_manager,
            network_manager,
            network_config,
            ur_config::DEFAULT_DAEMON_PORT + 1,
            PromptModesConfig::default(),
        );
        (mgr, workspace)
    }

    #[tokio::test]
    async fn prepare_creates_repo_and_registers() {
        let (mgr, workspace) = test_manager();
        let process_id = "test-proc";
        let agent_id = mgr.generate_agent_id(process_id);

        mgr.prepare(process_id, &agent_id, None).await.unwrap();

        // Verify repo dir exists and has .git
        let repo_dir = workspace.path().join(process_id);
        assert!(repo_dir.join(".git").exists());

        // Verify registry resolves
        let resolved = mgr.repo_registry.resolve(process_id);
        assert!(resolved.is_ok());
    }

    #[tokio::test]
    async fn prepare_with_workspace_skips_git_init() {
        let (mgr, _workspace) = test_manager();
        let process_id = "ws-proc";
        let agent_id = mgr.generate_agent_id(process_id);

        // Create a temp dir to act as the external workspace
        let ext_workspace = tempfile::tempdir().unwrap();
        let ext_path = ext_workspace.path().to_path_buf();

        mgr.prepare(process_id, &agent_id, Some(ext_path.clone()))
            .await
            .unwrap();

        // Should NOT have a .git dir — we skipped git init
        assert!(!ext_path.join(".git").exists());

        // Registry should resolve to the external path directly
        let resolved = mgr.repo_registry.resolve(process_id).unwrap();
        assert_eq!(resolved, ext_path);
    }

    #[tokio::test]
    async fn prepare_duplicate_process_id_returns_error() {
        let (mgr, _workspace) = test_manager();

        let existing_agent_id = AgentId("dup-proc-ab12".into());
        // Manually insert a process entry
        {
            let mut procs = mgr.processes.write().unwrap();
            procs.insert(
                existing_agent_id,
                ProcessEntry {
                    process_id: "dup-proc".into(),
                    project_key: String::new(),
                    slot_path: None,
                    strategy: WorkerStrategy::Code,
                    container_id: "fake-cid".into(),
                    agent_secret: Uuid::new_v4().to_string(),
                },
            );
        }

        // A new agent_id with a different suffix should still be rejected
        // because the process_id matches.
        let new_agent_id = AgentId("dup-proc-zz99".into());
        let result = mgr.prepare("dup-proc", &new_agent_id, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("process already running"));
    }

    #[tokio::test]
    async fn stop_unknown_process_returns_error() {
        let (mgr, _workspace) = test_manager();
        let result = mgr.stop("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown process"));
    }

    #[test]
    fn agent_id_generate_format() {
        let id = AgentId::generate("deploy");
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
    fn agent_id_parse_valid() {
        let id = AgentId::parse("deploy-x7q2").unwrap();
        assert_eq!(id.0, "deploy-x7q2");
    }

    #[test]
    fn agent_id_parse_rejects_bad_suffix() {
        assert!(AgentId::parse("deploy-ABCD").is_err());
        assert!(AgentId::parse("deploy-abc").is_err());
        assert!(AgentId::parse("deploy-abcde").is_err());
        assert!(AgentId::parse("nodash").is_err());
        assert!(AgentId::parse("-ab12").is_err());
    }

    #[test]
    fn agent_id_parse_with_multiple_dashes() {
        // process_id itself can contain dashes; we use rfind for the last dash
        let id = AgentId::parse("my-proc-x7q2").unwrap();
        assert_eq!(id.0, "my-proc-x7q2");
    }

    #[tokio::test]
    async fn resolve_process_id_works() {
        let (mgr, _workspace) = test_manager();
        let agent_id = AgentId("test-ab12".into());
        {
            let mut procs = mgr.processes.write().unwrap();
            procs.insert(
                agent_id.clone(),
                ProcessEntry {
                    process_id: "test".into(),
                    project_key: "myproject".into(),
                    slot_path: None,
                    strategy: WorkerStrategy::Code,
                    container_id: "cid".into(),
                    agent_secret: Uuid::new_v4().to_string(),
                },
            );
        }
        assert_eq!(mgr.resolve_process_id(&agent_id).unwrap(), "test");
        assert!(
            mgr.resolve_process_id(&AgentId("unknown-ab12".into()))
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
        assert!(code.contains(&"tk".to_string()));
        assert!(code.contains(&"ship".to_string()));
        let design = cfg.resolve_skills("design", &[]).unwrap();
        assert!(design.contains(&"brainstorming".to_string()));
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
        assert!(design.contains(&"brainstorming".to_string()));
    }

    #[test]
    fn prompt_modes_from_toml_no_section_uses_defaults() {
        let toml = "daemon_port = 5000\n";
        let cfg = PromptModesConfig::from_toml(toml).unwrap();
        let code = cfg.resolve_skills("", &[]).unwrap();
        assert!(code.contains(&"tk".to_string()));
    }

    #[test]
    fn resolve_mode_default_returns_code_strategy() {
        let cfg = PromptModesConfig::default();
        let (strategy, skills) = cfg.resolve_mode("").unwrap();
        assert_eq!(strategy, WorkerStrategy::Code);
        assert!(skills.contains(&"tk:agents".to_string()));
    }

    #[test]
    fn resolve_mode_design_returns_design_strategy() {
        let cfg = PromptModesConfig::default();
        let (strategy, skills) = cfg.resolve_mode("design").unwrap();
        assert_eq!(strategy, WorkerStrategy::Design);
        assert!(skills.contains(&"brainstorming".to_string()));
    }

    #[test]
    fn resolve_mode_custom_inherits_base_strategy() {
        let toml = r#"
[prompt_modes.my-docs]
base = "design"
skills = ["tk", "my-custom-skill"]
"#;
        let cfg = PromptModesConfig::from_toml(toml).unwrap();
        let (strategy, skills) = cfg.resolve_mode("my-docs").unwrap();
        assert_eq!(strategy, WorkerStrategy::Design);
        assert_eq!(skills, vec!["tk", "my-custom-skill"]);
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
skills = ["tk"]
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

    #[test]
    fn verify_agent_valid_pair_returns_true() {
        let (mgr, _workspace) = test_manager();
        let agent_id = AgentId("test-ab12".into());
        let secret = "my-secret-token";
        {
            let mut procs = mgr.processes.write().unwrap();
            procs.insert(
                agent_id.clone(),
                ProcessEntry {
                    process_id: "test".into(),
                    project_key: "proj".into(),
                    slot_path: Some(PathBuf::from("/tmp/slot")),
                    strategy: WorkerStrategy::Code,
                    container_id: "cid".into(),
                    agent_secret: secret.into(),
                },
            );
        }
        assert!(mgr.verify_agent("test-ab12", secret));
    }

    #[test]
    fn verify_agent_wrong_secret_returns_false() {
        let (mgr, _workspace) = test_manager();
        let agent_id = AgentId("test-ab12".into());
        {
            let mut procs = mgr.processes.write().unwrap();
            procs.insert(
                agent_id.clone(),
                ProcessEntry {
                    process_id: "test".into(),
                    project_key: String::new(),
                    slot_path: None,
                    strategy: WorkerStrategy::Code,
                    container_id: "cid".into(),
                    agent_secret: "correct-secret".into(),
                },
            );
        }
        assert!(!mgr.verify_agent("test-ab12", "wrong-secret"));
    }

    #[test]
    fn verify_agent_unknown_id_returns_false() {
        let (mgr, _workspace) = test_manager();
        assert!(!mgr.verify_agent("unknown-ab12", "any-secret"));
    }

    #[test]
    fn verify_agent_invalid_id_format_returns_false() {
        let (mgr, _workspace) = test_manager();
        assert!(!mgr.verify_agent("nodash", "any-secret"));
    }

    #[test]
    fn get_agent_context_returns_context_for_registered_agent() {
        let (mgr, _workspace) = test_manager();
        let agent_id = AgentId("ctx-ab12".into());
        let slot = PathBuf::from("/tmp/slot");
        {
            let mut procs = mgr.processes.write().unwrap();
            procs.insert(
                agent_id.clone(),
                ProcessEntry {
                    process_id: "ctx".into(),
                    project_key: "myproject".into(),
                    slot_path: Some(slot.clone()),
                    strategy: WorkerStrategy::Code,
                    container_id: "cid".into(),
                    agent_secret: "secret".into(),
                },
            );
        }
        let ctx = mgr.get_agent_context(&agent_id).unwrap();
        assert_eq!(ctx.project_key, Some("myproject".to_string()));
        assert_eq!(ctx.slot_path, slot);
    }

    #[test]
    fn get_agent_context_empty_project_key_maps_to_none() {
        let (mgr, _workspace) = test_manager();
        let agent_id = AgentId("ws-ab12".into());
        {
            let mut procs = mgr.processes.write().unwrap();
            procs.insert(
                agent_id.clone(),
                ProcessEntry {
                    process_id: "ws".into(),
                    project_key: String::new(),
                    slot_path: Some(PathBuf::from("/tmp/ws")),
                    strategy: WorkerStrategy::Code,
                    container_id: "cid".into(),
                    agent_secret: "secret".into(),
                },
            );
        }
        let ctx = mgr.get_agent_context(&agent_id).unwrap();
        assert_eq!(ctx.project_key, None);
    }

    #[test]
    fn get_agent_context_returns_none_for_unknown_agent() {
        let (mgr, _workspace) = test_manager();
        let agent_id = AgentId("missing-ab12".into());
        assert!(mgr.get_agent_context(&agent_id).is_none());
    }

    #[test]
    fn get_agent_context_returns_none_when_no_slot_path() {
        let (mgr, _workspace) = test_manager();
        let agent_id = AgentId("nosl-ab12".into());
        {
            let mut procs = mgr.processes.write().unwrap();
            procs.insert(
                agent_id.clone(),
                ProcessEntry {
                    process_id: "nosl".into(),
                    project_key: "proj".into(),
                    slot_path: None,
                    strategy: WorkerStrategy::Code,
                    container_id: "cid".into(),
                    agent_secret: "secret".into(),
                },
            );
        }
        assert!(mgr.get_agent_context(&agent_id).is_none());
    }
}
