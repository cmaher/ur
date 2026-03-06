# Container Launcher Design

**Ticket:** ur-97i2
**Date:** 2026-03-06

## Overview

A new `crates/container/` library crate providing a `ContainerRuntime` trait with two backends:

- **AppleRuntime** — wraps the macOS `container` CLI (local dev on Apple Silicon)
- **DockerRuntime** — wraps the `docker` CLI (CI on ubuntu-latest, also usable locally)

Both backends build from the same OCI Dockerfile in `containers/claude-worker/`.

## Backend Selection

The `UR_CONTAINER` env var selects the backend:

- `UR_CONTAINER=apple` — Apple `container` CLI
- `UR_CONTAINER=docker` — Docker CLI
- Unset — defaults to `apple` on macOS, `docker` on Linux

`cargo-make` sets `UR_CONTAINER=apple` on macOS so local `cargo make test` always exercises the apple backend.

```rust
pub fn runtime_from_env() -> Box<dyn ContainerRuntime> {
    match std::env::var("UR_CONTAINER").as_deref() {
        Ok("apple") => Box::new(AppleRuntime::new()),
        Ok("docker") => Box::new(DockerRuntime::new()),
        _ if cfg!(target_os = "macos") => Box::new(AppleRuntime::new()),
        _ => Box::new(DockerRuntime::new()),
    }
}
```

## Trait Surface

```rust
pub trait ContainerRuntime {
    fn build(&self, opts: &BuildOpts) -> Result<ImageId>;
    fn run(&self, opts: &RunOpts) -> Result<ContainerId>;
    fn stop(&self, id: &ContainerId) -> Result<()>;
    fn rm(&self, id: &ContainerId) -> Result<()>;
}
```

### RunOpts

Key fields:

| Field | Type | Purpose |
|---|---|---|
| `image` | `ImageId` | Image to run |
| `name` | `String` | Container name (e.g., `agent_<id>`) |
| `cpus` | `u32` | CPU allocation |
| `memory` | `String` | Memory limit (e.g., `"8G"`) |
| `volumes` | `Vec<(PathBuf, PathBuf)>` | Host:guest directory mounts |
| `socket_mounts` | `Vec<(PathBuf, PathBuf)>` | Host:guest UDS mounts |
| `workdir` | `Option<PathBuf>` | Working directory inside container |
| `command` | `Vec<String>` | Override entrypoint command |

### Backend Differences

| Concern | Apple `container` | Docker |
|---|---|---|
| UDS mount | `--publish-socket host:guest` | `--volume host:guest` |
| Path quirk | Resolve symlinks (`/tmp` -> `/private/tmp`) | No special handling |
| Build | `container build -t X -f F .` | `docker build -t X -f F .` |
| Run | `container run -d ...` | `docker run -d ...` |
| Stop | `container stop <id>` | `docker stop <id>` |
| Remove | `container rm <id>` | `docker rm <id>` |

## Container Image

`containers/claude-worker/`:

- **Dockerfile** — Ubuntu base, install tmux, copy `agent_tools` binary
- **entrypoint.sh** — Starts tmux session: `tmux new-session -d -s agent`

Single Dockerfile used by both backends (standard OCI image).

## File Layout

```
crates/container/
  Cargo.toml
  src/
    lib.rs        -- trait, types, runtime_from_env()
    apple.rs      -- AppleRuntime (wraps `container` CLI)
    docker.rs     -- DockerRuntime (wraps `docker` CLI)

containers/claude-worker/
  Dockerfile
  entrypoint.sh
```

## Testing Strategy

- **Unit tests**: Mock `ContainerRuntime` trait to verify orchestration logic builds correct opts
- **Integration tests**: Actually build + run + stop + rm against real backend
  - CI (ubuntu-latest): `UR_CONTAINER=docker` (default on Linux)
  - Local (macOS): `UR_CONTAINER=apple` (set by cargo-make)
- Docker integration tests run in CI; Apple integration tests run locally only

## cargo-make Configuration

```toml
[env]
UR_CONTAINER = { condition = { platforms = ["mac"] }, value = "apple" }
```
