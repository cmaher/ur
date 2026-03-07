# workercmd-git (Git Proxy)

Transparent git proxy binary for worker containers. Installed at `/usr/local/bin/git` to intercept all git commands.

- Connects to `$UR_SOCKET` (default: `/var/run/ur/ur.sock`) via tonic gRPC over UDS
- Sends all args to urd's `GitService::Exec` streaming RPC
- Streams stdout/stderr in real time and exits with the remote exit code
- Validation errors (blocked flags like `-C`, `--git-dir`) return gRPC status errors before streaming
