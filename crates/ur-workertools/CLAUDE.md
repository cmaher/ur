# ur-workertools

Worker toolkit binary used by agent hooks, skills, and hostexec shims.
Installed at `/usr/local/bin/ur-workertools` in worker containers and on the host next to `ur`
(`cargo make install` → `$UR_INSTALL_DIR`, default `~/.local/bin`), so the same binary serves
container and host-local workers.

- `ur-workertools host-exec <command> <args>` — forwards commands to ur-server via gRPC, streams output in real time
- `ur-workertools rag search <query>` — searches indexed documentation via RAG
- `ur-workertools status step-complete` — signals step completion to workerd (WorkerDaemonService)
- `ur-workertools status pause-nudge` — suppresses nudges for 5 min via workerd (WorkerDaemonService)
- `ur-workertools status request-human "<msg>"` — requests human attention via server (CoreService)
- `ur-workertools notify-idle` — notifies workerd that the agent is idle; `--json` prints `{}`
  on stdout after a successful RPC for AGY's synchronous `Stop` hook contract
- Bash shims at `/home/worker/.local/bin/<command>` call `ur-workertools host-exec`
- Connects to ur-server via `$UR_SERVER_ADDR`
- Connects to workerd via `$UR_WORKERD_ADDR` (`ur_config::UR_WORKERD_ADDR_ENV`) for every
  WorkerDaemonService call (`notify-idle`, `status step-complete|pause-nudge`, `workflow`). Accepts a
  URL or bare `host:port`; unset/blank → `http://127.0.0.1:9120` (the in-container daemon), so
  containers need no extra env. All workerd dialing goes through `src/workerd_client.rs`.
- Auth headers (`ur-worker-id`, `ur-worker-secret`) are added to every request via `inject_auth`
  (workerd requests use `workerd_client::workerd_request`), so a shared host daemon can route on
  `ur-worker-id`
- Exits with remote exit code
- `ur-workertools workflow set-ticket <id>` — sets the ticket ID on workerd for dispatch
- `ur-workertools workflow dispatch` — dispatches the previously set ticket to the server for workflow creation
- Ticket management is handled via `ur ticket` through host-exec (not a direct ur-workertools subcommand)

Hidden aliases for backwards compatibility:
- `ur-workertools step-complete` — alias for `ur-workertools status step-complete`
- `ur-workertools agent request-human` — alias for `ur-workertools status request-human`
