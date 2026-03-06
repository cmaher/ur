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
    pub socket_mounts: Vec<(PathBuf, PathBuf)>,
    pub workdir: Option<PathBuf>,
    pub command: Vec<String>,
}

pub trait ContainerRuntime {
    fn build(&self, opts: &BuildOpts) -> Result<ImageId>;
    fn run(&self, opts: &RunOpts) -> Result<ContainerId>;
    fn stop(&self, id: &ContainerId) -> Result<()>;
    fn rm(&self, id: &ContainerId) -> Result<()>;
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
        Ok("docker") => Box::new(docker::DockerRuntime),
        _ if has_command("container") => Box::new(apple::AppleRuntime),
        _ => Box::new(docker::DockerRuntime),
    }
}
