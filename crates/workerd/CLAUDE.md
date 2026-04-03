# workerd

Worker daemon running inside containers. The container entrypoint calls `exec workerd`,
making workerd PID 1 — the container lifecycle is tied to workerd.

Three modes:
- `workerd` (no args) — runs init, then daemon. Used by the base container entrypoint.
- `workerd init` — synchronous initialization only (skills, git hooks, hostexec shims). Used by image-specific entrypoints that need to launch background processes between init and daemon.
- `workerd daemon` — daemon without init (expects `workerd init` to have been called already). Used by image-specific entrypoints after init + background processes.

Startup sequence (daemon mode):
1. Creates tmux session `agent` (220x55), sets status line with worker ID
2. Launches Claude Code via `tmux send-keys`
3. Spawns healthz HTTP server on port 9119 (Docker HEALTHCHECK)
4. Starts gRPC server on port 9120 (long-lived, keeps the process alive)

Image-specific background processes (e.g., bacon, cargo sweep for rust variant) are launched
by the image's entrypoint.sh between `workerd init` and `exec workerd daemon` — NOT by workerd
itself. This keeps workerd image-agnostic.

Init phase:
- Copies skills from potential-skills based on `$UR_WORKER_SKILLS` env var
- Copies strategy-specific CLAUDE.md from potential-claudes based on `$UR_WORKER_CLAUDE` env var
- Copies git hooks from `$UR_GIT_HOOKS_DIR` (or default `/workspace/ur-hooks/git/`) into `/workspace/.git/hooks/`
- Copies skill hooks from `$UR_SKILL_HOOKS_DIR` (or default `/workspace/ur-hooks/skills/`) into `~/.claude/skill-hooks/`
- Calls `ListHostExecCommands` RPC on ur-server (retries with backoff)
- Generates shims in `~/.local/bin/` that call `workertools host-exec <command> "$@"`
