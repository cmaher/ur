use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{BuildOpts, ContainerId, ContainerRuntime, ExecOpts, ExecOutput, ImageId, RunOpts};

pub struct AppleRuntime;

impl AppleRuntime {
    fn exec(args: &[String]) -> Result<String> {
        let output = Command::new("container")
            .args(args)
            .output()
            .context("failed to execute container")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("container {} failed: {}", args[0], stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Resolve macOS symlinks: /tmp -> /private/tmp
    fn resolve_host_path(path: &Path) -> PathBuf {
        let s = path.display().to_string();
        if s.starts_with("/tmp/") || s == "/tmp" {
            PathBuf::from(format!("/private{s}"))
        } else {
            path.to_path_buf()
        }
    }

    pub fn build_args(opts: &BuildOpts) -> Vec<String> {
        vec![
            "build".into(),
            "--arch".into(),
            std::env::consts::ARCH.into(),
            "--tag".into(),
            opts.tag.clone(),
            "--file".into(),
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
            "-c".into(),
            opts.cpus.to_string(),
            "-m".into(),
            opts.memory.clone(),
        ];
        for (host, guest) in &opts.volumes {
            args.push("--volume".into());
            let resolved = Self::resolve_host_path(host);
            args.push(format!("{}:{}", resolved.display(), guest.display()));
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
            args.push("--workdir".into());
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

    pub fn exec_args(id: &ContainerId, opts: &ExecOpts) -> Vec<String> {
        let mut args = vec!["exec".into()];
        if let Some(workdir) = &opts.workdir {
            args.push("--workdir".into());
            args.push(workdir.display().to_string());
        }
        args.push(id.0.clone());
        args.extend(opts.command.iter().cloned());
        args
    }
}

/// Parse the host IP from the bridge100 interface (Apple container VM bridge).
fn parse_bridge100_ip() -> Result<String> {
    let output = Command::new("ifconfig")
        .arg("bridge100")
        .output()
        .context("failed to run ifconfig bridge100")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("inet ")
            && let Some(ip) = rest.split_whitespace().next()
        {
            return Ok(ip.to_string());
        }
    }
    bail!("could not determine host gateway IP from bridge100 interface")
}

impl ContainerRuntime for AppleRuntime {
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

    fn exec(&self, id: &ContainerId, opts: &ExecOpts) -> Result<ExecOutput> {
        let args = Self::exec_args(id, opts);
        let output = Command::new("container")
            .args(&args)
            .output()
            .context("failed to execute container exec")?;
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
        Command::new("container")
            .args(&args)
            .status()
            .context("failed to execute interactive container exec")
    }

    fn host_gateway_ip(&self) -> Result<String> {
        parse_bridge100_ip()
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

    fn sample_run_opts() -> RunOpts {
        RunOpts {
            image: ImageId("ur-worker:latest".into()),
            name: "agent_abc123".into(),
            cpus: 4,
            memory: "8G".into(),
            volumes: vec![(
                PathBuf::from("/tmp/ur/workspace"),
                PathBuf::from("/workspace"),
            )],
            port_maps: vec![],
            env_vars: vec![(ur_config::URD_ADDR_ENV.into(), "192.168.64.1:55000".into())],
            workdir: Some(PathBuf::from("/workspace")),
            command: vec![],
        }
    }

    #[test]
    fn run_resolves_tmp_symlink_on_volumes() {
        let args = AppleRuntime::run_args(&sample_run_opts());
        let vol_arg = args.iter().find(|a| a.contains("/workspace")).unwrap();
        assert!(vol_arg.starts_with("/private/tmp/ur/workspace:"));
    }

    #[test]
    fn run_uses_env_flag_for_vars() {
        let args = AppleRuntime::run_args(&sample_run_opts());
        assert!(args.contains(&s("-e")));
        assert!(args.contains(&format!("{}=192.168.64.1:55000", ur_config::URD_ADDR_ENV)));
    }

    #[test]
    fn run_uses_apple_resource_flags() {
        let args = AppleRuntime::run_args(&sample_run_opts());
        assert!(args.contains(&s("-c")));
        assert!(args.contains(&s("-m")));
    }

    #[test]
    fn build_uses_tag_flag() {
        let opts = BuildOpts {
            tag: "ur-worker:latest".into(),
            dockerfile: PathBuf::from("/project/Dockerfile"),
            context: PathBuf::from("/project"),
        };
        let args = AppleRuntime::build_args(&opts);
        assert_eq!(args[0], "build");
        assert!(args.contains(&s("--arch")));
        assert!(args.contains(&s("--tag")));
    }

    #[test]
    fn exec_command_args() {
        let opts = ExecOpts {
            command: vec![s("echo"), s("hello")],
            workdir: None,
        };
        assert_eq!(
            AppleRuntime::exec_args(&ContainerId("abc".into()), &opts),
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
            AppleRuntime::exec_args(&ContainerId("abc".into()), &opts),
            vec![
                s("exec"),
                s("--workdir"),
                s("/workspace"),
                s("abc"),
                s("ls")
            ]
        );
    }
}
