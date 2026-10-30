# workerd

Worker daemon running inside containers. The container entrypoint calls `exec workerd`,
making workerd PID 1 — the container lifecycle is tied to workerd.

## Library + binary

The crate is a library (`src/lib.rs`) plus the thin container binary (`src/main.rs`). The
library holds everything a host-side runtime (builderd, one daemon per local worker) needs to
reuse; the binary only resolves container defaults from the environment and wires them in.

- `paths::WorkerPaths` — every filesystem location workerd touches (`workspace`, `agent_home`,
  `skill_hooks`, `skills_dest`, `instructions_dest`, `settings_template`/`settings_dest`
  (`None` for agents without a settings file), `host_hooks`, `agent_shared`, `shim_dir`).
  `WorkerPaths::container(agent)` is the **only** place container path literals live; other
  runtimes build the struct with a literal. Every init manager takes `WorkerPaths` in its
  constructor and never hardcodes a path.
- `exit_watcher::ExitWatcherManager` polls the agent pane (`AgentPane`, implemented for
  `tmux::Session`) and calls the injected `AgentExitSink::agent_exited(AgentExitReason)` exactly
  once, then returns. It never exits the process itself.
- `container_exit::ContainerExitSink` is the container's sink: `WorkerStop` RPC then wait for the
  server to kill the container, falling back to `process::exit(0)` if the RPC fails.
- `skill_layout::SkillLayout` — `AgentHome` (containers: skills in `skills_dest/<name>/`,
  invoked bare) or `NamespacedPlugin { name }` (`SkillLayout::ur_plugin()`, local workers:
  `skills_dest/.claude-plugin/plugin.json` + `skills_dest/skills/<name>/`, invoked `ur:<name>`).
  Injected into `InitSkillsManager::new(paths, layout)`,
  `InitInstructionsManager::new(paths, agent, layout)` and `WorkerDaemonServiceImpl::new`; the
  container binary always passes `AgentHome`. See `docs/codeflows/skill-loading.md#skill-layouts-skilllayout`.
- `grpc_service::WorkerDaemonServiceImpl::new(WorkerIdentity, AgentType, SkillLayout, tmux::Session,
  ExitWatcherManager)` — the tmux session is injected (no handler calls `Session::agent()`);
  `spawn_exit_watcher()` starts the watcher and returns its `JoinHandle` so a host can abort it.
- `dispatch::dispatch_commands` builds the dispatch buffer for server-driven dispatches.
- `test_support` (test-only) has `temp_paths(tmp, agent)` and the crate-wide `ENV_LOCK`; use them
  in tests that need `WorkerPaths` or mutate env vars.

Three modes:
- `workerd` (no args) — runs init, startup hooks, then daemon. Used by every agent container entrypoint.
- `workerd init` — synchronous initialization only (skills, git hooks, hostexec shims).
- `workerd daemon` — daemon without init or startup hooks (expects initialization to have been handled by the caller).

Startup sequence (daemon mode):
1. Runs initialization, including hostexec shim creation.
2. Runs executable startup hooks through `StartupHooksManager`: synchronous hooks first, then detached background hooks. In-repo hooks live under `/workspace/ur-hooks/{startup,startup-bg}/`; host overlays under `/var/ur/host-hooks/{startup,startup-bg}/` win on filename conflicts.
3. Resolves `let agent = AgentType::from_env()` (reads `UR_AGENT_TYPE`, defaults to claude) and **binds the gRPC listener on 9120 before launching the agent** — the agent's `SessionStart` hook calls `NotifyIdle` immediately, and a refused connection drops the idle signal that triggers dispatch. Creates tmux session `agent` (220x55), sets status line with worker ID.
4. Launches the agent via `tmux send-keys(agent.spawn_command(model))`. For Claude, when `UR_WORKER_MODEL` is set, this produces `claude --model <name>` — the model is passed via CLI flag (NOT settings.json), because Claude Code rewrites `~/.claude/settings.json` on startup and silently drops the `model` key.
5. Spawns healthz HTTP server on port 9119 (Docker HEALTHCHECK).
6. Serves gRPC on the already-bound 9120 listener (long-lived, keeps the process alive).

The worker health check becomes healthy only after startup hooks complete. Builderd waits for
that transition during `LaunchWorker`; if a synchronous hook fails, it returns the container
logs through gRPC and removes the failed container before the server records a worker row.

The exit watcher (`exit_watcher.rs`, `AgentWatchState`/`advance_agent_watch_state`) that detects a design worker's agent exiting is deliberately agent-agnostic — it matches shell-vs-non-shell foreground processes (`is_shell_process`), never the agent's binary name. Claude Code's foreground process is `node`, not `claude`, so `AgentType` deliberately carries no binary-name accessor for this code to reach for.

