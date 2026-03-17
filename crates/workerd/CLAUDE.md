# workerd

Worker daemon running inside containers. The container entrypoint calls `exec workerd`,
making workerd PID 1 — the container lifecycle is tied to workerd.

Two modes:
- `workerd` (no args) — runs init, then launches tmux session, Claude Code, background processes, and gRPC server. This is the primary mode used by the container entrypoint.
- `workerd init` — synchronous initialization only (skills, git hooks, hostexec shims). Available for standalone use but automatically run by the daemon on startup.

Startup sequence (daemon mode):
1. Init phase: skills, git hooks, hostexec shim creation
2. Background processes: cargo sweep + bacon (only if their shims exist, i.e., rust variant containers)
3. Creates tmux session `agent` (220x55), launches Claude Code via send-keys
4. Spawns healthz HTTP server on port 9119 (Docker HEALTHCHECK)
5. Starts gRPC server on port 9120 (long-lived, keeps the process alive)

Init phase:
- Copies skills from potential-skills based on `$UR_WORKER_SKILLS` env var
- Copies strategy-specific CLAUDE.md from potential-claudes based on `$UR_WORKER_CLAUDE` env var
- Copies git hooks from `$UR_GIT_HOOKS_DIR` into `/workspace/.git/hooks/`
- Calls `ListHostExecCommands` RPC on ur-server (retries with backoff)
- Generates shims in `~/.local/bin/` that call `workertools host-exec <command> "$@"`
