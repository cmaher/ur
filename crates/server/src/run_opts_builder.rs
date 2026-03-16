use std::path::{Path, PathBuf};

use container::RunOpts;
use ur_config::{MountConfig, ResolvedTemplatePath, resolve_template_path};

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
    /// - If `git_hooks_dir` is `None`, this is a no-op.
    /// - If the template resolves to a [`ResolvedTemplatePath::HostPath`], adds a volume mount
    ///   from the host path to `/var/ur/git-hooks` and sets `UR_GIT_HOOKS_DIR=/var/ur/git-hooks`.
    /// - If the template resolves to a [`ResolvedTemplatePath::ProjectRelative`], adds no volume
    ///   mount and sets `UR_GIT_HOOKS_DIR=/workspace/<path>`.
    pub fn add_git_hooks(
        mut self,
        git_hooks_dir: &Option<String>,
        host_config_dir: &Path,
    ) -> Result<Self, String> {
        let Some(template) = git_hooks_dir.as_deref() else {
            return Ok(self);
        };

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
            match resolved {
                ResolvedTemplatePath::HostPath(host_path) => {
                    self.volumes
                        .push((host_path, PathBuf::from(&mount.destination)));
                }
                ResolvedTemplatePath::ProjectRelative(_) => {
                    return Err(format!(
                        "mount source '{}' resolved to a project-relative path, \
                         which is not supported for mounts",
                        mount.source
                    ));
                }
            }
        }
        Ok(self)
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
            port_maps: vec![],
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
            },
            MountConfig {
                source: "/host/b".into(),
                destination: "/container/b".into(),
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
    fn add_git_hooks_none_is_noop() {
        let opts = RunOptsBuilder::new("img".into(), "name".into(), "net".into())
            .add_git_hooks(&None, Path::new("/unused"))
            .unwrap()
            .build();

        assert!(opts.volumes.is_empty());
        assert!(opts.env_vars.is_empty());
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
}
