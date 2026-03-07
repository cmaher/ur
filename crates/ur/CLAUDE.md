# ur (Host CLI)

Runs on the host macOS system. Connects to `urd` via tonic gRPC over UDS at `$UR_CONFIG/ur-grpc.sock` (default `~/.ur/ur-grpc.sock`). Use `--socket` to override.

- Container image is built directly via the `container` crate (not via RPC)
- `process launch` builds the image locally, then calls ProcessLaunch RPC
- `process stop` calls ProcessStop RPC
- `process attach` uses the container runtime directly (exec_interactive)
