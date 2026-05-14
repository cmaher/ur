use std::path::{Path, PathBuf};

use ur_config::{MountConfig, PortMapping, ResolvedTemplatePath, resolve_template_path};
use ur_rpc::proto::builder_container::{
    EnvVar as ProtoEnvVar, LaunchWorkerRequest, PortMap as ProtoPortMap, Volume as ProtoVolume,
};

use crate::worker::ensure_file_exists;

/// Builder that accumulates volumes, env vars, and config to produce a [`LaunchWorkerRequest`].
///
/// Each concern (workspace, credentials, env vars) is a separate contributor method,
/// making it easy to add new volume mounts or env vars without bloating `run_and_record()`.
#[derive(Debug)]
pub struct RunOptsBuilder {
    image: String,
    name: String,
    network: String,
    cpus: u32,
    memory: String,
    workdir: Option<PathBuf>,
    volumes: Vec<(PathBuf, PathBuf)>,
    port_maps: Vec<ProtoPortMap>,
    env_vars: Vec<(String, String)>,
}

impl RunOptsBuilder {
    /// Create a builder with the required fields: image, container name, and network.
    pub fn new(image: String, name: String, network: String) -> Self {
        Self {
            image,
            name,
            network,
            cpus: 0,
            memory: String::new(),
            workdir: None,
            volumes: Vec::new(),
            port_maps: Vec::new(),
            env_vars: Vec::new(),
        }
    }

    /// Set CPU count for the container.
    pub fn cpus(mut self, cpus: u32) -> Self {
        self.cpus = cpus;
        self
    }

    /// Set memory limit for the container (e.g. "4g").
    pub fn memory(mut self, memory: String) -> Self {
        self.memory = memory;
        self
    }

    /// Set the working directory inside the container.
    pub fn workdir(mut self, workdir: &str) -> Self {
        self.workdir = Some(PathBuf::from(workdir));
        self
    }

    /// Add the workspace volume mount (host dir -> /workspace).
    /// No-op if `workspace_dir` is `None`.
    pub fn add_workspace(mut self, workspace_dir: &Option<PathBuf>) -> Self {
        if let Some(ws_dir) = workspace_dir {
            self.volumes
                .push((ws_dir.clone(), PathBuf::from("/workspace")));
        }
        self
    }

    /// Add the shared credentials volume mount so all containers share one OAuth session.
    ///
    /// Claude Code reads/writes this file for token refresh, keeping all
    /// containers in sync without per-launch credential injection.
    /// (.claude.json is baked into the image -- only credentials need mounting.)
    pub fn add_credentials(mut self, host_config_dir: &Path) -> Result<Self, String> {
        let host_creds = host_config_dir
            .join(ur_config::CLAUDE_DIR)
            .join(ur_config::CLAUDE_CREDENTIALS_FILENAME);
        ensure_file_exists(&host_creds)
            .map_err(|e| format!("failed to ensure credentials file: {e}"))?;
        let worker_home = PathBuf::from(ur_config::WORKER_HOME);
        self.volumes.push((
            host_creds,
            worker_home
                .join(".claude")
                .join(ur_config::CLAUDE_CREDENTIALS_FILENAME),
        ));
        Ok(self)
    }

    /// Add project CLAUDE.md mount and env var based on project configuration.
    ///
    /// - If `claude_md` is `None`, this is a no-op.
    /// - If the template resolves to a [`ResolvedTemplatePath::HostPath`], adds a read-only volume
    ///   mount from the host path to `/var/ur/project-claude/CLAUDE.md` and sets
    ///   `UR_PROJECT_CLAUDE=/var/ur/project-claude/CLAUDE.md`.
    /// - If the template resolves to a [`ResolvedTemplatePath::ProjectRelative`], adds no volume
    ///   mount and sets `UR_PROJECT_CLAUDE=/workspace/<rel>`.
    pub fn add_project_claude_md(
        mut self,
        claude_md: &Option<String>,
        host_config_dir: &Path,
    ) -> Result<Self, String> {
        let Some(template) = claude_md.as_deref() else {
            return Ok(self);
        };

        let resolved = resolve_template_path(template, host_config_dir)
            .map_err(|e| format!("failed to resolve claude_md: {e}"))?;

        match resolved {
            ResolvedTemplatePath::HostPath(host_path) => {
                let container_path = PathBuf::from("/var/ur/project-claude/CLAUDE.md:ro");
                self.volumes.push((host_path, container_path));
                self.env_vars.push((
                    ur_config::UR_PROJECT_CLAUDE_ENV.into(),
                    "/var/ur/project-claude/CLAUDE.md".into(),
                ));
            }
            ResolvedTemplatePath::ProjectRelative(rel_path) => {
                let container_claude = PathBuf::from("/workspace").join(&rel_path);
                self.env_vars.push((
                    ur_config::UR_PROJECT_CLAUDE_ENV.into(),
                    container_claude.to_string_lossy().into_owned(),
                ));
            }
        }

        Ok(self)
    }

