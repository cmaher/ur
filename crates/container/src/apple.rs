use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{BuildOpts, ContainerId, ContainerRuntime, ImageId, RunOpts};

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
        for (host, guest) in &opts.socket_mounts {
            args.push("--publish-socket".into());
            let resolved = Self::resolve_host_path(host);
            args.push(format!("{}:{}", resolved.display(), guest.display()));
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
            socket_mounts: vec![(
                PathBuf::from("/tmp/ur/sockets/agent_abc123.sock"),
                PathBuf::from("/var/run/ur.sock"),
            )],
            workdir: Some(PathBuf::from("/workspace")),
            command: vec![],
        }
    }

    #[test]
    fn run_uses_publish_socket_for_uds() {
        let args = AppleRuntime::run_args(&sample_run_opts());
        assert!(args.contains(&s("--publish-socket")));
        assert!(args.contains(&s(
            "/private/tmp/ur/sockets/agent_abc123.sock:/var/run/ur.sock"
        )));
    }

    #[test]
    fn run_resolves_tmp_symlink_on_volumes() {
        let args = AppleRuntime::run_args(&sample_run_opts());
        let vol_arg = args
            .iter()
            .find(|a| a.contains("/workspace"))
            .unwrap();
        assert!(vol_arg.starts_with("/private/tmp/ur/workspace:"));
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
}
