# ur (Host CLI)

Runs on the host macOS system. Connects to `ur-server` via tonic gRPC over TCP at `127.0.0.1:42069` (default). Use `--port` to override, or set `daemon_port` in `ur.toml`.

- Daemon port resolution: `--port` CLI flag > `ur.toml` > default (42069)
- Auto-starts `ur-server` via Docker Compose if not running — uses `compose_file` from `ur.toml` (default: `~/.ur/docker-compose.yml`)
- `ComposeManager` in `src/compose.rs` wraps `docker compose` CLI for up/down/status
- `stop` kills all agent containers then runs `docker compose down`
- Container images are built separately via `scripts/build/container-image.sh`, not by `ur` itself
- `process launch` assumes `ur-worker:latest` image exists, then calls ProcessLaunch RPC; `-f` kills existing container first
- `process stop` calls ProcessStop RPC
- `process kill` force-removes a container directly (stop + rm), no server needed
- `process attach` uses the container runtime directly (exec_interactive)
