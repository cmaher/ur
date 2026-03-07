# container (Runtime Abstraction)

Pure library crate — no async, no network. Builds CLI arg vectors and shells out to `container` (Apple), `docker`, or `nerdctl` (containerd).

- `AppleRuntime` resolves `/tmp` → `/private/tmp` on host paths
- `AppleRuntime` passes `--arch` matching the host's `std::env::consts::ARCH` for native builds
- `DockerRuntime` is parameterized by command name — works with both `docker` and `nerdctl` (Docker-compatible containerd CLI)
- Both runtimes support port mapping (`-p host:container`) and env vars (`-e KEY=VALUE`) via `RunOpts`
- `UR_CONTAINER` env var selects backend: `apple`, `docker`, `nerdctl`/`containerd`
- Integration test (`tests/lifecycle.rs`) requires a real container runtime — it builds/runs/stops/removes a container