Project-specific background processes are expressed as generic startup hooks. This repository's
`ur-hooks/startup-bg/` starts cargo sweep and bacon without requiring toolchain-specific images.

## Sending commands to the agent (`NotifyIdle`)

`NotifyIdle` is driven by Claude Code's `Stop` hook (`ur-workertools notify-idle`), which Claude
Code runs **synchronously and waits for**. So despite the name, the agent is *not yet idle*
while this RPC is being served, and text typed at that point lands in Claude Code's
queued-message buffer instead of executing — a queued `/clear` or `/implement` never runs as
a command. This is the failure mode where dispatched commands appear to be typed but never
submit.

`spawn_deferred_send` is therefore how anything hook-driven reaches the agent: the RPC returns
immediately, the hook completes, Claude Code returns to its prompt, and only then
(`HOOK_RETURN_GRACE`, 750ms later) is the text typed in. Never call `send_keys` inline from
`NotifyIdle`. This rationale is Claude-specific (its `Stop` hook is synchronous), but the
deferred send stays in place for every agent rather than being special-cased away: Codex's
command handlers accept `async = true` so the hazard may not apply there, while AGY hooks are
synchronous like Claude's. A uniform harmless deferred send keeps this path agent-agnostic.

`Implement` / `Design` / `AddressFeedbackTickets` send inline instead — they arrive from the
server rather than from a hook, so their errors can propagate back to the caller.

### Dispatch commands are agent-phrased

`WorkerDaemonServiceImpl.agent: AgentType` is resolved once in `main` and
injected into the gRPC service — no handler calls `AgentType::from_env()` itself. The
`Implement` / `Design` / `AddressFeedbackTickets` handlers build their tmux commands via the
shared `dispatch_commands(agent, &skill_layout, skill, args)` helper in `dispatch.rs`, which is just
`[agent.clear_command(), agent.skill_invocation(&layout.qualify(skill), args)]` (plugin layout
yields `/ur:implement ur-x`) — for Claude that's
`["/clear", "/implement ur-x"]`; for Codex, which has no custom slash commands, it's
`["/new", "Run the \`implement\` skill. Arguments: ur-x"]`; AGY uses Claude-shaped phrasing,
including `/clear`. No slash-command literal lives in `grpc_service.rs` itself.

Init phase (`crates/workerd/src/init/{skills,instructions,settings}.rs`, split by concern). `main` resolves the agent (`AgentType::from_env()`) and `WorkerPaths::container(agent)` **once** and injects them into every manager's constructor — no init manager reads `UR_AGENT_TYPE` or `$HOME` itself, so the whole phase cannot disagree about which agent it is setting up:
- `InitSkillsManager` wipes `skills_dest` and copies skills from `<agent_shared>/potential-skills/` (baked, agent-agnostic) based on `$UR_WORKER_SKILLS` (container: `~/.claude/skills/` or `~/.gemini/config/skills/`), then renders each UTF-8 file (`init/skill_render.rs`): `%UR_SKILL_HOOKS%` → `paths.skill_hooks` always; for `NamespacedPlugin`, references to other installed skills are rewritten to `ur:<name>` first and `plugin.json` is written. Skill sources MUST use `@%UR_SKILL_HOOKS%/…`, never a literal hooks path.
- `InitInstructionsManager` appends a `# Skill Precedence` fragment (prefer `ur:<name>` over `<name>`) only for `NamespacedPlugin`.
- `InitInstructionsManager` copies the strategy-specific instruction file from `<agent_shared>/instructions/` based on `$UR_WORKER_INSTRUCTION_STRATEGY` env var, appends `<agent_shared>/shared-instructions/*.md` fragments, and writes the result to `instructions_dest` (project instructions go to `PROJECT_<filename>` beside it). AGY cannot read a workspace `CLAUDE.md`, so its project instructions are folded directly into `~/.gemini/config/AGENTS.md`.
- `InitSettingsManager` copies `settings_template` (container: `~/{agent.home_subdir()}/potential-settings.json`, baked into the agent image) to `settings_dest` verbatim; both are `None` for agents without a settings file. The `model` key is intentionally NOT injected here — see step 4 of the daemon startup sequence above for why.
- `InitGitHooksManager` copies git hooks from `<workspace>/ur-hooks/git/` (in-repo), then `<host_hooks>/git/` (host overlay, wins on conflict), into `<workspace>/.git/hooks/`
- `InitSkillHooksManager` copies skill hooks from `<workspace>/ur-hooks/skills/` (in-repo), then `<host_hooks>/skills/` (host overlay, wins on conflict), into `skill_hooks` (container: always under `/home/worker`, e.g. `/home/worker/.claude/skill-hooks/`)
- Calls `ListHostExecCommands` RPC on ur-server (retries with backoff)
- Generates shims in `shim_dir` (container: `~/.local/bin/`) that call `ur-workertools host-exec <command> "$@"`