    /// Add project memory directory mount.
    ///
    /// - If `memory_dir` is `None`, this is a no-op.
    /// - Otherwise, template-resolves via [`resolve_template_path`]. The result must be a
    ///   [`ResolvedTemplatePath::HostPath`]; a `ProjectRelative` result returns `Err` because
    ///   `%PROJECT%` is rejected for `memory_dir` at config validation time.
    /// - Pre-creates the host directory and chowns it to [`ur_config::WORKER_UID`] so the
    ///   non-root worker user can write to it (Docker would otherwise create the dir as root).
    ///   Errors from create/chown propagate as `Err(...)` — a missing or unwritable memory dir
    ///   would cause the mount to silently fail, so we surface the error early.
    /// - Pushes a single volume mount: host path → `/home/worker/.claude/projects/-workspace/memory`.
    /// - No env var is added; Claude Code discovers the dir by its cwd convention.
    pub fn add_memory_dir(
        mut self,
        memory_dir: &Option<String>,
        host_config_dir: &Path,
    ) -> Result<Self, String> {
        let Some(template) = memory_dir.as_deref() else {
            return Ok(self);
        };

        let resolved = resolve_template_path(template, host_config_dir)
            .map_err(|e| format!("failed to resolve memory_dir: {e}"))?;

        let host_path = match resolved {
            ResolvedTemplatePath::HostPath(host_path) => host_path,
            ResolvedTemplatePath::ProjectRelative(_) => {
                return Err("memory_dir resolved to a project-relative path, \
                     which is not supported (use an absolute path or %URCONFIG%)"
                    .to_string());
            }
        };

        std::fs::create_dir_all(&host_path)
            .map_err(|e| format!("failed to create memory_dir '{}': {e}", host_path.display()))?;
        std::os::unix::fs::chown(
            &host_path,
            Some(ur_config::WORKER_UID),
            Some(ur_config::WORKER_UID),
        )
        .map_err(|e| format!("failed to chown memory_dir '{}': {e}", host_path.display()))?;

        let container_path = PathBuf::from("/home/worker/.claude/projects/-workspace/memory");
        self.volumes.push((host_path, container_path));

        Ok(self)
    }

    /// Add project-configured volume mounts.
    ///
    /// Each mount entry has a template source (resolved to a host path) and an absolute
    /// container destination. Only `%URCONFIG%` and absolute paths are supported as sources
    /// (`%PROJECT%` is rejected at config load time).
    pub fn add_mounts(
        mut self,
        mounts: &[MountConfig],
        host_config_dir: &Path,
    ) -> Result<Self, String> {
        for mount in mounts {
            let resolved = resolve_template_path(&mount.source, host_config_dir)
                .map_err(|e| format!("failed to resolve mount source '{}': {e}", mount.source))?;
            let host_path = match resolved {
                ResolvedTemplatePath::HostPath(host_path) => host_path,
                ResolvedTemplatePath::ProjectRelative(_) => {
                    return Err(format!(
                        "mount source '{}' resolved to a project-relative path, \
                         which is not supported for mounts",
                        mount.source
                    ));
                }
            };
            let container_path = if mount.readonly {
                PathBuf::from(format!("{}:ro", mount.destination))
            } else {
                PathBuf::from(&mount.destination)
            };
            self.volumes.push((host_path, container_path));
        }
        Ok(self)
    }

