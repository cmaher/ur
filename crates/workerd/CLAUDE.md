# workerd

Worker daemon running inside containers. Handles all container initialization
and creates bash shims for host-executed commands.

Two modes:
- `workerd init` — synchronous initialization: skills, git hooks, hostexec shims. Run by entrypoint before anything else.
- `workerd` (no args) — background daemon loop. Stays alive for future daemon uses.

Init phase:
- Copies skills from potential-skills based on `$UR_WORKER_SKILLS` env var
- Copies git hooks from `$UR_GIT_HOOKS_DIR` into `/workspace/.git/hooks/`
- Calls `ListHostExecCommands` RPC on ur-server (retries with backoff)
- Generates shims in `~/.local/bin/` that call `workertools host-exec <command> "$@"`
