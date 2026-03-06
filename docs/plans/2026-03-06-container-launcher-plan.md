# Container Launcher Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement `crates/container/` library crate with a `ContainerRuntime` trait, Apple and Docker backends, and a worker container image.

**Architecture:** A trait-based abstraction over container CLIs. `UR_CONTAINER` env var selects backend (apple or docker), defaulting by platform. Both backends use the same OCI Dockerfile from `containers/claude-worker/`. Each backend wraps its respective CLI via `std::process::Command`.

**Tech Stack:** Rust (std::process::Command), cargo-make, Dockerfile (OCI)

**Design doc:** `docs/plans/2026-03-06-container-launcher-design.md`

---

### Task 1: Create `crates/container/` crate with trait and types

**Files:**
- Create: `crates/container/Cargo.toml`
- Create: `crates/container/src/lib.rs`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "container"
edition.workspace = true
version.workspace = true

[dependencies]
anyhow = "1"
```

**Step 2: Create `src/lib.rs` with trait and types**

```rust
mod apple;
mod docker;

use std::path::PathBuf;

use anyhow::Result;

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

pub fn runtime_from_env() -> Box<dyn ContainerRuntime> {
    match std::env::var("UR_CONTAINER").as_deref() {
        Ok("apple") => Box::new(apple::AppleRuntime),
        Ok("docker") => Box::new(docker::DockerRuntime),
        _ if cfg!(target_os = "macos") => Box::new(apple::AppleRuntime),
        _ => Box::new(docker::DockerRuntime),
    }
}
```

**Step 3: Create stub `src/apple.rs` and `src/docker.rs`**

Both files start with the same skeleton so the crate compiles:

```rust
// apple.rs
use anyhow::Result;
use crate::{BuildOpts, ContainerId, ContainerRuntime, ImageId, RunOpts};

pub struct AppleRuntime;

impl ContainerRuntime for AppleRuntime {
    fn build(&self, _opts: &BuildOpts) -> Result<ImageId> {
        todo!()
    }
    fn run(&self, _opts: &RunOpts) -> Result<ContainerId> {
        todo!()
    }
    fn stop(&self, _id: &ContainerId) -> Result<()> {
        todo!()
    }
    fn rm(&self, _id: &ContainerId) -> Result<()> {
        todo!()
    }
}
```

```rust
// docker.rs
use anyhow::Result;
use crate::{BuildOpts, ContainerId, ContainerRuntime, ImageId, RunOpts};

pub struct DockerRuntime;