    /// Add host-side per-project hook directories as read-only overlays.
    ///
    /// Mounts the following paths when they exist on the host, using fixed container
    /// destinations that workerd will read in a later ticket:
    ///
    /// - `<host_config_dir>/projects/<project_key>/hooks/git/` → `/var/ur/host-hooks/git/:ro`
    /// - `<host_config_dir>/projects/<project_key>/hooks/skills/` → `/var/ur/host-hooks/skills/:ro`
    ///
    /// Rules:
    /// - When `project_key` is empty (workspace-mode launch without a project), this is a no-op.
    /// - When a host directory does not exist, no mount is added for that type.
    /// - Both checks are independent — one mount can be added without the other.
    pub fn add_host_hooks_overlay(mut self, project_key: &str, host_config_dir: &Path) -> Self {
        if project_key.is_empty() {
            return self;
        }

        let hooks_base = host_config_dir
            .join("projects")
            .join(project_key)
            .join("hooks");

        let git_host = hooks_base.join("git");
        if git_host.exists() {
            self.volumes
                .push((git_host, PathBuf::from("/var/ur/host-hooks/git:ro")));
        }

        let skills_host = hooks_base.join("skills");
        if skills_host.exists() {
            self.volumes
                .push((skills_host, PathBuf::from("/var/ur/host-hooks/skills:ro")));
        }

        self
    }

    /// Add per-worker logs directory mount.
    ///
    /// Mounts `<host_logs_dir>/workers/<worker_id>/` from the host into
    /// `/var/ur/logs` inside the container and sets `UR_LOGS_DIR=/var/ur/logs`
    /// so workerd writes file-based logs there.
    pub fn add_logs_dir(
        mut self,
        host_logs_dir: &Path,
        local_logs_dir: &Path,
        worker_id: &str,
    ) -> Self {
        let host_path = host_logs_dir.join("workers").join(worker_id);
        let local_path = local_logs_dir.join("workers").join(worker_id);
        // Pre-create via the container-internal path and chown so the non-root
        // worker user can write logs. Without this, Docker creates bind-mount
        // source dirs as root, causing permission errors.
        if std::fs::create_dir_all(&local_path).is_ok() {
            let _ = std::os::unix::fs::chown(
                &local_path,
                Some(ur_config::WORKER_UID),
                Some(ur_config::WORKER_UID),
            );
        }
        self.volumes
            .push((host_path, PathBuf::from("/var/ur/logs")));
        self.env_vars
            .push(("UR_LOGS_DIR".into(), "/var/ur/logs".into()));
        self
    }

    /// Add context repository mounts as read-only volumes.
    ///
    /// Each entry maps a host path to `/context/<project-key>/` inside the container.
    /// The `:ro` flag prevents workers from modifying context repos.
    pub fn add_context_repos(mut self, context_mounts: &[(String, PathBuf)]) -> Self {
        for (key, host_path) in context_mounts {
            // Encode `:ro` into the container path so the Docker volume flag
            // becomes `host_path:/context/<key>:ro`.
            let container_path = PathBuf::from(format!("/context/{key}:ro"));
            self.volumes.push((host_path.clone(), container_path));
        }
        self
    }

    /// Mount each `(name, host_path)` pair at `/home/worker/.claude/potential-skills/<name>:ro`.
    ///
    /// Each mount is a read-only directory bind mount. The existing `workerd init` step
    /// iterates `UR_WORKER_SKILLS` and copies `~/.claude/potential-skills/<name>/` →
    /// `~/.claude/skills/<name>/`, so bind-mounting here causes the copy step to pick
    /// up the host skill transparently. No env vars are set by this method —
    /// `UR_WORKER_SKILLS` is set elsewhere.
    ///
    /// Empty slice → no-op (no volumes added).
    pub fn add_extra_skills(mut self, mounts: &[(String, PathBuf)]) -> Self {
        for (name, host_path) in mounts {
            let container_path =
                PathBuf::from(format!("/home/worker/.claude/potential-skills/{name}:ro"));
            self.volumes.push((host_path.clone(), container_path));
        }
        self
    }

    /// Add port mappings to the container.
    ///
    /// Each [`PortMapping`] is converted to a Docker `-p host_port:container_port` flag.
    pub fn add_ports(mut self, ports: &[PortMapping]) -> Self {
        for port in ports {
            self.port_maps.push(ProtoPortMap {
                host_port: u32::from(port.host_port),
                container_port: u32::from(port.container_port),
            });
        }
        self
    }

