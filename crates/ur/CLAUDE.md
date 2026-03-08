# ur (Host CLI)

Runs on the host macOS system. Connects to `urd` via tonic gRPC over TCP at `127.0.0.1:42068` (default). Use `--port` or `$UR_DAEMON_PORT` to override.

- Container image is built directly via the `container` crate (not via RPC)
- `process launch` builds the image locally, then calls ProcessLaunch RPC
- `process stop` calls ProcessStop RPC
- `process attach` uses the container runtime directly (exec_interactive)
