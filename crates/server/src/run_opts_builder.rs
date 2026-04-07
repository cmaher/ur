use std::path::{Path, PathBuf};

use container::{PortMap, RunOpts};
use ur_config::{MountConfig, PortMapping, ResolvedTemplatePath, resolve_template_path};

use crate::worker::ensure_file_exists;

/// Builder that accumulates volumes, env vars, and config to produce a [`container::RunOpts`].
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
    port_maps: Vec<PortMap>,
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

    /// Add git hooks volume mount and env var based on project configuration.
    ///
    /// - If `git_hooks_dir` is `None`, defaults to `%PROJECT%/ur-hooks/git` (convention path),
    ///   setting `UR_GIT_HOOKS_DIR=/workspace/ur-hooks/git` with no volume mount.
    /// - If the template resolves to a [`ResolvedTemplatePath::HostPath`], adds a volume mount
    ///   from the host path to `/var/ur/git-hooks` and sets `UR_GIT_HOOKS_DIR=/var/ur/git-hooks`.
    /// - If the template resolves to a [`ResolvedTemplatePath::ProjectRelative`], adds no volume
    ///   mount and sets `UR_GIT_HOOKS_DIR=/workspace/<path>`.
    pub fn add_git_hooks(
        mut self,
        git_hooks_dir: &Option<String>,
        host_config_dir: &Path,
    ) -> Result<Self, String> {
        let default_template = "%PROJECT%/ur-hooks/git".to_string();
        let template = git_hooks_dir.as_deref().unwrap_or(&default_template);

        let resolved = resolve_template_path(template, host_config_dir)
            .map_err(|e| format!("failed to resolve git_hooks_dir: {e}"))?;

        match resolved {
            ResolvedTemplatePath::HostPath(host_path) => {
                let container_path = PathBuf::from("/var/ur/git-hooks");
                self.volumes.push((host_path, container_path));
                self.env_vars
                    .push(("UR_GIT_HOOKS_DIR".into(), "/var/ur/git-hooks".into()));
            }
            ResolvedTemplatePath::ProjectRelative(rel_path) => {
                let container_hooks_dir = PathBuf::from("/workspace").join(&rel_path);
                self.env_vars.push((
                    "UR_GIT_HOOKS_DIR".into(),
                    container_hooks_dir.to_string_lossy().into_owned(),
                ));
            }
        }

        Ok(self)
    }

    /// Add skill hooks volume mount and env var based on project configuration.
    ///
    /// - If `skill_hooks_dir` is `None`, this is a no-op.
    /// - If the template resolves to a [`ResolvedTemplatePath::HostPath`], adds a volume mount
    ///   from the host path to `/var/ur/skill-hooks` and sets `UR_SKILL_HOOKS_DIR=/var/ur/skill-hooks`.
    /// - If the template resolves to a [`ResolvedTemplatePath::ProjectRelative`], adds no volume
    ///   mount and sets `UR_SKILL_HOOKS_DIR=/workspace/<path>`.
    pub fn add_skill_hooks(
        mut self,
        skill_hooks_dir: &Option<String>,
        host_config_dir: &Path,
    ) -> Result<Self, String> {
        let Some(template) = skill_hooks_dir.as_deref() else {
            return Ok(self);
        };

        let resolved = resolve_template_path(template, host_config_dir)
            .map_err(|e| format!("failed to resolve skill_hooks_dir: {e}"))?;

        match resolved {
            ResolvedTemplatePath::HostPath(host_path) => {
                let container_path = PathBuf::from("/var/ur/skill-hooks");
                self.volumes.push((host_path, container_path));
                self.env_vars
                    .push(("UR_SKILL_HOOKS_DIR".into(), "/var/ur/skill-hooks".into()));
            }
            ResolvedTemplatePath::ProjectRelative(rel_path) => {
                let container_hooks_dir = PathBuf::from("/workspace").join(&rel_path);
                self.env_vars.push((
                    "UR_SKILL_HOOKS_DIR".into(),
                    container_hooks_dir.to_string_lossy().into_owned(),
                ));
            }
        }

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

    /// Add port mappings to the container.
    ///
    /// Each [`PortMapping`] is converted to a Docker `-p host_port:container_port` flag.
    pub fn add_ports(mut self, ports: &[PortMapping]) -> Self {
        for port in ports {
            self.port_maps.push(PortMap {
                host_port: port.host_port,
                container_port: port.container_port,
            });
        }
        self
    }

    /// Add environment variables to the container.
    pub fn add_env_vars(mut self, env_vars: Vec<(String, String)>) -> Self {
        self.env_vars.extend(env_vars);
        self
    }

    /// Produce the final [`RunOpts`].
    pub fn build(self) -> RunOpts {
        RunOpts {
            image: container::ImageId(self.image),
            name: self.name,
            cpus: self.cpus,
            memory: self.memory,
            volumes: self.volumes,
            port_maps: self.port_maps,
            env_vars: self.env_vars,
            workdir: self.workdir,
            command: vec![],
            network: Some(self.network),
            add_hosts: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_minimal() {
        let opts = RunOptsBuilder::new(
            "test-image:latest".into(),
            "test-container".into(),
            "test-network".into(),
        )
        .build();

        assert_eq!(opts.image.0, "test-image:latest");
        assert_eq!(opts.name, "test-container");
        assert_eq!(opts.network, Some("test-network".into()));
        assert_eq!(opts.cpus, 0);
        assert!(opts.memory.is_empty());
        assert!(opts.volumes.is_empty());
        assert!(opts.env_vars.is_empty());
        assert!(opts.workdir.is_none());
    }

    #[test]
    fn build_with_all_basic_config() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .cpus(4)
            .memory("8g".into())
            .workdir("/workspace")
            .build();

        assert_eq!(opts.cpus, 4);
        assert_eq!(opts.memory, "8g");
        assert_eq!(opts.workdir, Some(PathBuf::from("/workspace")));
    }

    #[test]
    fn add_workspace_with_some_path() {
        let ws = Some(PathBuf::from("/host/workspace"));
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_workspace(&ws)
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(opts.volumes[0].0, PathBuf::from("/host/workspace"));
        assert_eq!(opts.volumes[0].1, PathBuf::from("/workspace"));
    }

    #[test]
    fn add_workspace_with_none_is_noop() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_workspace(&None)
            .build();

        assert!(opts.volumes.is_empty());
    }

    #[test]
    fn add_credentials_creates_mount() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_credentials(tmp.path())
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 1);
        let (host, container) = &opts.volumes[0];
        assert!(host.ends_with(".credentials.json"));
        assert!(container.ends_with(".credentials.json"));
        // Verify the file was created on disk
        assert!(host.exists());
    }

    #[test]
    fn add_env_vars_accumulates() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_env_vars(vec![("A".into(), "1".into())])
            .add_env_vars(vec![("B".into(), "2".into())])
            .build();

        assert_eq!(opts.env_vars.len(), 2);
        assert_eq!(opts.env_vars[0], ("A".into(), "1".into()));
        assert_eq!(opts.env_vars[1], ("B".into(), "2".into()));
    }

    #[test]
    fn build_always_sets_empty_defaults() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into()).build();

        assert!(opts.port_maps.is_empty());
        assert!(opts.command.is_empty());
        assert!(opts.add_hosts.is_empty());
    }

    #[test]
    fn add_mounts_empty_is_noop() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&[], Path::new("/unused"))
            .unwrap()
            .build();

        assert!(opts.volumes.is_empty());
    }

    #[test]
    fn add_mounts_absolute_source() {
        let mounts = vec![MountConfig {
            source: "/host/tickets".into(),
            destination: "/workspace/.tickets".into(),
            readonly: false,
        }];
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&mounts, Path::new("/unused"))
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(opts.volumes[0].0, PathBuf::from("/host/tickets"));
        assert_eq!(opts.volumes[0].1, PathBuf::from("/workspace/.tickets"));
    }

    #[test]
    fn add_mounts_urconfig_source() {
        let mounts = vec![MountConfig {
            source: "%URCONFIG%/shared-data".into(),
            destination: "/var/data".into(),
            readonly: false,
        }];
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&mounts, Path::new("/home/user/.ur"))
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(
            opts.volumes[0].0,
            PathBuf::from("/home/user/.ur/shared-data")
        );
        assert_eq!(opts.volumes[0].1, PathBuf::from("/var/data"));
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
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&mounts, Path::new("/unused"))
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 2);
        assert_eq!(opts.volumes[0].0, PathBuf::from("/host/a"));
        assert_eq!(opts.volumes[0].1, PathBuf::from("/container/a"));
        assert_eq!(opts.volumes[1].0, PathBuf::from("/host/b"));
        assert_eq!(opts.volumes[1].1, PathBuf::from("/container/b"));
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
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_mounts(&mounts, Path::new("/unused"))
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 2);
        assert_eq!(opts.volumes[0].0, PathBuf::from("/host/a"));
        assert_eq!(opts.volumes[0].1, PathBuf::from("/container/a:ro"));
        assert_eq!(opts.volumes[1].0, PathBuf::from("/host/b"));
        assert_eq!(opts.volumes[1].1, PathBuf::from("/container/b"));
    }

    #[test]
    fn add_git_hooks_none_defaults_to_convention_path() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_git_hooks(&None, Path::new("/unused"))
            .unwrap()
            .build();

        assert!(opts.volumes.is_empty());
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_GIT_HOOKS_DIR");
        assert_eq!(opts.env_vars[0].1, "/workspace/ur-hooks/git");
    }

    #[test]
    fn add_git_hooks_host_path_adds_mount_and_env() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_git_hooks(
                &Some("%URCONFIG%/hooks/myproject".into()),
                Path::new("/home/user/.ur"),
            )
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(
            opts.volumes[0].0,
            PathBuf::from("/home/user/.ur/hooks/myproject")
        );
        assert_eq!(opts.volumes[0].1, PathBuf::from("/var/ur/git-hooks"));
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_GIT_HOOKS_DIR");
        assert_eq!(opts.env_vars[0].1, "/var/ur/git-hooks");
    }

    #[test]
    fn add_git_hooks_absolute_path_adds_mount_and_env() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_git_hooks(
                &Some("/opt/git-hooks/myproject".into()),
                Path::new("/unused"),
            )
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(opts.volumes[0].0, PathBuf::from("/opt/git-hooks/myproject"));
        assert_eq!(opts.volumes[0].1, PathBuf::from("/var/ur/git-hooks"));
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_GIT_HOOKS_DIR");
        assert_eq!(opts.env_vars[0].1, "/var/ur/git-hooks");
    }

    #[test]
    fn add_git_hooks_project_relative_no_mount() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_git_hooks(&Some("%PROJECT%/.git-hooks".into()), Path::new("/unused"))
            .unwrap()
            .build();

        assert!(opts.volumes.is_empty());
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_GIT_HOOKS_DIR");
        assert_eq!(opts.env_vars[0].1, "/workspace/.git-hooks");
    }

    #[test]
    fn add_skill_hooks_none_is_noop() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_skill_hooks(&None, Path::new("/unused"))
            .unwrap()
            .build();

        assert!(opts.volumes.is_empty());
        assert!(opts.env_vars.is_empty());
    }

    #[test]
    fn add_skill_hooks_host_path_adds_mount_and_env() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_skill_hooks(
                &Some("%URCONFIG%/skill-hooks".into()),
                Path::new("/home/user/.ur"),
            )
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(
            opts.volumes[0].0,
            PathBuf::from("/home/user/.ur/skill-hooks")
        );
        assert_eq!(opts.volumes[0].1, PathBuf::from("/var/ur/skill-hooks"));
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_SKILL_HOOKS_DIR");
        assert_eq!(opts.env_vars[0].1, "/var/ur/skill-hooks");
    }

    #[test]
    fn add_skill_hooks_project_relative_no_mount() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_skill_hooks(
                &Some("%PROJECT%/ur-hooks/skills".into()),
                Path::new("/unused"),
            )
            .unwrap()
            .build();

        assert!(opts.volumes.is_empty());
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_SKILL_HOOKS_DIR");
        assert_eq!(opts.env_vars[0].1, "/workspace/ur-hooks/skills");
    }

    #[test]
    fn add_logs_dir_creates_mount_and_env() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_logs_dir(Path::new("/home/user/.ur/logs"), tmp.path(), "worker-ab12")
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(
            opts.volumes[0].0,
            PathBuf::from("/home/user/.ur/logs/workers/worker-ab12")
        );
        assert_eq!(opts.volumes[0].1, PathBuf::from("/var/ur/logs"));
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_LOGS_DIR");
        assert_eq!(opts.env_vars[0].1, "/var/ur/logs");
    }

    #[test]
    fn add_context_repos_empty_is_noop() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_context_repos(&[])
            .build();

        assert!(opts.volumes.is_empty());
    }

    #[test]
    fn add_context_repos_single() {
        let mounts = vec![("frontend".into(), PathBuf::from("/host/pool/frontend/0"))];
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_context_repos(&mounts)
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(opts.volumes[0].0, PathBuf::from("/host/pool/frontend/0"));
        assert_eq!(opts.volumes[0].1, PathBuf::from("/context/frontend:ro"));
    }

    #[test]
    fn add_context_repos_multiple() {
        let mounts = vec![
            ("frontend".into(), PathBuf::from("/host/pool/frontend/0")),
            ("backend".into(), PathBuf::from("/host/pool/backend/1")),
        ];
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_context_repos(&mounts)
            .build();

        assert_eq!(opts.volumes.len(), 2);
        assert_eq!(opts.volumes[0].0, PathBuf::from("/host/pool/frontend/0"));
        assert_eq!(opts.volumes[0].1, PathBuf::from("/context/frontend:ro"));
        assert_eq!(opts.volumes[1].0, PathBuf::from("/host/pool/backend/1"));
        assert_eq!(opts.volumes[1].1, PathBuf::from("/context/backend:ro"));
    }

    #[test]
    fn add_ports_empty_is_noop() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_ports(&[])
            .build();

        assert!(opts.port_maps.is_empty());
    }

    #[test]
    fn add_ports_single() {
        let ports = vec![PortMapping {
            host_port: 8080,
            container_port: 80,
        }];
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_ports(&ports)
            .build();

        assert_eq!(opts.port_maps.len(), 1);
        assert_eq!(opts.port_maps[0].host_port, 8080);
        assert_eq!(opts.port_maps[0].container_port, 80);
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
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_ports(&ports)
            .build();

        assert_eq!(opts.port_maps.len(), 2);
        assert_eq!(opts.port_maps[0].host_port, 8080);
        assert_eq!(opts.port_maps[0].container_port, 80);
        assert_eq!(opts.port_maps[1].host_port, 3000);
        assert_eq!(opts.port_maps[1].container_port, 3000);
    }

    #[test]
    fn add_project_claude_md_none_is_noop() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_claude_md(&None, Path::new("/unused"))
            .unwrap()
            .build();

        assert!(opts.volumes.is_empty());
        assert!(opts.env_vars.is_empty());
    }

    #[test]
    fn add_project_claude_md_host_path_adds_mount_and_env() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_claude_md(
                &Some("/opt/claude/ur/CLAUDE.md".into()),
                Path::new("/unused"),
            )
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(opts.volumes[0].0, PathBuf::from("/opt/claude/ur/CLAUDE.md"));
        assert_eq!(
            opts.volumes[0].1,
            PathBuf::from("/var/ur/project-claude/CLAUDE.md:ro")
        );
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_PROJECT_CLAUDE");
        assert_eq!(opts.env_vars[0].1, "/var/ur/project-claude/CLAUDE.md");
    }

    #[test]
    fn add_project_claude_md_urconfig_adds_mount_and_env() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_claude_md(
                &Some("%URCONFIG%/projects/ur/CLAUDE.md".into()),
                Path::new("/home/user/.ur"),
            )
            .unwrap()
            .build();

        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(
            opts.volumes[0].0,
            PathBuf::from("/home/user/.ur/projects/ur/CLAUDE.md")
        );
        assert_eq!(
            opts.volumes[0].1,
            PathBuf::from("/var/ur/project-claude/CLAUDE.md:ro")
        );
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_PROJECT_CLAUDE");
        assert_eq!(opts.env_vars[0].1, "/var/ur/project-claude/CLAUDE.md");
    }

    #[test]
    fn add_project_claude_md_project_relative_no_mount() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_project_claude_md(&Some("%PROJECT%/CLAUDE.md".into()), Path::new("/unused"))
            .unwrap()
            .build();

        assert!(opts.volumes.is_empty());
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0].0, "UR_PROJECT_CLAUDE");
        assert_eq!(opts.env_vars[0].1, "/workspace/CLAUDE.md");
    }
}
