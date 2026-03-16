# workertools

Worker binary for container-side host command execution and RAG search.
Installed at `/usr/local/bin/workertools` in worker containers.

- `workertools host-exec <command> <args>` — forwards commands to ur-server via gRPC, streams output in real time
- `workertools rag search <query>` — searches indexed documentation via RAG
- Bash shims at `/home/worker/.local/bin/<command>` call `workertools host-exec`
- Connects to ur-server via `$UR_SERVER_ADDR`
- Auth headers (`ur-agent-id`, `ur-agent-secret`) injected via tonic interceptor for all subcommands (header names use legacy "agent" naming for wire compatibility)
- Exits with remote exit code
- Ticket management is handled via `ur ticket` through host-exec (not a direct workertools subcommand)
