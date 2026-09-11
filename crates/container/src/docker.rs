use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{
    BuildOpts, ContainerId, ContainerRuntime, ContainerState, ExecOpts, ExecOutput, ImageId,
    RunOpts,
};

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
        for (key, val) in &opts.env_vars {
            args.push("-e".into());
            args.push(format!("{key}={val}"));
        }
        if let Some(workdir) = &opts.workdir {
            args.push("-w".into());
            args.push(workdir.display().to_string());
        }
        if let Some(network) = &opts.network {
            args.push("--network".into());
            args.push(network.clone());
        }
        for (host, ip) in &opts.add_hosts {
            args.push("--add-host".into());
            args.push(format!("{host}:{ip}"));
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

    pub fn logs_args(id: &ContainerId) -> Vec<String> {
        vec!["logs".into(), "--tail".into(), "100".into(), id.0.clone()]
    }

    pub fn health_status_args(id: &ContainerId) -> Vec<String> {
        vec![
            "inspect".into(),
            "--format".into(),
            "{{if .State.Health}}{{.State.Health.Status}}{{end}}".into(),
            id.0.clone(),
        ]
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

    fn list_by_prefix(&self, prefix: &str) -> Result<Vec<ContainerId>> {
        let output = Command::new(&self.command)
            .args([
                "ps",
                "-a",
                "--filter",
                &format!("name={prefix}"),
                "--format",
                "{{.Names}}",
            ])
            .output()
            .with_context(|| format!("failed to execute {} ps", self.command))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| ContainerId(l.trim().to_string()))
            .collect())
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

    fn health_status(&self, id: &ContainerId) -> Result<String> {
        self.exec(&Self::health_status_args(id))
            .with_context(|| format!("failed to inspect health for container {}", id.0))
    }

    fn logs(&self, id: &ContainerId) -> Result<String> {
        let args = Self::logs_args(id);
        let output = Command::new(&self.command)
            .args(&args)
            .output()
            .with_context(|| format!("failed to read logs for container {}", id.0))?;
        if !output.status.success() {
            bail!(
                "{} logs failed for container {}: {}",
                self.command,
                id.0,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }

    fn inspect_state(&self, id: &ContainerId) -> Result<Option<ContainerState>> {
        let output = Command::new(&self.command)
            .args(["inspect", "--format", "{{.State.Running}} {{.Id}}", &id.0])
            .output()
            .with_context(|| format!("failed to inspect container {}", id.0))?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            return parse_inspect_state(&stdout).with_context(|| {
                format!(
                    "unexpected {} inspect output for container {}: {}",
                    self.command,
                    id.0,
                    stdout.trim()
                )
            });
        }
        // "No such container" is a definitive answer; anything else (daemon
        // unreachable, permission denied) leaves the state unknown.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such container") || stderr.contains("no such object") {
            return Ok(None);
        }
        bail!(
            "{} inspect failed for container {}: {}",
            self.command,
            id.0,
            stderr.trim()
        )
    }
}

/// Parse `docker inspect --format "{{.State.Running}} {{.Id}}"` output into a
/// `ContainerState`. Returns `Err` when the line is not the expected shape,
/// so a malformed answer never reads as a valid one.
fn parse_inspect_state(stdout: &str) -> Result<Option<ContainerState>> {
    let line = stdout.trim();
    let Some((running, id)) = line.split_once(char::is_whitespace) else {
        bail!("expected \"<running> <id>\", got {line:?}");
    };
    let running = match running.trim() {
        "true" => true,
        "false" => false,
        other => bail!("expected running to be true/false, got {other:?}"),
    };
    let id = id.trim();
    if id.is_empty() {
        bail!("empty container id in inspect output");
    }
    Ok(Some(ContainerState {
        id: ContainerId(id.to_owned()),
        running,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::BuildOpts;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn parses_running_container_state() {
        let state = parse_inspect_state("true abc123\n").unwrap().unwrap();
        assert!(state.running);
        assert_eq!(state.id.0, "abc123");
    }

    #[test]
    fn parses_stopped_container_state() {
        let state = parse_inspect_state("false abc123").unwrap().unwrap();
        assert!(!state.running);
        assert_eq!(state.id.0, "abc123");
    }

    #[test]
    fn rejects_unparseable_inspect_output() {
        // A malformed answer must not read as a valid one — reconciliation
        // treats "not running" as grounds to release a worker's slot.
        assert!(parse_inspect_state("").is_err());
        assert!(parse_inspect_state("true").is_err());
        assert!(parse_inspect_state("maybe abc123").is_err());
        assert!(parse_inspect_state("true  ").is_err());
    }

    #[test]
    fn logs_command_args() {
        assert_eq!(
            DockerRuntime::logs_args(&ContainerId("abc123".into())),
            vec![s("logs"), s("--tail"), s("100"), s("abc123")]
        );
    }

    #[test]
    fn health_status_command_handles_images_without_a_healthcheck() {
        assert_eq!(
            DockerRuntime::health_status_args(&ContainerId("abc123".into())),
            vec![
                s("inspect"),
                s("--format"),
                s("{{if .State.Health}}{{.State.Health.Status}}{{end}}"),
                s("abc123"),
            ]
        );
    }

    fn sample_build_opts() -> BuildOpts {
        BuildOpts {
            tag: "ur-worker-claude:latest".into(),
            dockerfile: PathBuf::from("/project/containers/worker-claude/Dockerfile"),
            context: PathBuf::from("/project/containers/worker-claude"),
        }
    }

    fn sample_run_opts() -> RunOpts {
        RunOpts {
            image: ImageId("ur-worker-claude:latest".into()),
            name: "agent_abc123".into(),
            cpus: 4,
            memory: "8G".into(),
            volumes: vec![(
                PathBuf::from("/host/workspace"),
                PathBuf::from("/workspace"),
            )],
            port_maps: vec![],
            env_vars: vec![(
                ur_config::UR_SERVER_ADDR_ENV.into(),
                "ur-server:55000".into(),
            )],
            workdir: Some(PathBuf::from("/workspace")),
            command: vec![],
            network: None,
            add_hosts: vec![],
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
                s("ur-worker-claude:latest"),
                s("-f"),
                s("/project/containers/worker-claude/Dockerfile"),
                s("/project/containers/worker-claude"),
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
                s("-e"),
                format!("{}=ur-server:55000", ur_config::UR_SERVER_ADDR_ENV),
                s("-w"),
                s("/workspace"),
                s("ur-worker-claude:latest"),
            ]
        );
    }

    #[test]
    fn run_command_args_with_network() {
        let mut opts = sample_run_opts();
        opts.network = Some("ur".into());
        let args = DockerRuntime::run_args(&opts);
        // --network ur should appear before the image name
        let net_idx = args.iter().position(|a| a == "--network").unwrap();
        assert_eq!(args[net_idx + 1], "ur");
        // Image name comes after --network pair
        let image_idx = args
            .iter()
            .position(|a| a == "ur-worker-claude:latest")
            .unwrap();
        assert!(net_idx + 1 < image_idx);
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
