# ur (Host CLI)

Runs on the host macOS system. Connects to `urd` via tonic gRPC over TCP at `127.0.0.1:42069` (default). Use `--port` to override, or set `daemon_port` in `ur.toml`.

- Daemon port resolution: `--port` CLI flag > `ur.toml` > default (42069)
- Container image is built directly via the `container` crate (not via RPC)
- `process launch` builds the image locally, then calls ProcessLaunch RPC
- `process stop` calls ProcessStop RPC
- `process attach` uses the container runtime directly (exec_interactive)
