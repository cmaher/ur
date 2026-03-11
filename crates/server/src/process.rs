use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::Deserialize;
use tokio::task::JoinHandle;
use tracing::info;

use ur_config::NetworkConfig;

use container::{ContainerRuntime, NetworkManager};

use crate::RepoRegistry;

/// Default skills for the "code" prompt template.
fn default_code_skills() -> Vec<String> {
    vec![
        "tk".into(),
        "ship".into(),
        "tk:agents".into(),
        "tk:start".into(),
        "systematic-debugging".into(),
        "test-driven-development".into(),
        "writing-skills".into(),
    ]
}

/// Default skills for the "design" prompt template.
fn default_design_skills() -> Vec<String> {
    vec![
        "tk".into(),
        "brainstorming".into(),
        "writing-skills".into(),
    ]
}

/// Returns the hardcoded default prompt templates.
fn default_prompt_templates() -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    map.insert("code".into(), default_code_skills());
    map.insert("design".into(), default_design_skills());
    map
}

/// Raw TOML representation for the `[prompt_templates]` section.
/// Each key is a template name mapping to a table with a `skills` list.
#[derive(Debug, Default, Deserialize)]
struct RawPromptTemplates {
    #[serde(flatten)]
    templates: HashMap<String, RawTemplateEntry>,
}

/// A single prompt template entry with its skills list.
#[derive(Debug, Deserialize)]
struct RawTemplateEntry {
    skills: Vec<String>,
}

/// Resolved prompt templates configuration mapping template names to skill lists.
#[derive(Debug, Clone)]
pub struct PromptTemplatesConfig {
    templates: HashMap<String, Vec<String>>,
}

impl Default for PromptTemplatesConfig {
    fn default() -> Self {
        Self {
            templates: default_prompt_templates(),
        }
    }
}

impl PromptTemplatesConfig {
    /// Parse prompt_templates from a TOML string.
    /// If no `[prompt_templates]` section exists, hardcoded defaults are used.
    /// Any templates defined in the config replace the defaults entirely.
    pub fn from_toml(toml_content: &str) -> Result<Self, String> {
        // Parse the full TOML to extract just the prompt_templates section
        let value: toml::Value =
            toml::from_str(toml_content).map_err(|e| format!("invalid TOML: {e}"))?;

        match value.get("prompt_templates") {
            Some(section) => {
                let raw: RawPromptTemplates = section
                    .clone()
                    .try_into()
                    .map_err(|e| format!("invalid prompt_templates config: {e}"))?;
                let mut templates = default_prompt_templates();
                for (name, entry) in raw.templates {
                    templates.insert(name, entry.skills);
                }
                Ok(Self { templates })
            }
            None => Ok(Self::default()),
        }
    }

    /// Resolve skills for a launch request.
    ///
    /// Priority:
    /// 1. If `skills` is non-empty, use it directly.
    /// 2. If `template` is non-empty, look up `prompt_templates.<template>.skills`.
    /// 3. Otherwise, use `prompt_templates.code` (default).
    ///
    /// Returns an error if the requested template name is not found.
    pub fn resolve_skills(&self, template: &str, skills: &[String]) -> Result<Vec<String>, String> {
        if !skills.is_empty() {
            return Ok(skills.to_vec());
        }
        let template_name = if template.is_empty() {
            "code"
        } else {
            template
        };
        self.templates
            .get(template_name)
            .cloned()
            .ok_or_else(|| format!("unknown prompt template: {template_name}"))
    }
}

/// Tracks a running agent process.
struct ProcessEntry {
    container_id: String,
    /// Host-side TCP port the per-agent gRPC server is bound to.
    grpc_port: u16,
    /// Handle to the per-agent gRPC server task.
    server_handle: JoinHandle<()>,
}

/// Configuration for launching a container process.
pub struct ProcessConfig {
    pub process_id: String,
    pub image_id: String,
    pub cpus: u32,
    pub memory: String,
    pub grpc_port: u16,
    pub workspace_dir: Option<PathBuf>,
    pub proxy_hostname: String,
    /// Resolved skills to pass as `UR_WORKER_SKILLS` env var (comma-separated).
    pub skills: Vec<String>,
}

/// Orchestrates the full lifecycle of agent processes:
/// per-agent gRPC server (TCP), repo registration, git init, container run/stop.
#[derive(Clone)]
pub struct ProcessManager {
    workspace: PathBuf,
    /// Host-side config directory path, used to construct volume mounts for
    /// agent containers (e.g., shared credentials file).
    host_config_dir: PathBuf,
    repo_registry: Arc<RepoRegistry>,
    network_manager: NetworkManager,
    network_config: NetworkConfig,
    prompt_templates: PromptTemplatesConfig,
    processes: Arc<RwLock<HashMap<String, ProcessEntry>>>,
}