    /// Add bind mounts for project hostexec scripts.
    ///
    /// For each entry in `scripts`, mounts `shim_host_path` read-only over
    /// `/workspace/<rel_path>` inside the container. This causes the worker to
    /// invoke the hostexec shim whenever it runs the declared script path.
    ///
    /// - No-op when `scripts` is empty.
    /// - When `workspace_path` is provided, checks that each script exists at
    ///   `<workspace_path>/<rel_path>` and returns an error if any are missing,
    ///   preventing a silent Docker bind-mount failure.
    pub fn add_project_hostexec_scripts(
        mut self,
        scripts: &[String],
        shim_host_path: &std::path::Path,
        workspace_path: Option<&std::path::Path>,
    ) -> Result<Self, String> {
        if scripts.is_empty() {
            return Ok(self);
        }
        for rel_path in scripts {
            check_script_exists(rel_path, workspace_path)?;
            let container_path = PathBuf::from(format!("/workspace/{rel_path}:ro"));
            self.volumes
                .push((shim_host_path.to_path_buf(), container_path));
        }
        Ok(self)
    }

    /// Add environment variables to the container.
    pub fn add_env_vars(mut self, env_vars: Vec<(String, String)>) -> Self {
        self.env_vars.extend(env_vars);
        self
    }

    /// Produce the final [`LaunchWorkerRequest`].
    pub fn build(self) -> LaunchWorkerRequest {
        LaunchWorkerRequest {
            image: self.image,
            name: self.name,
            cpus: self.cpus,
            memory: self.memory,
            volumes: self
                .volumes
                .into_iter()
                .map(|(host, container)| ProtoVolume {
                    host_path: host.display().to_string(),
                    container_path: container.display().to_string(),
                })
                .collect(),
            port_maps: self.port_maps,
            env_vars: self
                .env_vars
                .into_iter()
                .map(|(key, value)| ProtoEnvVar { key, value })
                .collect(),
            workdir: self
                .workdir
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            network: self.network,
            add_hosts: vec![],
        }
    }
}

