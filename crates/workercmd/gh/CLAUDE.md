# workercmd-gh (GitHub CLI Proxy)

Transparent gh proxy binary for worker containers. Installed at `/usr/local/bin/gh` to intercept all gh commands.

- Connects to `$UR_SERVER_ADDR` (host:port) via tonic gRPC over TCP
- `UR_SERVER_ADDR` env var is **required** — the binary panics if it is not set
- `--help` is handled locally; all other args are forwarded to the server's `GhService::Exec` streaming RPC
- No argument validation — all args are forwarded as-is
- Authentication uses the host's existing `gh` login
- Streams stdout/stderr in real time and exits with the remote exit code
