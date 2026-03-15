# workertools

Worker binary for container-side host command execution, RAG search, and ticket management.
Installed at `/usr/local/bin/workertools` in worker containers.

- `workertools host-exec <command> <args>` — forwards commands to ur-server via gRPC, streams output in real time
- `workertools rag search <query>` — searches indexed documentation via RAG
- `workertools ticket <subcommand>` — ticket management via TicketService gRPC (uses `ticket_client` crate)
- Bash shims at `/home/worker/.local/bin/<command>` call `workertools host-exec`
- Connects to ur-server via `$UR_SERVER_ADDR`
- Auth headers (`ur-agent-id`, `ur-agent-secret`) injected via tonic interceptor for all subcommands
- Exits with remote exit code
