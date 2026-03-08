# workercmd-git (Git Proxy)

Transparent git proxy binary for worker containers. Installed at `/usr/local/bin/git` to intercept all git commands.

- Connects to `$UR_GRPC_HOST:$UR_GRPC_PORT` via tonic gRPC over TCP
- `UR_GRPC_HOST` and `UR_GRPC_PORT` env vars are **required** — the binary panics if they are not set
- All args are forwarded as-is to urd's `GitService::Exec` streaming RPC (including `--help`, `--version`)
- `-C` flags are silently stripped by urd (not blocked); `--git-dir` and `--work-tree` return gRPC errors
- Streams stdout/stderr in real time and exits with the remote exit code
