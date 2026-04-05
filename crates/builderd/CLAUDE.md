# builderd

Builder daemon for executing commands on the host. Receives already-validated
requests from ur-server and spawns processes locally.

- Standalone binary, runs natively on host (not containerized)
- tonic gRPC server; binds to `127.0.0.1` by default, `--bind` flag overrides (on Linux, `ur start` passes the Docker bridge gateway IP)
- Trusts ur-server completely — no command validation
- Started/stopped by `ur start`/`ur stop`, PID tracked at `~/.ur/builderd.pid`
- Resolves `%WORKSPACE%` prefixes in `working_dir` via `--workspace` CLI flag or `BUILDERD_WORKSPACE` env var
