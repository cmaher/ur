# workercmd-git (Git Proxy)

Transparent git proxy binary for worker containers. Installed at `/usr/local/bin/git` to intercept all git commands.

- Connects to `$URD_ADDR` (host:port) via tonic gRPC over TCP
- `URD_ADDR` env var is **required** — the binary panics if it is not set
- `--help` is handled locally and shows blocked flags; all other args are forwarded to urd's `GitService::Exec` streaming RPC
- `-C`, `--git-dir`, and `--work-tree` are blocked by urd with errors
- Streams stdout/stderr in real time and exits with the remote exit code
