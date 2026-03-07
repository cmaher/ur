# workercmd-git (Git Proxy)

Transparent git proxy binary for worker containers. Installed at `/usr/local/bin/git` to intercept all git commands.

- Connects to `127.0.0.1:$UR_GRPC_PORT` (default port: `42069`) via tonic gRPC over TCP
- Sends all args to urd's `GitService::Exec` streaming RPC
- Streams stdout/stderr in real time and exits with the remote exit code
- Validation errors (blocked flags like `-C`, `--git-dir`) return gRPC status errors before streaming
