use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{BuildOpts, ContainerId, ContainerRuntime, ExecOpts, ExecOutput, ImageId, RunOpts};

/// Docker-compatible container runtime. Works with `docker` and `nerdctl` (containerd).
#[derive(Clone)]
pub struct DockerRuntime {
    pub command: String,
}

impl DockerRuntime {
    fn exec(&self, args: &[String]) -> Result<String> {
        let output = Command::new(&self.command)
            .args(args)
            .output()
            .with_context(|| format!("failed to execute {}", self.command))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{} {} failed: {}", self.command, args[0], stderr.trim());
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
        for pm in &opts.port_maps {
            args.push("-p".into());
            args.push(format!("{}:{}", pm.host_port, pm.container_port));
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
        vec!["stop".into(), "-t".into(), "3".into(), id.0.clone()]
    }

    pub fn rm_args(id: &ContainerId) -> Vec<String> {
        vec!["rm".into(), id.0.clone()]
    }

    pub fn exec_args(id: &ContainerId, opts: &ExecOpts) -> Vec<String> {
        let mut args = vec!["exec".into()];
        if let Some(workdir) = &opts.workdir {
            args.push("-w".into());
            args.push(workdir.display().to_string());
        }
        args.push(id.0.clone());
        args.extend(opts.command.iter().cloned());
        args
    }
}

impl ContainerRuntime for DockerRuntime {
    fn build(&self, opts: &BuildOpts) -> Result<ImageId> {
        let args = Self::build_args(opts);
        self.exec(&args)?;
        Ok(ImageId(opts.tag.clone()))
    }

    fn run(&self, opts: &RunOpts) -> Result<ContainerId> {
        let args = Self::run_args(opts);
        let id = self.exec(&args)?;
        Ok(ContainerId(id))
    }

    fn stop(&self, id: &ContainerId) -> Result<()> {
        let args = Self::stop_args(id);
        self.exec(&args)?;
        Ok(())
    }

    fn rm(&self, id: &ContainerId) -> Result<()> {
        let args = Self::rm_args(id);
        self.exec(&args)?;
        Ok(())
    }

    fn exec(&self, id: &ContainerId, opts: &ExecOpts) -> Result<ExecOutput> {
        let args = Self::exec_args(id, opts);
        let output = Command::new(&self.command)
            .args(&args)
            .output()
            .with_context(|| format!("failed to execute {} exec", self.command))?;
        Ok(ExecOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn exec_interactive(
        &self,
        id: &ContainerId,
        command: &[String],
    ) -> Result<std::process::ExitStatus> {
        let mut args = vec!["exec".to_string(), "-it".to_string(), id.0.clone()];
        args.extend(command.iter().cloned());
        Command::new(&self.command)
            .args(&args)
            .status()
            .with_context(|| format!("failed to execute interactive {} exec", self.command))
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
            port_maps: vec![crate::PortMap {
                host_port: 55000,
                container_port: 42069,
            }],
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
                s("-p"),
                s("55000:42069"),
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
        let last_three: Vec<&str> = args[args.len() - 3..].iter().map(|s| s.as_str()).collect();
        assert_eq!(last_three, vec!["tmux", "new-session", "-d"]);
    }

    #[test]
    fn stop_command_args() {
        assert_eq!(
            DockerRuntime::stop_args(&ContainerId("abc".into())),
            vec![s("stop"), s("-t"), s("3"), s("abc")]
        );
    }

    #[test]
    fn rm_command_args() {
        assert_eq!(
            DockerRuntime::rm_args(&ContainerId("abc".into())),
            vec![s("rm"), s("abc")]
        );
    }

    #[test]
    fn exec_command_args() {
        let opts = ExecOpts {
            command: vec![s("echo"), s("hello")],
            workdir: None,
        };
        assert_eq!(
            DockerRuntime::exec_args(&ContainerId("abc".into()), &opts),
            vec![s("exec"), s("abc"), s("echo"), s("hello")]
        );
    }

    #[test]
    fn exec_command_args_with_workdir() {
        let opts = ExecOpts {
            command: vec![s("ls")],
            workdir: Some(PathBuf::from("/workspace")),
        };
        assert_eq!(
            DockerRuntime::exec_args(&ContainerId("abc".into()), &opts),
            vec![s("exec"), s("-w"), s("/workspace"), s("abc"), s("ls")]
        );
    }
}
