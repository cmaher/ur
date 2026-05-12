# builderd

Builder daemon for executing commands and managing containers on the host. Receives
already-validated requests from ur-server and acts on the host filesystem and Docker daemon.

- Standalone binary, runs natively on host (not containerized)
- tonic gRPC server; binds to `127.0.0.1` by default, `--bind` flag overrides (on Linux, `ur start` passes the Docker bridge gateway IP)
- Trusts ur-server completely — no command validation
- Started/stopped by `ur start`/`ur stop`, PID tracked at `~/.ur/builderd.pid`
- Resolves `%WORKSPACE%` prefixes in both `command` and `working_dir` via `--workspace` CLI flag or `BUILDERD_WORKSPACE` env var
- Exposes three gRPC services on the same port:
  - `BuilderDaemonService` (`proto/builder.proto`) — exec arbitrary commands on the host; used for worker hostexec (git, gh via the three-hop pipeline)
  - `BuilderContainerService` (`proto/builder_container.proto`) — worker container lifecycle (launch, stop, exec, network inspect); owns the Docker socket on behalf of the server
  - `BuilderPoolService` (`proto/builder_pool.proto`) — pool slot lifecycle (clone, reset, clean, branch checkout); owns all pool filesystem and git operations on behalf of `RepoPoolManager`
- `--config-dir` flag (default `~/.ur`) sets the root config directory; used by `BuilderPoolHandler` to locate local overlay files at `<config_dir>/projects/<project>/local/`
