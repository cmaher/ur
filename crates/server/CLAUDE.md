# ur-server (Server)

Long-running server process. Auto-spawned by `ur` on first use (not a launchd service). Listens on TCP via tonic gRPC, serves both host CLI (`ur`) and in-container workers (`workercmd` binaries).

- Docker runtime is created via `container::runtime_from_env()` (supports docker and nerdctl)
- Each RPC handler instantiates its own runtime via `runtime_from_env()` — there is no shared runtime state yet
- Main gRPC server runs on `127.0.0.1:$daemon_port` (TCP, default 42069); per-agent gRPC servers bind to `0.0.0.0` on an OS-assigned port, reachable from containers via Docker internal DNS (`network.server_hostname`)
- `CoreServiceHandler` is `Clone` — keep it stateless or use `Arc` for shared state
- Config and constants are in the `ur_config` crate, re-exported via `ur_server::config`
- `stream::spawn_child_output_stream` is the shared helper for streaming child process output as `CommandOutput` gRPC frames
- `registry::RepoRegistry` maps process_id to repo directory paths (used by HostExecServiceHandler for CWD mapping)
- **Pool git operations run on the host via ur-hostd**, not inside the server container. The server container has no SSH keys or git credentials. `RepoPoolManager` sends git commands (clone, fetch, reset) to hostd over gRPC. It tracks two path namespaces: local (container-side, for `read_dir`/`create_dir_all`) and host-side (for hostd CWD and Docker volume mounts).
- Squid proxy: `SquidManager` writes config to `$UR_CONFIG/squid/` (`squid.conf` + `allowlist.txt`) and signals reconfigure via `docker exec ur-squid squid -k reconfigure`. Compose manages the Squid container lifecycle — SquidManager only handles config files and reload signals.
