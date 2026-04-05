# workertools

Worker binary for container-side host command execution and RAG search.
Installed at `/usr/local/bin/workertools` in worker containers.

- `workertools host-exec <command> <args>` — forwards commands to ur-server via gRPC, streams output in real time
- `workertools rag search <query>` — searches indexed documentation via RAG
- `workertools status step-complete` — signals step completion to workerd (WorkerDaemonService)
- `workertools status pause-nudge` — suppresses nudges for 5 min via workerd (WorkerDaemonService)
- `workertools status request-human "<msg>"` — requests human attention via server (CoreService)
- Bash shims at `/home/worker/.local/bin/<command>` call `workertools host-exec`
- Connects to ur-server via `$UR_SERVER_ADDR`
- Auth headers (`ur-worker-id`, `ur-worker-secret`) injected via tonic interceptor for all subcommands
- Exits with remote exit code
- `workertools workflow set-ticket <id>` — sets the ticket ID on workerd for dispatch (design mode only)
- `workertools workflow dispatch` — dispatches the previously set ticket to the server for workflow creation (design mode only)
- Ticket management is handled via `ur ticket` through host-exec (not a direct workertools subcommand)

Hidden aliases for backwards compatibility:
- `workertools step-complete` — alias for `workertools status step-complete`
- `workertools agent request-human` — alias for `workertools status request-human`
