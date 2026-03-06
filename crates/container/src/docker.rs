use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{BuildOpts, ContainerId, ContainerRuntime, ImageId, RunOpts};

pub struct DockerRuntime;

impl DockerRuntime {
    fn exec(args: &[String]) -> Result<String> {
        let output = Command::new("docker")
            .args(args)
            .output()
            .context("failed to execute docker")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("docker {} failed: {}", args[0], stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn build_args(opts: &BuildOpts) -> Vec<String> {
        vec![
            "build".into(),
            "-t".into(),
            opts.tag.clone(),
            "-f".into(),
            opts.dockerfile.display().to_string(),
            opts.context.display().to_string(),
        ]
    }

    pub fn run_args(opts: &RunOpts) -> Vec<String> {
        let mut args = vec![
            "run".into(),
            "-d".into(),
            "--name".into(),
            opts.name.clone(),
            "--cpus".into(),
            opts.cpus.to_string(),
            "--memory".into(),
            opts.memory.clone(),
        ];
        for (host, guest) in &opts.volumes {
            args.push("-v".into());
            args.push(format!("{}:{}", host.display(), guest.display()));
        }
        for (host, guest) in &opts.socket_mounts {
            args.push("-v".into());
            args.push(format!("{}:{}", host.display(), guest.display()));
        }
        if let Some(workdir) = &opts.workdir {
            args.push("-w".into());
            args.push(workdir.display().to_string());
        }
        args.push(opts.image.0.clone());
        args.extend(opts.command.iter().cloned());
        args
    }

    pub fn stop_args(id: &ContainerId) -> Vec<String> {
        vec!["stop".into(), id.0.clone()]
    }

    pub fn rm_args(id: &ContainerId) -> Vec<String> {
        vec!["rm".into(), id.0.clone()]
    }
}

impl ContainerRuntime for DockerRuntime {
    fn build(&self, opts: &BuildOpts) -> Result<ImageId> {
        let args = Self::build_args(opts);
        Self::exec(&args)?;
        Ok(ImageId(opts.tag.clone()))
    }

    fn run(&self, opts: &RunOpts) -> Result<ContainerId> {
        let args = Self::run_args(opts);
        let id = Self::exec(&args)?;
        Ok(ContainerId(id))
    }

    fn stop(&self, id: &ContainerId) -> Result<()> {
        let args = Self::stop_args(id);
        Self::exec(&args)?;
        Ok(())
    }

    fn rm(&self, id: &ContainerId) -> Result<()> {
        let args = Self::rm_args(id);
        Self::exec(&args)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::BuildOpts;

    fn s(v: &str) -> String {
        v.to_string()
    }

    fn sample_build_opts() -> BuildOpts {
        BuildOpts {
            tag: "ur-worker:latest".into(),
            dockerfile: PathBuf::from("/project/containers/claude-worker/Dockerfile"),
            context: PathBuf::from("/project/containers/claude-worker"),
        }
    }

    fn sample_run_opts() -> RunOpts {
        RunOpts {
            image: ImageId("ur-worker:latest".into()),
            name: "agent_abc123".into(),
            cpus: 4,
            memory: "8G".into(),
            volumes: vec![(
                PathBuf::from("/host/workspace"),
                PathBuf::from("/workspace"),
            )],
            socket_mounts: vec![(
                PathBuf::from("/tmp/ur/sockets/agent_abc123.sock"),
                PathBuf::from("/var/run/ur.sock"),
            )],
            workdir: Some(PathBuf::from("/workspace")),
            command: vec![],
        }
    }

    #[test]
    fn build_command_args() {
        let args = DockerRuntime::build_args(&sample_build_opts());
        assert_eq!(
            args,
            vec![
                s("build"),
                s("-t"),
                s("ur-worker:latest"),
                s("-f"),
                s("/project/containers/claude-worker/Dockerfile"),
                s("/project/containers/claude-worker"),
            ]
        );
    }

    #[test]
    fn run_command_args() {
        let args = DockerRuntime::run_args(&sample_run_opts());
        assert_eq!(
            args,
            vec![
                s("run"),
                s("-d"),
                s("--name"),
                s("agent_abc123"),
                s("--cpus"),
                s("4"),
                s("--memory"),
                s("8G"),
                s("-v"),
                s("/host/workspace:/workspace"),
                s("-v"),
                s("/tmp/ur/sockets/agent_abc123.sock:/var/run/ur.sock"),
                s("-w"),
                s("/workspace"),
                s("ur-worker:latest"),
            ]
        );
    }

    #[test]
    fn run_command_args_with_command_override() {
        let mut opts = sample_run_opts();
        opts.command = vec!["tmux".into(), "new-session".into(), "-d".into()];
        let args = DockerRuntime::run_args(&opts);
        let last_three: Vec<&str> =
            args[args.len() - 3..].iter().map(|s| s.as_str()).collect();
        assert_eq!(last_three, vec!["tmux", "new-session", "-d"]);
    }

    #[test]
    fn stop_command_args() {
        assert_eq!(
            DockerRuntime::stop_args(&ContainerId("abc".into())),
            vec![s("stop"), s("abc")]
        );
    }

    #[test]
    fn rm_command_args() {
        assert_eq!(
            DockerRuntime::rm_args(&ContainerId("abc".into())),
            vec![s("rm"), s("abc")]
        );
    }
}
