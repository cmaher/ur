# container (Runtime Abstraction)

Pure library crate — no async, no network. Builds CLI arg vectors and shells out to `docker` or `nerdctl` (containerd).

- `DockerRuntime` is parameterized by command name — works with both `docker` and `nerdctl` (Docker-compatible containerd CLI)
- Supports port mapping (`-p host:container`) and env vars (`-e KEY=VALUE`) via `RunOpts`
- `UR_CONTAINER` env var selects nerdctl/containerd; defaults to docker
- Integration test (`tests/lifecycle.rs`) requires a real container runtime — it builds/runs/stops/removes a container
