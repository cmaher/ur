# ur (Host CLI)

Runs on the host macOS system. Connects to `ur-server` via tonic gRPC over TCP at `127.0.0.1:42069` (default). Set `server_port` in `ur.toml` to override.

- Server port: read from `config.server_port` (defaults to 42069)
- Auto-starts `ur-server` via Docker Compose if not running — uses `compose_file` from `ur.toml` (default: `~/.ur/docker-compose.yml`)
- `ComposeManager` in `src/compose.rs` wraps `docker compose` CLI for up/down/status. Compose YAML is generated programmatically at `ur start` time (no static template) via `generate_compose()`.
- Container images are built separately via `scripts/build/image.sh`, not by `ur` itself
- **All worker interactions go through the server via gRPC.** The only direct container access is server lifecycle (`ur start`/`ur stop`). Direct Docker manipulation from the CLI is not allowed — it desynchronizes the server's in-memory process table.
- `worker launch` assumes `ur-worker-rust:latest` image exists, then calls WorkerLaunch RPC; `-f` stops existing worker first via WorkerStop RPC
- `worker stop` / `worker kill` both call WorkerStop RPC
- `worker attach` uses the container runtime directly (exec_interactive) — temporary until a proper attach RPC exists
