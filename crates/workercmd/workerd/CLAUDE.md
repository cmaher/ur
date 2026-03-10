# ur-workerd

Worker daemon running inside containers. Queries ur-server for available host-exec
commands and creates bash shims in `/home/worker/.local/bin/`.

- Started by container entrypoint as a background process
- Calls `ListHostExecCommands` RPC on ur-server at startup
- Generates shims that call `ur-tools host-exec <command> "$@"`
- Retries with backoff if ur-server is not ready
- Stays running for future daemon uses
