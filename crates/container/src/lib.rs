mod docker;

use std::path::PathBuf;

use anyhow::Result;

pub use docker::DockerRuntime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerId(pub String);

/// Maps a host TCP port to a container TCP port (`-p host_port:container_port`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortMap {
    pub host_port: u16,
    pub container_port: u16,
}

#[derive(Debug, Clone)]
pub struct BuildOpts {
    pub tag: String,
    pub dockerfile: PathBuf,
    pub context: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RunOpts {
    pub image: ImageId,
    pub name: String,
    pub cpus: u32,
    pub memory: String,
    pub volumes: Vec<(PathBuf, PathBuf)>,
    pub port_maps: Vec<PortMap>,
    pub env_vars: Vec<(String, String)>,
    pub workdir: Option<PathBuf>,
    pub command: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ExecOpts {
    pub command: Vec<String>,
    pub workdir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Container name prefix used for all ur-managed agent containers.
pub const AGENT_CONTAINER_PREFIX: &str = "ur-agent-";

pub trait ContainerRuntime {
    fn build(&self, opts: &BuildOpts) -> Result<ImageId>;
    fn run(&self, opts: &RunOpts) -> Result<ContainerId>;
    fn stop(&self, id: &ContainerId) -> Result<()>;
    fn rm(&self, id: &ContainerId) -> Result<()>;
    /// List running containers whose name starts with `prefix`.
    fn list_by_prefix(&self, prefix: &str) -> Result<Vec<ContainerId>>;
    /// Execute a command inside a running container and capture its output.
    fn exec(&self, id: &ContainerId, opts: &ExecOpts) -> Result<ExecOutput>;
    /// Attach interactively to a running container with a PTY.
    /// Inherits the caller's stdin/stdout/stderr.
    fn exec_interactive(
        &self,
        id: &ContainerId,
        command: &[String],
    ) -> Result<std::process::ExitStatus>;
}

/// Create a Docker-based container runtime.
///
/// Checks `UR_CONTAINER` env var for `nerdctl`/`containerd` to use nerdctl;
/// otherwise defaults to `docker`.
pub fn runtime_from_env() -> DockerRuntime {
    match std::env::var("UR_CONTAINER").as_deref() {
        Ok("nerdctl") | Ok("containerd") => DockerRuntime {
            command: "nerdctl".into(),
        },
        _ => DockerRuntime {
            command: "docker".into(),
        },
    }
}
