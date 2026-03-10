# workercmd-tools (ur-tools)

Unified worker binary for container-side commands. Installed at `/usr/local/bin/ur-tools`
in worker containers. Bash shims at `/home/worker/.local/bin/<command>` call
`ur-tools host-exec <command> <args>`.

- Connects to ur-server via `$UR_SERVER_ADDR`
- Streams `CommandOutput` to stdout/stderr in real time
- Exits with remote exit code
