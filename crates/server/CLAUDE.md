# ur-server (Server)

Long-running server process. Auto-spawned by `ur` on first use (not a launchd service). Listens on TCP via tonic gRPC, serves both host CLI (`ur`) and in-container workers (`workertools` and `workerd` binaries).

- Docker runtime is created via `container::runtime_from_env()` (supports docker and nerdctl)
- Each RPC handler instantiates its own runtime via `runtime_from_env()` — there is no shared runtime state yet
- Two gRPC servers: host server on `127.0.0.1:$server_port` (Core + Ticket, no auth) and worker server on `0.0.0.0:$worker_port` (HostExec + Ticket, auth interceptor validates worker-id/secret via WorkerManager)
- `CoreServiceHandler` is `Clone` — keep it stateless or use `Arc` for shared state
- Config and constants are in the `ur_config` crate, re-exported via `ur_server::config`
- `stream::spawn_child_output_stream` is the shared helper for streaming child process output as `CommandOutput` gRPC frames
- **Pool git operations run on the host via builderd**, not inside the server container. The server container has no SSH keys or git credentials. `RepoPoolManager` sends git commands (clone, fetch, reset) to builderd over gRPC. CWD paths sent to builderd use `%WORKSPACE%` templates (e.g., `%WORKSPACE%/pool/ur/0`) which builderd resolves to its local workspace path. Host-side paths are still used for Docker volume mounts and in-use tracking. Local (container-side) paths are used for filesystem operations (`read_dir`/`create_dir_all`).
- Squid proxy: `SquidManager` writes config to `$UR_CONFIG/squid/` (`squid.conf` + `allowlist.txt`) and signals reconfigure via `docker exec ur-squid squid -k reconfigure`. Compose manages the Squid container lifecycle — SquidManager only handles config files and reload signals.
