# workercmd-tools (ur-tools)

Worker binary for container-side host command execution and RAG search.
Installed at `/usr/local/bin/ur-tools` in worker containers.

- `ur-tools host-exec <command> <args>` — forwards commands to ur-server via gRPC, streams output in real time
- `ur-tools rag search <query>` — searches indexed documentation via RAG
- Bash shims at `/home/worker/.local/bin/<command>` call `ur-tools host-exec`
- Connects to ur-server via `$UR_SERVER_ADDR`
- Exits with remote exit code
