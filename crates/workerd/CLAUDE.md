# workerd

Worker daemon running inside containers. The container entrypoint calls `exec workerd`,
making workerd PID 1 — the container lifecycle is tied to workerd.

Three modes:
- `workerd` (no args) — runs init, then daemon. Used by the base container entrypoint.
- `workerd init` — synchronous initialization only (skills, git hooks, hostexec shims). Used by image-specific entrypoints that need to launch background processes between init and daemon.
- `workerd daemon` — daemon without init (expects `workerd init` to have been called already). Used by image-specific entrypoints after init + background processes.

Startup sequence (daemon mode):
1. Resolves `let agent = AgentType::from_env()` (reads `UR_AGENT_TYPE`, defaults to claude). Creates tmux session `agent` (220x55), sets status line with worker ID
2. Launches the agent via `tmux send-keys(agent.spawn_command(model))`. For Claude, when `UR_WORKER_MODEL` is set, this produces `claude --model <name>` — the model is passed via CLI flag (NOT settings.json), because Claude Code rewrites `~/.claude/settings.json` on startup and silently drops the `model` key.
3. Spawns healthz HTTP server on port 9119 (Docker HEALTHCHECK)
4. Starts gRPC server on port 9120 (long-lived, keeps the process alive)

The exit watcher (`AgentWatchState`/`advance_agent_watch_state`) that shuts down a design worker's container is deliberately agent-agnostic — it matches shell-vs-non-shell foreground processes (`is_shell_process`), never the agent's binary name. Claude Code's foreground process is `node`, not `claude`, so `AgentType` deliberately carries no binary-name accessor for this code to reach for.

Image-specific background processes (e.g., bacon, cargo sweep for rust variant) are launched
by the image's entrypoint.sh between `workerd init` and `exec workerd daemon` — NOT by workerd
itself. This keeps workerd image-agnostic.

## Sending commands to the agent (`NotifyIdle`)

`NotifyIdle` is driven by Claude Code's `Stop` hook (`workertools notify-idle`), which Claude
Code runs **synchronously and waits for**. So despite the name, the agent is *not yet idle*
while this RPC is being served, and text typed at that point lands in Claude Code's
queued-message buffer instead of executing — a queued `/clear` or `/implement` never runs as
a command. This is the failure mode where dispatched commands appear to be typed but never
submit.

`spawn_deferred_send` is therefore how anything hook-driven reaches the agent: the RPC returns
immediately, the hook completes, Claude Code returns to its prompt, and only then
(`HOOK_RETURN_GRACE`, 750ms later) is the text typed in. Never call `send_keys` inline from
`NotifyIdle`. This rationale is Claude-specific (its `Stop` hook is synchronous), but the
deferred send stays in place for both agents rather than being special-cased away: codex's
command handlers accept `async = true` so the hazard may not apply there, but a needless-only
divergence between agents is worse than a harmless deferred send.

`Implement` / `Design` / `AddressFeedbackTickets` send inline instead — they arrive from the
server rather than from a hook, so their errors can propagate back to the caller.

### Dispatch commands are agent-phrased

`WorkerDaemonServiceImpl.agent: AgentType` is resolved once in `main` (`run_daemon_only`) and
injected into the gRPC service — no handler calls `AgentType::from_env()` itself. The
`Implement` / `Design` / `AddressFeedbackTickets` handlers build their tmux commands via the
shared `dispatch_commands(agent, skill, args)` helper in `grpc_service.rs`, which is just
`[agent.clear_command(), agent.skill_invocation(skill, args)]` — for Claude that's
`["/clear", "/implement ur-x"]`; for Codex, which has no custom slash commands, it's
`["/new", "Run the \`implement\` skill. Arguments: ur-x"]`. No slash-command literal lives in
`grpc_service.rs` itself.

Init phase (`crates/workerd/src/init/{skills,instructions,settings}.rs`, split by concern). `run_init` resolves the agent (`AgentType::from_env()`) and home (`init::worker_home()`) **once** and injects both into every manager's constructor — no init manager reads `UR_AGENT_TYPE` itself, so the whole phase cannot disagree about which agent it is setting up:
- `InitSkillsManager` copies skills from `.agent-shared/potential-skills/` (baked, agent-agnostic) based on `$UR_WORKER_SKILLS` env var into `~/{agent.home_subdir()}/{agent.skill_subdir()}/` (e.g. `~/.claude/skills/`)
- `InitInstructionsManager` copies the strategy-specific instruction file from `.agent-shared/instructions/` based on `$UR_WORKER_INSTRUCTION_STRATEGY` env var, appends `.agent-shared/shared-instructions/*.md` fragments, and writes the result to `~/{agent.home_subdir()}/{agent.instruction_filename()}` (e.g. `~/.claude/CLAUDE.md`)
- `InitSettingsManager` copies `~/{agent.home_subdir()}/potential-settings.json` (baked into the agent image) to `~/{agent.home_subdir()}/{agent.settings_filename()}` verbatim, gated on `agent.settings_filename().is_some()`, so Claude Code picks up hooks/permissions. The `model` key is intentionally NOT injected here — see step 2 of the daemon startup sequence above for why.
- Copies git hooks from `/workspace/ur-hooks/git/` (in-repo), then `/var/ur/host-hooks/git/` (host overlay, wins on conflict), into `/workspace/.git/hooks/`
- `InitSkillHooksManager` copies skill hooks from `/workspace/ur-hooks/skills/` (in-repo), then `/var/ur/host-hooks/skills/` (host overlay, wins on conflict), into `~/{agent.home_subdir()}/{agent.skill_hooks_subdir()}/` (e.g. `~/.claude/skill-hooks/`)
- Calls `ListHostExecCommands` RPC on ur-server (retries with backoff)
- Generates shims in `~/.local/bin/` that call `workertools host-exec <command> "$@"`