impl ProcessManager {
    pub fn new(
        workspace: PathBuf,
        host_config_dir: PathBuf,
        repo_registry: Arc<RepoRegistry>,
        network_manager: NetworkManager,
        network_config: NetworkConfig,
        prompt_templates: PromptTemplatesConfig,
    ) -> Self {
        Self {
            workspace,
            host_config_dir,
            repo_registry,
            network_manager,
            network_config,
            prompt_templates,
            processes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Resolve skills for a launch request using the configured prompt templates.
    pub fn resolve_skills(&self, template: &str, skills: &[String]) -> Result<Vec<String>, String> {
        self.prompt_templates.resolve_skills(template, skills)
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
            info!(process_id, workspace_dir = %ws_dir.display(), "registering external workspace");
            self.repo_registry.register_absolute(process_id, ws_dir);
        } else {
            // Default: create repo dir and git init
            info!(process_id, "creating repo directory and initializing git");
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
        // Ensure the Docker network exists before launching the container
        self.network_manager
            .ensure()
            .map_err(|e| format!("failed to ensure Docker network: {e}"))?;

        let server_hostname = &self.network_config.server_hostname;
        let server_addr = format!("{server_hostname}:{}", config.grpc_port);

        // Build volume mounts
        let mut volumes: Vec<(PathBuf, PathBuf)> = Vec::new();
        if let Some(ws_dir) = &config.workspace_dir {
            volumes.push((ws_dir.clone(), PathBuf::from("/workspace")));
        }

        // Mount shared credentials file so all containers share one OAuth session.
        // Claude Code reads/writes this file for token refresh, keeping all
        // containers in sync without per-launch credential injection.
        // (.claude.json is baked into the image — only credentials need mounting.)
        let host_creds = self
            .host_config_dir
            .join(ur_config::CLAUDE_DIR)
            .join(ur_config::CLAUDE_CREDENTIALS_FILENAME);
        ensure_file_exists(&host_creds)
            .map_err(|e| format!("failed to ensure credentials file: {e}"))?;
        let worker_home = PathBuf::from(ur_config::WORKER_HOME);
        volumes.push((
            host_creds,
            worker_home
                .join(".claude")
                .join(ur_config::CLAUDE_CREDENTIALS_FILENAME),
        ));

        let mut env_vars = vec![(ur_config::UR_SERVER_ADDR_ENV.into(), server_addr)];

        // Inject proxy env vars (Squid proxy reachable via Docker DNS on the internal network)
        env_vars.extend(proxy_env_vars(&config.proxy_hostname));

        // Inject resolved skills as comma-separated list
        if !config.skills.is_empty() {
            env_vars.push((
                "UR_WORKER_SKILLS".into(),
                config.skills.join(","),
            ));
        }

        // Run the container on the shared Docker network
        let cid = {
            let rt = container::runtime_from_env();
            let container_name =
                format!("{}{}", self.network_config.agent_prefix, config.process_id);
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
                network: Some(self.network_manager.network_name().to_string()),
                add_hosts: vec![],
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

        info!(
            process_id,
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

        // 2. Unregister from RepoRegistry
        self.repo_registry.unregister(process_id);

        // 3. Abort the per-agent gRPC server task
        entry.server_handle.abort();

        info!(process_id, grpc_port = entry.grpc_port, "process stopped");

        Ok(())
    }
}

/// Ensure a file exists on disk, creating it (and parent dirs) if missing.
/// Docker bind-mounts require the source to exist as a file; if missing, Docker
/// creates a directory instead, causing an OCI runtime error.
fn ensure_file_exists(path: &PathBuf) -> Result<(), std::io::Error> {
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

    fn test_manager() -> (ProcessManager, tempfile::TempDir) {
        let workspace = tempfile::tempdir().unwrap();
        let registry = Arc::new(RepoRegistry::new(workspace.path().to_path_buf()));
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
            network_manager,
            network_config,
            PromptTemplatesConfig::default(),
        );
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

        // Verify registry resolves
        let resolved = mgr.repo_registry.resolve(process_id);
        assert!(resolved.is_ok());
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
    fn prompt_templates_default_has_code_and_design() {
        let cfg = PromptTemplatesConfig::default();
        let code = cfg.resolve_skills("", &[]).unwrap();
        assert!(code.contains(&"tk".to_string()));
        assert!(code.contains(&"ship".to_string()));
        let design = cfg.resolve_skills("design", &[]).unwrap();
        assert!(design.contains(&"brainstorming".to_string()));
    }

    #[test]
    fn prompt_templates_explicit_skills_override() {
        let cfg = PromptTemplatesConfig::default();
        let skills = vec!["custom-skill".to_string()];
        let resolved = cfg.resolve_skills("code", &skills).unwrap();
        assert_eq!(resolved, vec!["custom-skill"]);
    }

    #[test]
    fn prompt_templates_unknown_template_errors() {
        let cfg = PromptTemplatesConfig::default();
        let result = cfg.resolve_skills("nonexistent", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown prompt template"));
    }

    #[test]
    fn prompt_templates_from_toml_overrides_defaults() {
        let toml = r#"
[prompt_templates.code]
skills = ["only-one"]

[prompt_templates.custom]
skills = ["a", "b"]
"#;
        let cfg = PromptTemplatesConfig::from_toml(toml).unwrap();
        let code = cfg.resolve_skills("code", &[]).unwrap();
        assert_eq!(code, vec!["only-one"]);
        let custom = cfg.resolve_skills("custom", &[]).unwrap();
        assert_eq!(custom, vec!["a", "b"]);
        // design default should still be present
        let design = cfg.resolve_skills("design", &[]).unwrap();
        assert!(design.contains(&"brainstorming".to_string()));
    }

    #[test]
    fn prompt_templates_from_toml_no_section_uses_defaults() {
        let toml = "daemon_port = 5000\n";
        let cfg = PromptTemplatesConfig::from_toml(toml).unwrap();
        let code = cfg.resolve_skills("", &[]).unwrap();
        assert!(code.contains(&"tk".to_string()));
    }
}