impl ContainerRuntime for DockerRuntime {
    fn build(&self, _opts: &BuildOpts) -> Result<ImageId> {
        todo!()
    }
    fn run(&self, _opts: &RunOpts) -> Result<ContainerId> {
        todo!()
    }
    fn stop(&self, _id: &ContainerId) -> Result<()> {
        todo!()
    }
    fn rm(&self, _id: &ContainerId) -> Result<()> {
        todo!()
    }
}
```

**Step 4: Verify it compiles**

Run: `cargo check -p container`
Expected: compiles with `todo!()` warnings

**Step 5: Commit**

```
feat(container): add crate with ContainerRuntime trait and types
```

---

### Task 2: Implement DockerRuntime

**Files:**
- Modify: `crates/container/src/docker.rs`
- Test: `crates/container/src/docker.rs` (unit tests in same file)

**Step 1: Write unit tests for command construction**

Add a helper that builds the `Command` without running it, then test it returns the expected args. Add tests at the bottom of `docker.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        assert_eq!(args, vec![
            "build",
            "-t", "ur-worker:latest",
            "-f", "/project/containers/claude-worker/Dockerfile",
            "/project/containers/claude-worker",
        ]);
    }

    #[test]
    fn run_command_args() {
        let args = DockerRuntime::run_args(&sample_run_opts());
        assert_eq!(args, vec![
            "run", "-d",
            "--name", "agent_abc123",
            "--cpus", "4",
            "--memory", "8G",
            "-v", "/host/workspace:/workspace",
            "-v", "/tmp/ur/sockets/agent_abc123.sock:/var/run/ur.sock",
            "-w", "/workspace",
            "ur-worker:latest",
        ]);
    }

    #[test]
    fn run_command_args_with_command() {
        let mut opts = sample_run_opts();
        opts.command = vec!["tmux".into(), "new-session".into(), "-d".into()];
        let args = DockerRuntime::run_args(&opts);
        assert!(args.ends_with(&["tmux", "new-session", "-d"]));
    }

    #[test]
    fn stop_command_args() {
        let args = DockerRuntime::stop_args(&ContainerId("abc123".into()));
        assert_eq!(args, vec!["stop", "abc123"]);
    }

    #[test]
    fn rm_command_args() {
        let args = DockerRuntime::rm_args(&ContainerId("abc123".into()));
        assert_eq!(args, vec!["rm", "abc123"]);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p container`
Expected: FAIL — `build_args` method does not exist

**Step 3: Implement DockerRuntime**

Replace the docker.rs stub with:

```rust
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{BuildOpts, ContainerId, ContainerRuntime, ImageId, RunOpts};

pub struct DockerRuntime;

impl DockerRuntime {
    fn exec(args: &[&str]) -> Result<String> {
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

    pub fn build_args(opts: &BuildOpts) -> Vec<&str> {
        // Note: returns borrowed from opts, but for testability we use a
        // separate owned-string version internally. This returns references
        // for assertion convenience.
        // Actually, we need owned strings. Let's rethink.
        todo!()
    }
}
```

Actually — the testability pattern should use owned strings. Let me restructure. Each method builds a `Vec<String>` of args, and an `exec` helper runs them. Tests assert on the `Vec<String>`.

Replace docker.rs entirely:

```rust
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
            "-t".into(), opts.tag.clone(),
            "-f".into(), opts.dockerfile.display().to_string(),
            opts.context.display().to_string(),
        ]
    }

    pub fn run_args(opts: &RunOpts) -> Vec<String> {
        let mut args = vec![
            "run".into(), "-d".into(),
            "--name".into(), opts.name.clone(),
            "--cpus".into(), opts.cpus.to_string(),
            "--memory".into(), opts.memory.clone(),
        ];
        for (host, guest) in &opts.volumes {
            args.push("-v".into());
            args.push(format!("{}:{}", host.display(), guest.display()));
        }
        // Docker mounts UDS as regular volumes (shared kernel)
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
```

Update tests to use `Vec<String>`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::BuildOpts;

    fn s(v: &str) -> String { v.to_string() }

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
        assert_eq!(args, vec![
            s("build"),
            s("-t"), s("ur-worker:latest"),
            s("-f"), s("/project/containers/claude-worker/Dockerfile"),
            s("/project/containers/claude-worker"),
        ]);
    }

    #[test]
    fn run_command_args() {
        let args = DockerRuntime::run_args(&sample_run_opts());
        assert_eq!(args, vec![
            s("run"), s("-d"),
            s("--name"), s("agent_abc123"),
            s("--cpus"), s("4"),
            s("--memory"), s("8G"),
            s("-v"), s("/host/workspace:/workspace"),
            s("-v"), s("/tmp/ur/sockets/agent_abc123.sock:/var/run/ur.sock"),
            s("-w"), s("/workspace"),
            s("ur-worker:latest"),
        ]);
    }

    #[test]
    fn run_command_args_with_command_override() {
        let mut opts = sample_run_opts();
        opts.command = vec!["tmux".into(), "new-session".into(), "-d".into()];
        let args = DockerRuntime::run_args(&opts);
        let last_three: Vec<&str> = args[args.len()-3..].iter().map(|s| s.as_str()).collect();
        assert_eq!(last_three, vec!["tmux", "new-session", "-d"]);
    }

    #[test]
    fn stop_command_args() {
        assert_eq!(DockerRuntime::stop_args(&ContainerId("abc".into())),
            vec![s("stop"), s("abc")]);
    }

    #[test]
    fn rm_command_args() {
        assert_eq!(DockerRuntime::rm_args(&ContainerId("abc".into())),
            vec![s("rm"), s("abc")]);
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p container`
Expected: all 5 tests pass

**Step 5: Commit**

```
feat(container): implement DockerRuntime with command construction and tests
```

---

### Task 3: Implement AppleRuntime

**Files:**
- Modify: `crates/container/src/apple.rs`

**Step 1: Write unit tests for Apple-specific command construction**

Same pattern as Docker, but verify:
- `build` uses `container` not `docker`
- `run` uses `--publish-socket` for UDS (not `-v`)
- `run` uses `-c` for cpus and `-m` for memory (Apple CLI differs from Docker)
- Paths under `/tmp` are resolved to `/private/tmp`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::BuildOpts;

    fn s(v: &str) -> String { v.to_string() }

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
        assert!(args.contains(&s("/private/tmp/ur/sockets/agent_abc123.sock:/var/run/ur.sock")));
    }

    #[test]
    fn run_resolves_tmp_symlink_on_volumes() {
        let args = AppleRuntime::run_args(&sample_run_opts());
        // Host path /tmp/... should become /private/tmp/...
        let vol_arg = args.iter().find(|a| a.contains("/workspace")).unwrap();
        assert!(vol_arg.starts_with("/private/tmp/ur/workspace:"));
    }

    #[test]
    fn run_uses_apple_resource_flags() {
        let args = AppleRuntime::run_args(&sample_run_opts());
        assert!(args.contains(&s("-c")));
        assert!(args.contains(&s("-m")));
    }

    #[test]
    fn build_uses_container_binary() {
        // This is implicitly tested by the exec helper using "container",
        // but we verify the args structure
        let opts = BuildOpts {
            tag: "ur-worker:latest".into(),
            dockerfile: PathBuf::from("/project/Dockerfile"),
            context: PathBuf::from("/project"),
        };
        let args = AppleRuntime::build_args(&opts);
        assert_eq!(args[0], "build");
        assert!(args.contains(&s("--tag")));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p container`
Expected: FAIL — `run_args` etc. not defined on AppleRuntime

**Step 3: Implement AppleRuntime**

Key differences from Docker:
- Binary: `container` instead of `docker`
- UDS: `--publish-socket host:guest` instead of `-v host:guest`
- Resources: `-c N` and `-m SIZE` (not `--cpus` and `--memory`)
- Tags: `--tag` not `-t` for build
- Path resolution: `/tmp/X` -> `/private/tmp/X` on host paths

```rust
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
            "--tag".into(), opts.tag.clone(),
            "--file".into(), opts.dockerfile.display().to_string(),
            opts.context.display().to_string(),
        ]
    }

    pub fn run_args(opts: &RunOpts) -> Vec<String> {
        let mut args = vec![
            "run".into(), "-d".into(),
            "--name".into(), opts.name.clone(),
            "-c".into(), opts.cpus.to_string(),
            "-m".into(), opts.memory.clone(),
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
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p container`
Expected: all tests pass (both docker and apple)

**Step 5: Commit**

```
feat(container): implement AppleRuntime with symlink resolution and publish-socket
```

---

### Task 4: Create container image definition

**Files:**
- Create: `containers/claude-worker/Dockerfile`
- Create: `containers/claude-worker/entrypoint.sh`

**Step 1: Create entrypoint.sh**

```bash
#!/bin/sh
set -e

# Start tmux session for agent interaction
exec tmux new-session -d -s agent \; wait
```

Note: `tmux ... ; wait` keeps PID 1 alive. Without it, the container exits immediately since tmux forks to background.

Actually, a simpler approach — tmux in foreground mode:

```bash
#!/bin/sh
set -e

# Start tmux in foreground (keeps container alive)
tmux new-session -s agent
```

Without `-d`, tmux stays in the foreground as PID 1.

**Step 2: Create Dockerfile**

```dockerfile
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    tmux \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# agent_tools binary will be copied in at build time once it exists.
# For now, create placeholder so the image shape is correct.
RUN mkdir -p /usr/local/bin

ENTRYPOINT ["/entrypoint.sh"]
```

**Step 3: Verify Dockerfile syntax (no execution yet)**

Run: `docker build --check containers/claude-worker/ 2>&1 || echo "check flag not supported, that's ok"`

This is just a sanity check. The real integration test is Task 6.

**Step 4: Commit**

```
feat(container): add claude-worker Dockerfile and entrypoint
```

---

### Task 5: Update workspace and cargo-make configuration

**Files:**
- Modify: `Makefile.toml`

The workspace `Cargo.toml` uses `members = ["crates/*"]` which auto-discovers the new crate. No change needed there.

**Step 1: Add UR_CONTAINER env to Makefile.toml**

Add at the top of `Makefile.toml`, after `[config]`:

```toml
[env]
UR_CONTAINER = { condition = { platforms = ["mac"] }, value = "apple" }
```

**Step 2: Verify the full workspace builds**

Run: `cargo check --workspace`
Expected: compiles clean

**Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: all tests pass (unit tests for both backends, existing bridge_test)

**Step 4: Run clippy**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::excessive_nesting`
Expected: no warnings

**Step 5: Commit**

```
chore: add UR_CONTAINER env to cargo-make for macOS
```

---

### Task 6: Integration test — Docker backend lifecycle

**Files:**
- Create: `crates/container/tests/docker_lifecycle.rs`

This test actually builds, runs, stops, and removes a container using the Docker backend. It runs in CI (ubuntu-latest has Docker) and locally if Docker is available.

**Step 1: Write the integration test**

```rust
use std::path::PathBuf;

use container::{BuildOpts, ContainerRuntime, DockerRuntime, RunOpts, ImageId};

fn claude_worker_context() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../containers/claude-worker")
        .canonicalize()
        .expect("containers/claude-worker directory must exist")
}

#[test]
fn docker_build_run_stop_rm() {
    if std::env::var("UR_CONTAINER").as_deref() == Ok("apple") {
        eprintln!("skipping docker integration test (UR_CONTAINER=apple)");
        return;
    }

    let rt = DockerRuntime;
    let context = claude_worker_context();

    // Build
    let image = rt.build(&BuildOpts {
        tag: "ur-worker-test:latest".into(),
        dockerfile: context.join("Dockerfile"),
        context: context.clone(),
    }).expect("build should succeed");

    // Run (use sleep instead of tmux for a simpler test container)
    let id = rt.run(&RunOpts {
        image,
        name: "ur-test-lifecycle".into(),
        cpus: 1,
        memory: "512M".into(),
        volumes: vec![],
        socket_mounts: vec![],
        workdir: None,
        command: vec!["sleep".into(), "30".into()],
    }).expect("run should succeed");

    // Stop
    rt.stop(&id).expect("stop should succeed");

    // Remove
    rt.rm(&id).expect("rm should succeed");
}
```

Note: we override the entrypoint with `sleep 30` so the test doesn't need a TTY for tmux. The test verifies the full lifecycle: build -> run -> stop -> rm.

**Step 2: Run the test locally (requires Docker)**

Run: `UR_CONTAINER=docker cargo test -p container --test docker_lifecycle -- --nocapture`
Expected: PASS (builds image, runs container, stops, removes)

**Step 3: Commit**

```
test(container): add Docker lifecycle integration test
```

---

### Task 7: Make DockerRuntime and AppleRuntime public exports

**Files:**
- Modify: `crates/container/src/lib.rs`

Ensure `pub use apple::AppleRuntime;` and `pub use docker::DockerRuntime;` are exported so downstream crates (`urd`, `ur`) can use them directly if needed, and integration tests can import them.

**Step 1: Add pub use to lib.rs**

Add after the mod declarations:

```rust
pub use apple::AppleRuntime;
pub use docker::DockerRuntime;
```

**Step 2: Verify build**

Run: `cargo check --workspace`
Expected: clean

**Step 3: Commit**

```
feat(container): export AppleRuntime and DockerRuntime publicly
```

---

### Task 8: Wire container crate into urd dependency

**Files:**
- Modify: `crates/urd/Cargo.toml`

This doesn't add any runtime logic yet — just makes the crate available for urd to use when it needs to launch containers.

**Step 1: Add dependency**

Add to `[dependencies]` in `crates/urd/Cargo.toml`:

```toml
container = { path = "../container" }
```

**Step 2: Verify build**

Run: `cargo check --workspace`
Expected: clean

**Step 3: Commit**

```
chore(urd): add container crate dependency
```

---

### Task 9: Final CI verification

**Step 1: Run full CI locally**

Run: `cargo make ci`
Expected: fmt, clippy, build, test all pass

**Step 2: Commit any fixes if needed**

**Step 3: Start ticket**

Run: `tk start ur-97i2`
