# builderd

Builder daemon for executing commands and managing containers on the host. Receives
already-validated requests from ur-server and acts on the host filesystem and Docker daemon.

- Standalone binary, runs natively on host (not containerized)
- tonic gRPC server; binds to `127.0.0.1` by default, `--bind` flag overrides (on Linux, `ur start` passes the Docker bridge gateway IP)
- Trusts ur-server completely — no command validation
- Started/stopped by `ur start`/`ur stop`, PID tracked at `~/.ur/builderd.pid`
- Resolves `%WORKSPACE%` prefixes in both `command` and `working_dir` via `--workspace` CLI flag or `BUILDERD_WORKSPACE` env var
- Exposes two gRPC services:
  - `BuilderDaemonService` (`proto/builder.proto`) — exec arbitrary commands on the host; used for pool git ops and worker hostexec (git, gh)
  - `BuilderContainerService` (`proto/builder_container.proto`) — worker container lifecycle (launch, stop, exec, network inspect); owns the Docker socket on behalf of the server