/// Pre-launch sanity check for a single hostexec script entry.
///
/// When `workspace_path` is provided, verifies that `rel_path` exists under it.
/// Returns an error message if the script is absent so callers can surface it
/// before Docker silently mounts a missing source.
fn check_script_exists(rel_path: &str, workspace_path: Option<&Path>) -> Result<(), String> {
    let Some(ws_path) = workspace_path else {
        return Ok(());
    };
    let source_path = ws_path.join(rel_path);
    if !source_path.exists() {
        return Err(format!(
            "hostexec script '{rel_path}' not found at '{}': \
             ensure the script is committed to the repository",
            source_path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_minimal() {
        let req = RunOptsBuilder::new(
            "test-image:latest".into(),
            "test-container".into(),
            "test-network".into(),
        )
        .build();

        assert_eq!(req.image, "test-image:latest");
        assert_eq!(req.name, "test-container");
        assert_eq!(req.network, "test-network");
        assert_eq!(req.cpus, 0);
        assert!(req.memory.is_empty());
        assert!(req.volumes.is_empty());
        assert!(req.env_vars.is_empty());
        assert!(req.workdir.is_empty());
    }

    #[test]
    fn build_with_all_basic_config() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .cpus(4)
            .memory("8g".into())
            .workdir("/workspace")
            .build();

        assert_eq!(req.cpus, 4);
        assert_eq!(req.memory, "8g");
        assert_eq!(req.workdir, "/workspace");
    }

    #[test]
    fn add_workspace_with_some_path() {
        let ws = Some(PathBuf::from("/host/workspace"));
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_workspace(&ws)
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, "/host/workspace");
        assert_eq!(req.volumes[0].container_path, "/workspace");
    }

    #[test]
    fn add_workspace_with_none_is_noop() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_workspace(&None)
            .build();

        assert!(req.volumes.is_empty());
    }

    #[test]
    fn add_credentials_creates_mount() {
        let tmp = tempfile::tempdir().unwrap();
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_credentials(tmp.path())
            .unwrap()
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert!(req.volumes[0].host_path.ends_with(".credentials.json"));
        assert!(req.volumes[0].container_path.ends_with(".credentials.json"));
        // Verify the file was created on disk
        assert!(PathBuf::from(&req.volumes[0].host_path).exists());
    }

    #[test]
    fn add_env_vars_accumulates() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_env_vars(vec![("A".into(), "1".into())])
            .add_env_vars(vec![("B".into(), "2".into())])
            .build();

        assert_eq!(req.env_vars.len(), 2);
        assert_eq!(req.env_vars[0].key, "A");
        assert_eq!(req.env_vars[0].value, "1");
        assert_eq!(req.env_vars[1].key, "B");
        assert_eq!(req.env_vars[1].value, "2");
    }

    #[test]
    fn build_always_sets_empty_defaults() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into()).build();

        assert!(req.port_maps.is_empty());
        assert!(req.add_hosts.is_empty());
    }

    #[test]
    fn add_mounts_empty_is_noop() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&[], Path::new("/unused"))
            .unwrap()
            .build();

        assert!(req.volumes.is_empty());
    }

    #[test]
    fn add_mounts_absolute_source() {
        let mounts = vec![MountConfig {
            source: "/host/tickets".into(),
            destination: "/workspace/.tickets".into(),
            readonly: false,
        }];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&mounts, Path::new("/unused"))
            .unwrap()
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, "/host/tickets");
        assert_eq!(req.volumes[0].container_path, "/workspace/.tickets");
    }

    #[test]
    fn add_mounts_urconfig_source() {
        let mounts = vec![MountConfig {
            source: "%URCONFIG%/shared-data".into(),
            destination: "/var/data".into(),
            readonly: false,
        }];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&mounts, Path::new("/home/user/.ur"))
            .unwrap()
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, "/home/user/.ur/shared-data");
        assert_eq!(req.volumes[0].container_path, "/var/data");
    }

    #[test]
    fn add_mounts_multiple() {
        let mounts = vec![
            MountConfig {
                source: "/host/a".into(),
                destination: "/container/a".into(),
                readonly: false,
            },
            MountConfig {
                source: "/host/b".into(),
                destination: "/container/b".into(),
                readonly: false,
            },
        ];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&mounts, Path::new("/unused"))
            .unwrap()
            .build();

        assert_eq!(req.volumes.len(), 2);
        assert_eq!(req.volumes[0].host_path, "/host/a");
        assert_eq!(req.volumes[0].container_path, "/container/a");
        assert_eq!(req.volumes[1].host_path, "/host/b");
        assert_eq!(req.volumes[1].container_path, "/container/b");
    }

    #[test]
    fn add_mounts_readonly_appends_ro_suffix() {
        let mounts = vec![
            MountConfig {
                source: "/host/a".into(),
                destination: "/container/a".into(),
                readonly: true,
            },
            MountConfig {
                source: "/host/b".into(),
                destination: "/container/b".into(),
                readonly: false,
            },
        ];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&mounts, Path::new("/unused"))
            .unwrap()
            .build();

        assert_eq!(req.volumes.len(), 2);
        assert_eq!(req.volumes[0].host_path, "/host/a");
        assert_eq!(req.volumes[0].container_path, "/container/a:ro");
        assert_eq!(req.volumes[1].host_path, "/host/b");
        assert_eq!(req.volumes[1].container_path, "/container/b");
    }

    #[test]
    fn add_logs_dir_creates_mount_and_env() {
        let tmp = tempfile::tempdir().unwrap();
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_logs_dir(Path::new("/home/user/.ur/logs"), tmp.path(), "worker-ab12")
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(
            req.volumes[0].host_path,
            "/home/user/.ur/logs/workers/worker-ab12"
        );
        assert_eq!(req.volumes[0].container_path, "/var/ur/logs");
        assert_eq!(req.env_vars.len(), 1);
        assert_eq!(req.env_vars[0].key, "UR_LOGS_DIR");
        assert_eq!(req.env_vars[0].value, "/var/ur/logs");
    }

    #[test]
    fn add_context_repos_empty_is_noop() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_context_repos(&[])
            .build();

        assert!(req.volumes.is_empty());
    }

    #[test]
    fn add_context_repos_single() {
        let mounts = vec![("frontend".into(), PathBuf::from("/host/pool/frontend/0"))];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_context_repos(&mounts)
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, "/host/pool/frontend/0");
        assert_eq!(req.volumes[0].container_path, "/context/frontend:ro");
    }

    #[test]
    fn add_context_repos_multiple() {
        let mounts = vec![
            ("frontend".into(), PathBuf::from("/host/pool/frontend/0")),
            ("backend".into(), PathBuf::from("/host/pool/backend/1")),
        ];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_context_repos(&mounts)
            .build();

        assert_eq!(req.volumes.len(), 2);
        assert_eq!(req.volumes[0].host_path, "/host/pool/frontend/0");
        assert_eq!(req.volumes[0].container_path, "/context/frontend:ro");
        assert_eq!(req.volumes[1].host_path, "/host/pool/backend/1");
        assert_eq!(req.volumes[1].container_path, "/context/backend:ro");
    }

    #[test]
    fn add_ports_empty_is_noop() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_ports(&[])
            .build();

        assert!(req.port_maps.is_empty());
    }

    #[test]
    fn add_ports_single() {
        let ports = vec![PortMapping {
            host_port: 8080,
            container_port: 80,
        }];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_ports(&ports)
            .build();

        assert_eq!(req.port_maps.len(), 1);
        assert_eq!(req.port_maps[0].host_port, 8080u32);
        assert_eq!(req.port_maps[0].container_port, 80u32);
    }

    #[test]
    fn add_ports_multiple() {
        let ports = vec![
            PortMapping {
                host_port: 8080,
                container_port: 80,
            },
            PortMapping {
                host_port: 3000,
                container_port: 3000,
            },
        ];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_ports(&ports)
            .build();

        assert_eq!(req.port_maps.len(), 2);
        assert_eq!(req.port_maps[0].host_port, 8080u32);
        assert_eq!(req.port_maps[0].container_port, 80u32);
        assert_eq!(req.port_maps[1].host_port, 3000u32);
        assert_eq!(req.port_maps[1].container_port, 3000u32);
    }

    #[test]
    fn add_project_claude_md_none_is_noop() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_claude_md(&None, Path::new("/unused"))
            .unwrap()
            .build();

        assert!(req.volumes.is_empty());
        assert!(req.env_vars.is_empty());
    }

    #[test]
    fn add_project_claude_md_host_path_adds_mount_and_env() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_claude_md(
                &Some("/opt/claude/ur/CLAUDE.md".into()),
                Path::new("/unused"),
            )
            .unwrap()
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, "/opt/claude/ur/CLAUDE.md");
        assert_eq!(
            req.volumes[0].container_path,
            "/var/ur/project-claude/CLAUDE.md:ro"
        );
        assert_eq!(req.env_vars.len(), 1);
        assert_eq!(req.env_vars[0].key, "UR_PROJECT_CLAUDE");
        assert_eq!(req.env_vars[0].value, "/var/ur/project-claude/CLAUDE.md");
    }

    #[test]
    fn add_project_claude_md_urconfig_adds_mount_and_env() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_claude_md(
                &Some("%URCONFIG%/projects/ur/CLAUDE.md".into()),
                Path::new("/home/user/.ur"),
            )
            .unwrap()
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(
            req.volumes[0].host_path,
            "/home/user/.ur/projects/ur/CLAUDE.md"
        );
        assert_eq!(
            req.volumes[0].container_path,
            "/var/ur/project-claude/CLAUDE.md:ro"
        );
        assert_eq!(req.env_vars.len(), 1);
        assert_eq!(req.env_vars[0].key, "UR_PROJECT_CLAUDE");
        assert_eq!(req.env_vars[0].value, "/var/ur/project-claude/CLAUDE.md");
    }

    #[test]
    fn add_project_claude_md_project_relative_no_mount() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_claude_md(&Some("%PROJECT%/CLAUDE.md".into()), Path::new("/unused"))
            .unwrap()
            .build();

        assert!(req.volumes.is_empty());
        assert_eq!(req.env_vars.len(), 1);
        assert_eq!(req.env_vars[0].key, "UR_PROJECT_CLAUDE");
        assert_eq!(req.env_vars[0].value, "/workspace/CLAUDE.md");
    }

    #[test]
    fn add_project_hostexec_scripts_empty_is_noop() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_hostexec_scripts(&[], Path::new("/unused/shim.sh"), None)
            .unwrap()
            .build();

        assert!(req.volumes.is_empty());
    }

    #[test]
    fn add_project_hostexec_scripts_multi_entry_mounts() {
        let tmp = tempfile::tempdir().unwrap();
        let shim_path = tmp.path().join("script-shim.sh");
        std::fs::write(&shim_path, "#!/bin/sh\n").unwrap();

        let scripts = vec!["scripts/deploy.sh".to_string(), "tools/lint.sh".to_string()];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_hostexec_scripts(&scripts, &shim_path, None)
            .unwrap()
            .build();

        let shim_str = shim_path.display().to_string();
        assert_eq!(req.volumes.len(), 2);
        assert_eq!(req.volumes[0].host_path, shim_str);
        assert_eq!(
            req.volumes[0].container_path,
            "/workspace/scripts/deploy.sh:ro"
        );
        assert_eq!(req.volumes[1].host_path, shim_str);
        assert_eq!(req.volumes[1].container_path, "/workspace/tools/lint.sh:ro");
    }

    #[test]
    fn add_project_hostexec_scripts_no_workspace_skips_existence_check() {
        // When workspace_path is None, no existence check is done — mount is added regardless.
        let shim_path = Path::new("/nonexistent/shim.sh");
        let scripts = vec!["run.sh".to_string()];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_hostexec_scripts(&scripts, shim_path, None)
            .unwrap()
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, "/nonexistent/shim.sh");
        assert_eq!(req.volumes[0].container_path, "/workspace/run.sh:ro");
    }

    #[test]
    fn add_project_hostexec_scripts_missing_source_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let shim_path = tmp.path().join("script-shim.sh");
        std::fs::write(&shim_path, "#!/bin/sh\n").unwrap();

        // Workspace exists but script does not.
        let scripts = vec!["scripts/deploy.sh".to_string()];
        let result = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_hostexec_scripts(&scripts, &shim_path, Some(tmp.path()));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("scripts/deploy.sh"),
            "error should name the missing script: {err}"
        );
        assert!(
            err.contains("not found"),
            "error should say 'not found': {err}"
        );
    }

    #[test]
    fn add_project_hostexec_scripts_present_source_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let shim_path = tmp.path().join("script-shim.sh");
        std::fs::write(&shim_path, "#!/bin/sh\n").unwrap();

        // Create the script in the workspace.
        let scripts_dir = tmp.path().join("scripts");
        std::fs::create_dir_all(&scripts_dir).unwrap();
        std::fs::write(scripts_dir.join("deploy.sh"), "#!/bin/sh\necho deploy\n").unwrap();

        let scripts = vec!["scripts/deploy.sh".to_string()];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_hostexec_scripts(&scripts, &shim_path, Some(tmp.path()))
            .unwrap()
            .build();

        let shim_str = shim_path.display().to_string();
        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, shim_str);
        assert_eq!(
            req.volumes[0].container_path,
            "/workspace/scripts/deploy.sh:ro"
        );
    }

    #[test]
    fn add_extra_skills_empty_is_noop() {
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_extra_skills(&[])
            .build();

        assert!(req.volumes.is_empty());
        assert!(req.env_vars.is_empty());
    }

    #[test]
    fn add_extra_skills_single_entry_correct_container_path() {
        let mounts = vec![(
            "my-skill".to_string(),
            PathBuf::from("/host/skills/my-skill"),
        )];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_extra_skills(&mounts)
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, "/host/skills/my-skill");
        assert_eq!(
            req.volumes[0].container_path,
            "/home/worker/.claude/potential-skills/my-skill:ro"
        );
        assert!(req.env_vars.is_empty());
    }

    #[test]
    fn add_extra_skills_multiple_entries_preserved_in_order() {
        let mounts = vec![
            ("alpha".to_string(), PathBuf::from("/host/skills/alpha")),
            ("beta".to_string(), PathBuf::from("/host/skills/beta")),
            ("gamma".to_string(), PathBuf::from("/host/skills/gamma")),
        ];
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_extra_skills(&mounts)
            .build();

        assert_eq!(req.volumes.len(), 3);
        assert_eq!(req.volumes[0].host_path, "/host/skills/alpha");
        assert_eq!(
            req.volumes[0].container_path,
            "/home/worker/.claude/potential-skills/alpha:ro"
        );
        assert_eq!(req.volumes[1].host_path, "/host/skills/beta");
        assert_eq!(
            req.volumes[1].container_path,
            "/home/worker/.claude/potential-skills/beta:ro"
        );
        assert_eq!(req.volumes[2].host_path, "/host/skills/gamma");
        assert_eq!(
            req.volumes[2].container_path,
            "/home/worker/.claude/potential-skills/gamma:ro"
        );
        assert!(req.env_vars.is_empty());
    }

    #[test]
    fn add_memory_dir_none_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_memory_dir(&None, tmp.path())
            .unwrap()
            .build();

        assert!(req.volumes.is_empty());
        assert!(req.env_vars.is_empty());
    }

    /// Test the HostPath success path: directory is created if missing, chowned,
    /// and a single volume mount is pushed.
    ///
    /// Restricted to Linux because `chown` to `WORKER_UID` (1000) requires either
    /// running as root or being that user; on macOS the test runner is typically a
    /// different uid and the syscall returns EPERM.
    #[cfg(target_os = "linux")]
    #[test]
    fn add_memory_dir_host_path_creates_dir_and_mounts() {
        let tmp = tempfile::tempdir().unwrap();
        let host_config_dir = tmp.path();
        // Use an absolute path for the memory dir (no pre-existing directory).
        let memory_host_path = tmp.path().join("shared_memory");
        let memory_dir = Some(memory_host_path.display().to_string());

        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_memory_dir(&memory_dir, host_config_dir)
            .unwrap()
            .build();

        // Directory should have been created.
        assert!(
            memory_host_path.exists(),
            "memory dir should be created on host"
        );

        // Exactly one volume, no env vars.
        assert_eq!(req.volumes.len(), 1, "expected one volume mount");
        assert_eq!(
            req.volumes[0].host_path,
            memory_host_path.display().to_string()
        );
        assert_eq!(
            req.volumes[0].container_path,
            "/home/worker/.claude/projects/-workspace/memory"
        );
        assert!(req.env_vars.is_empty(), "no env vars should be added");
    }

    #[test]
    fn add_host_hooks_overlay_empty_project_key_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        // Create the directories so existence is not the reason for no mount.
        let hooks_git = tmp
            .path()
            .join("projects")
            .join("")
            .join("hooks")
            .join("git");
        // Can't create a path with empty project key component meaningfully,
        // just verify empty key returns no volumes regardless.
        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_host_hooks_overlay("", tmp.path())
            .build();

        assert!(
            req.volumes.is_empty(),
            "empty project key should add no mounts: {:?}",
            req.volumes
        );
        let _ = hooks_git; // suppress unused warning
    }

    #[test]
    fn add_host_hooks_overlay_both_dirs_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp
            .path()
            .join("projects")
            .join("myproj")
            .join("hooks")
            .join("git");
        let skills_dir = tmp
            .path()
            .join("projects")
            .join("myproj")
            .join("hooks")
            .join("skills");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::create_dir_all(&skills_dir).unwrap();

        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_host_hooks_overlay("myproj", tmp.path())
            .build();

        assert_eq!(req.volumes.len(), 2);
        assert_eq!(req.volumes[0].host_path, git_dir.display().to_string());
        assert_eq!(req.volumes[0].container_path, "/var/ur/host-hooks/git:ro");
        assert_eq!(req.volumes[1].host_path, skills_dir.display().to_string());
        assert_eq!(
            req.volumes[1].container_path,
            "/var/ur/host-hooks/skills:ro"
        );
    }

    #[test]
    fn add_host_hooks_overlay_only_git_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp
            .path()
            .join("projects")
            .join("myproj")
            .join("hooks")
            .join("git");
        std::fs::create_dir_all(&git_dir).unwrap();
        // skills dir intentionally NOT created

        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_host_hooks_overlay("myproj", tmp.path())
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, git_dir.display().to_string());
        assert_eq!(req.volumes[0].container_path, "/var/ur/host-hooks/git:ro");
    }

    #[test]
    fn add_host_hooks_overlay_only_skills_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp
            .path()
            .join("projects")
            .join("myproj")
            .join("hooks")
            .join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        // git dir intentionally NOT created

        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_host_hooks_overlay("myproj", tmp.path())
            .build();

        assert_eq!(req.volumes.len(), 1);
        assert_eq!(req.volumes[0].host_path, skills_dir.display().to_string());
        assert_eq!(
            req.volumes[0].container_path,
            "/var/ur/host-hooks/skills:ro"
        );
    }

    #[test]
    fn add_host_hooks_overlay_neither_exists_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        // No hooks directories created at all

        let req = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_host_hooks_overlay("myproj", tmp.path())
            .build();

        assert!(
            req.volumes.is_empty(),
            "no existing dirs should produce no mounts: {:?}",
            req.volumes
        );
    }

    #[test]
    fn add_memory_dir_project_relative_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        // %PROJECT%/... resolves to ProjectRelative — should be rejected defensively.
        let memory_dir = Some("%PROJECT%/memory".to_string());

        let result = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_memory_dir(&memory_dir, tmp.path());

        assert!(result.is_err(), "ProjectRelative should return an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("project-relative"),
            "error should mention project-relative: {err}"
        );
    }
}
