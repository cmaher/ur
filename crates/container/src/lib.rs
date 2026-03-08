mod apple;
mod docker;

use std::path::PathBuf;

use anyhow::Result;

pub use apple::AppleRuntime;
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

pub trait ContainerRuntime {
    fn build(&self, opts: &BuildOpts) -> Result<ImageId>;
    fn run(&self, opts: &RunOpts) -> Result<ContainerId>;
    fn stop(&self, id: &ContainerId) -> Result<()>;
    fn rm(&self, id: &ContainerId) -> Result<()>;
    /// Execute a command inside a running container and capture its output.
    fn exec(&self, id: &ContainerId, opts: &ExecOpts) -> Result<ExecOutput>;
    /// Attach interactively to a running container with a PTY.
    /// Inherits the caller's stdin/stdout/stderr.
    fn exec_interactive(
        &self,
        id: &ContainerId,
        command: &[String],
    ) -> Result<std::process::ExitStatus>;
    /// Return the host IP address reachable from inside containers.
    /// Containers use this to connect back to services running on the host.
    fn host_gateway_ip(&self) -> Result<String>;
}

fn has_command(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success())
}

pub fn runtime_from_env() -> Box<dyn ContainerRuntime> {
    match std::env::var("UR_CONTAINER").as_deref() {
        Ok("apple") => Box::new(apple::AppleRuntime),
        Ok("docker") => Box::new(docker::DockerRuntime {
            command: "docker".into(),
        }),
        Ok("nerdctl") | Ok("containerd") => Box::new(docker::DockerRuntime {
            command: "nerdctl".into(),
        }),
        _ if has_command("container") => Box::new(apple::AppleRuntime),
        _ if has_command("docker") => Box::new(docker::DockerRuntime {
            command: "docker".into(),
        }),
        _ if has_command("nerdctl") => Box::new(docker::DockerRuntime {
            command: "nerdctl".into(),
        }),
        _ => Box::new(docker::DockerRuntime {
            command: "docker".into(),
        }),
    }
}
