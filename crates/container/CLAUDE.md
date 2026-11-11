# container (Runtime Abstraction)

Pure library crate — no async, no network. Builds CLI arg vectors and shells out to `docker` or `nerdctl` (containerd).

- `DockerRuntime` is parameterized by command name — works with both `docker` and `nerdctl` (Docker-compatible containerd CLI)
- Supports port mapping (`-p host:container`), env vars (`-e KEY=VALUE`) and labels (`--label key=value`) via `RunOpts`
- `DockerRuntime::list_by_labels` (`ps -a` by label filters, reporting one label's value and running state) and `env_of` (a container's env) are inherent methods used by builderd to identify worker containers by label (ur-ezhdb)
- `ContainerRuntime` exposes run-state, optional health-check status, and bounded container logs so builderd can diagnose launch failures without shelling out around the abstraction
- `UR_CONTAINER` env var selects nerdctl/containerd; defaults to docker
- Integration test (`tests/lifecycle.rs`) requires a real container runtime — it builds/runs/stops/removes a container
