# ur-server (Server)

Long-running server process. Auto-spawned by `ur` on first use (not a launchd service). Listens on TCP via tonic gRPC, serves both host CLI (`ur`) and in-container workers (`workertools` and `workerd` binaries). Depends on the `ur-postgres` container for database access — reads `DATABASE_URL` env var or falls back to `config.db.database_url()` (default: `postgres://ur:ur@ur-postgres:5432/ur`).

- Two gRPC servers: host server on `127.0.0.1:$server_port` (Core + Ticket, no auth) and worker server on `0.0.0.0:$worker_port` (HostExec + Ticket, auth interceptor validates worker-id/secret via WorkerManager)
- `CoreServiceHandler` is `Clone` — keep it stateless or use `Arc` for shared state
- Config and constants are in the `ur_config` crate, re-exported via `ur_server::config`
- `stream::spawn_child_output_stream` is the shared helper for streaming child process output as `CommandOutput` gRPC frames
- **All container operations run on the host via builderd**, not inside the server container. The server container has no Docker socket access. `WorkerManager` delegates worker launch (`LaunchWorker` RPC) and stop (`StopWorker` RPC) to builderd's `BuilderContainerService`. `NetworkManager` uses the `InspectNetwork` RPC. `SquidManager` uses `ExecContainer` instead of a local `docker exec`. Pool git operations also go through builderd (`BuilderDaemonService`): `RepoPoolManager` sends git commands (clone, fetch, reset) via `BuilderdClient`. CWD paths use `%WORKSPACE%` templates which builderd resolves to its local workspace path.
- Squid proxy: `SquidManager` writes config to `$UR_CONFIG/squid/` (`squid.conf` + `allowlist.txt`) and signals reconfigure via `ExecContainer` RPC → builderd. Compose manages the Squid container lifecycle — SquidManager only handles config files and reload signals.
