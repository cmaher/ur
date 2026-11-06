# Local Workers (`runtime = "local"`)

A local worker runs the agent natively on the host, in a pool slot, inside a tmux session that
builderd owns. Slot acquisition, dispatch (`WorkerDaemonService`) and the worker row are the same
as for containers. What changes is that builderd launches and supervises the agent in tmux, and
serves the worker's daemon in-process, where a container would run `docker run` and `workerd`.
Design: ur-wdfir. Built on branch ur-y94lc (ur-vatwl, ur-4p34c, ur-y1f6n, ur-j5puq, ur-dc3yc,
ur-hld78).

This file is the cross-cutting map. These codeflows have the detail:

- [builderd.md](builderd.md#builderlocalworkerservice-and-the-routed-workerdaemonservice): RPCs, session env, Stop barrier, adoption, reconcile pass
- [pool-git-builder-flow.md](pool-git-builder-flow.md#slot-claim-ur-j5puq): DB slot claim and unlink-only release
- [server-lifecycle.md](server-lifecycle.md): `ur start` builderd health wait, `ur stop` double sweep, loopback worker port
- [config.md](config.md#runtime-precedence): `runtime` key and its precedence
- [skill-loading.md](skill-loading.md#skill-layouts-skilllayout): `ur` plugin layout, `ur:` rewrite, `%UR_SKILL_HOOKS%`

Scope: Claude only (`AgentType::supports_local_runtime`), and pool launches only. A `local = true`
project, a `-w` launch and a launch with no project are rejected with `FailedPrecondition` in
`crates/server/src/launch_runtime.rs` before any slot, filesystem or container work. A local
launch never falls back to a container.

## Architecture

```
host ─────────────────────────────────────────────────────────────────────────────
 ur CLI ──WorkerLaunch/Stop──▶ ur-server (container)          ur worker attach
                               │ LaunchManager → WorkerManager      │
                               │ LocalWorkerReconcileManager (15s)  │ tmux -L ur attach
                               │ worker port on 127.0.0.1:<worker_port>
                               ▼ UR_BUILDERD_ADDR                   ▼
 builderd (native) ── BuilderLocalWorkerService ── LocalWorkerManager
   │                  WorkerdRouter (ur-worker-id) ─▶ LocalWorkerRegistry
   │                                                   └ per id: WorkerDaemonServiceImpl
   │                                                            + exit watcher
   │                  WorkerBarrier (per-id lock + tombstones)
   ▼
 tmux server, socket `ur` ─ session ur-<id> (cwd = pool slot)
     window 0: claude --settings … --plugin-dir … (auto mode)
     window bg: background startup hooks
       └ hooks/skills: ur-workertools ─▶ UR_WORKERD_ADDR (builderd, routed by ur-worker-id)
                                       ─▶ UR_SERVER_ADDR = 127.0.0.1:<worker_port>
       └ git / gh / ur run natively with the user's credentials (no hostexec)
```

The server's dispatch reaches a local worker through `WorkerdAddressResolver`
(`crates/server/src/workerd_address.rs`). It calls builderd's address with `ur-worker-id`
metadata, and `WorkerdRouter` (`crates/builderd/src/workerd_router.rs`) hands the call to that
worker's daemon (missing header: `InvalidArgument`, unknown id: `NotFound`). The daemon is built
with `SkillLayout::ur_plugin()`, so dispatch types `/clear` and then `/ur:implement <ticket>`.

## Launch sequence

```
ur worker launch -p <project> [--runtime local]
1. ur-server LaunchManager
   ├─ resolve_launch_runtime → validate_launch_runtime          (launch_runtime.rs)
   ├─ check_credentials_seeded: container only (skipped for local)
   └─ acquire_slot: claim_slot inserts the provisioning row + worker_slot link in one txn,
      then RecycleSlot / PrepareNewSlot and branch checkout     (pool-git-builder-flow.md)
2. WorkerManager::run_and_record → run_and_record_local         (local_launch.rs)
   ├─ build_local_launch_request: host paths only (templates resolve against the host config
   │  dir); mounts + context repos, memory/brain dirs created (no chown), instruction_md,
   │  skills, host_skills, allow_no_verify; env = UR_SERVER_ADDR=127.0.0.1:<worker_port>,
   │  UR_WORKER_SECRET, UR_PROJECT, UR_WORKER_FORGE
   ├─ record_provisioning_worker (container_id = session name ur-<id>, secret)
   └─ LaunchLocalWorker ─────────────────────────────────────────────┐
3. builderd LocalWorkerManager::launch, on its own task            ◀┘
   ├─ barrier.begin_launch(id): per-id lock; FailedPrecondition if tombstoned
   ├─ validate: no '.'/':' in id; UR_SERVER_ADDR + UR_WORKER_SECRET present (InvalidArgument)
   ├─ AlreadyExists if registered or a ur-<id> session already exists
   ├─ LocalWorkspaceManager::build: agent check, check_assets, drive, .ur/owner,
   │  mount/brain links, memory link, plugin skills, skill hooks, git hooks into the slot,
   │  settings.json, instructions.md (a failed build removes what it built)
   ├─ sync startup hooks in the slot (failure → FailedPrecondition with stderr)
   ├─ tmux -L ur new-session -d -s ur-<id> -c <slot> -e <env…>
   ├─ background startup hooks: new-window bg -c <slot>
   ├─ registry insert + exit watcher (before the agent: SessionStart calls NotifyIdle)
   └─ send-keys the agent argv into window 0
   any failure after build → teardown → error (with "teardown also failed" if it did)
4. ur-server finalize_worker_launch: provisioning → running (compare-and-set)
   ├─ false (a stop landed mid-launch) → "stopped during launch"; the stopper finishes it
   └─ launch error → abandon_launch → stop_by_worker_id (row stopped, or stopping on failure)
```

Agent argv (`LocalWorkspaceManager::agent_argv`):

```
claude --settings <drive>/.ur/settings.json
       --append-system-prompt-file <drive>/.ur/instructions.md
       --plugin-dir <drive>/.ur/plugin
       --add-dir <drive> [--add-dir <link source>…]   (each mount and the brain)
       [--model <model>] --effort <effort or agent default>
```

Session env (builderd's vars come last, and the request cannot override them): the request env,
then `UR_WORKER_ID`, `UR_WORKERD_ADDR`, `UR_AGENT_TYPE`, `UR_DRIVE`, `UR_RUNTIME=local`, and
`UR_WORKER_INSTRUCTION_STRATEGY` when it is set. Adoption reads them back. builderd sets
`UR_WORKERD_ADDR` from its own listen address (an unspecified bind becomes `127.0.0.1`), because
on Linux it binds the Docker bridge IP.

## Drive layout

```
<workspace>/drive/<worker_id>/                          (LocalWorkspaceManager)
  <mount dest without leading />  -> <mount source>     container.mounts and context repos
  brain                           -> <brain_dir>
  .ur/
    owner                    agent, slot path, memory target (one per line)
    settings.json            layered over the user's settings via --settings
    instructions.md          strategy + shared fragments + skill precedence + project
                             instruction (inlined) + drive map + host-tools note
    plugin/.claude-plugin/plugin.json, plugin/skills/<name>/   the `ur` plugin
    skill-hooks/<skill>/*.md       ur-hooks/skills overlaid by <config>/projects/<key>/hooks/skills
    skill-sources/potential-skills/<name> -> resolved skill source
<slot>/.git/hooks/                ur-hooks/git overlaid by the host git hooks (InitGitHooksManager)
~/.claude/projects/<encoded slot>/memory -> <memory_dir>   only when the project has one
```

- `<encoded slot>` is the slot path with every non-alphanumeric ASCII character replaced by `-`
  (`AgentType::local_memory_dir`). The memory dir is created if it is missing. A real non-empty
  directory already at the link path fails the launch with `FailedPrecondition`. A symlink there
  that points somewhere else gets replaced.
- A mount source that does not exist, or a mount nested inside another linked mount, fails with
  `FailedPrecondition`.
- Nothing else is written to `~/.claude`. The user's own settings, skills and memory stay in
  effect for the session.

## Skills, settings and instructions

- **Plugin.** ur skills install as the `ur` plugin (`SkillLayout::ur_plugin()`). This is
  needed because a user skill shadows a same-named `--add-dir` skill (see the spike results).
  `ur:` is preferred in three ways: dispatch commands are qualified (`/ur:implement`), cross-skill
  references inside installed skills are rewritten to `ur:<name>`, and `instructions.md` includes
  the `# Skill Precedence` rule. See [skill-loading.md](skill-loading.md#skill-layouts-skilllayout).
- **Sources.** Each requested skill comes from its `host_skills` entry (the server's merged
  `[skills]` host paths) when there is one. Otherwise it comes from
  `<agent_shared>/potential-skills/<name>`, so host skills shadow baked ones, as in containers.
  `check_assets` fails the launch (`FailedPrecondition`) if a skill or the strategy instruction
  file is missing. In containers the init managers only warn about this.
- **Agent-shared assets.** builderd `--agent-shared` (default `$UR_CONFIG/agent-shared`) is the
  host copy of the image's `.agent-shared`. `scripts/build/install-agent-assets.sh <dest>` fills
  it the same way as `containers/worker-base/Dockerfile`: vendored superpowers skills first, then
  the repo's `potential-skills/` (which win name collisions), then `instructions/` and
  `shared-instructions/`. Each run replaces the previous copy. `make install` (`install.sh`) and
  the acceptance suite both run it.
- **`%UR_SKILL_HOOKS%`** renders to `<drive>/.ur/skill-hooks`.
- **settings.json** (`crates/builderd/src/local_settings.rs`):
  `permissions.defaultMode = "auto"`, `skipAutoPermissionPrompt = true`, and `SessionStart` and
  `Stop` hooks that run `ur-workertools notify-idle`. It also has these `permissions.deny` rules:
  `Bash(gh pr merge:*)`, `Bash(gh pr close:*)`, `Bash(gh pr review --approve:*)`,
  `Bash(gh pr review --request-changes:*)`, `Bash(git worktree:*)`, plus
  `Bash(git push --no-verify:*)` unless the project sets `allow_no_verify`.
- **No hostexec policy locally.** `git`, `gh`, `ur` and the hostexec commands run natively with
  the user's credentials. The Lua hostexec transforms, the shims and Squid are not used. The deny
  rules copy the main policy blocks as guardrails, but they are prefix matches, not a sandbox, and
  `instructions.md` states the same rules in prose ("Host Tools"). This is deliberate: the premise
  is that auto mode is trusted enough to run without the container sandbox.
- **Credentials.** Nothing is seeded or mounted. Claude uses the host login (OS keychain), and the
  server skips `check_credentials_seeded`. See [process-launch-credentials.md](process-launch-credentials.md).

## Slot ownership

The database is the only record of who owns a slot (ur-j5puq). Full flow:
[pool-git-builder-flow.md](pool-git-builder-flow.md#slot-claim-ur-j5puq).

- **Claim.** `WorkerRepo::claim_slot` inserts the `provisioning` worker row and its `worker_slot`
  link in one transaction. `UNIQUE (slot_id)` (migration 005) decides between racing launches.
  Migration 005 resolves existing duplicates first: links of inactive workers are dropped, a
  `running` worker beats the others and then the newest wins, and each loser moves to `stopping`.
- **Transitions are conditional UPDATEs.** finalize `provisioning → running`, `begin_stopping`
  (active → `stopping`), `finish_stop` (`stopping → stopped` + delete this worker's link).
- **Release is unlink-only** (both runtimes). The slot's contents are not touched. The next
  acquire's `RecycleSlot` resets it under the new owner's claim. Downside: a stopped worker's
  dirty tree stays on disk until then.
- **Release requires confirmation.** `finish_stop` runs only after `StopLocalWorker` returns Ok.
  On any failure the row stays `stopping` and keeps its slot.

## builderd Stop barrier

`WorkerBarrier` (`crates/builderd/src/barrier.rs`), used by the local and container services:
`begin_stop(id)` records an in-memory tombstone, then takes the id's lock, which waits for any
in-flight launch. `begin_launch(id)` takes the lock and refuses a tombstoned id. Launches run on
a spawned task, so a cancelled RPC cannot leave one half done. So **Stop Ok means the id is not
running in this builderd and never will be.** Tombstones expire after `TOMBSTONE_TTL` (1h). A
builderd restart loses them safely: in-flight launches die with the process, and worker ids are
never reused. Stopping an unregistered id kills a leftover `ur-<id>` session we own and tears
down a leftover drive. Details: [builderd.md](builderd.md#stop-is-a-barrier-ur-j5puq).

## Teardown

Builderd teardown (`LocalWorkerManager::teardown`) runs in this order: kill session `ur-<id>`
(including window `bg`), abort the exit watcher (unless the watcher is the caller), then
`LocalWorkspaceManager::teardown` (remove the slot memory symlink only while it still points at
the target in `.ur/owner`, then `remove_dir_all` the drive, which never follows symlinks), then
deregister. Each step is idempotent. If a step fails, teardown stops there and the id stays
registered (listed), so a retried Stop runs it again. Link targets (mount sources, brain, memory)
are never removed.

| Termination | Trigger | Path |
|---|---|---|
| Agent exits | `ExitWatcherManager` (workerd lib) polls window 0: pane dead, probe failure, or (design workers only) the foreground process back at a shell | `LocalExitSink` → teardown. Within 15s the reconcile pass sees the id missing from the list → `stop_by_worker_id` (a Stop of an unknown id is Ok) → `finish_stop`. A non-design agent that quits leaves its shell alive, so nothing fires and the worker stays `running` until stopped (same as containers) |
| External kill (`tmux -L ur kill-session`, agent crash) | same exit watcher (session gone) | same as agent exit |
| `ur worker stop`, `launch -f`, `--rm` attach exit, workflow completion (merge, GitHub poller), ticket close | `WorkerStop` / server-internal stop | `stop_by_worker_id`: `begin_stopping` → `StopLocalWorker` (barrier) → teardown → `finish_stop`; a failure leaves the row `stopping` for the reconcile loop |
| Failed launch | builderd step error, LaunchLocalWorker error, or finalize error | builderd: a failed build removes what it built, and a later failure runs teardown before returning. Server: `abandon_launch` → `stop_by_worker_id` (row `stopped`, or `stopping` if Stop fails). Finalize false: the concurrent stopper finishes it |
| builderd restart / host reboot | builderd startup, `adopt_existing` before `serve` | Our `ur-<id>` sessions (`UR_DRIVE` under our drive root) are re-registered from their env and start dir with a new exit watcher. Ours with unusable env are killed. Sessions that are not ours are left alone. Every drive without a registered worker gets `teardown_orphan` (its memory link is kept if a registered worker uses that slot). Listing failure, a read error on a live session, or a failed kill aborts startup, and `ur start` reports the builderd logs |
| Server restart | `LocalWorkerReconcileManager::startup`, before serving or dispatching | Every local `provisioning` row (an unfinished launch) → `stopping`, then one reconcile pass. Container `reconcile_workers` skips local rows |
| Periodic reconcile | every 15s (`DEFAULT_LOCAL_RECONCILE_INTERVAL`) | Rows are read first, then `ListLocalWorkers`. `running` + not listed, `running` + session dead, or `stopping` → `stop_by_worker_id`. `provisioning` → none. A listed id with no active row (re-read first) → `StopLocalWorker`. A list error changes nothing |
| `ur stop` | CLI | Server stops running workers, then builderd sweep (`ListLocalWorkers` → `StopLocalWorker` each, from the host), `compose down`, a second sweep (catches a launch the server finished during the first), then builderd exits. A sweep failure aborts `ur stop`. If builderd is not running: warn, and its next start adopts the sessions. See [server-lifecycle.md](server-lifecycle.md) |

**Own-session rule** (ur-hld78): every ur instance on the host shares the `ur` socket. builderd
adopts, kills or lists a `ur-*` session only if its `UR_DRIVE` is set and lies under its own
`<workspace>/drive/` (`owns_session`). So another instance's sessions and hand-made `ur-*`
sessions survive builderd restarts and the acceptance suite.

## Attach

`ur worker attach <id>` (also `launch -a` and `--rm`) goes through `AttachManager`
(`crates/ur/src/attach.rs`). It resolves the worker with `WorkerList` (newest running row for the
process id) rather than `WorkerInfo` (ur-z5u7e), then branches on `runtime`:

- container: `docker exec -it <prefix><id> tmux -u attach-session -t =agent` (unchanged)
- local: `tmux -L ur -u attach-session -t =ur-<worker_id_full>` on the host. `TMUX` is inherited,
  so attaching from the user's own tmux works and tmux still refuses to nest the `ur` server in
  itself. There is no health wait (launch returns only after the session exists). A missing
  session gives a clear error that points to `ur worker list`.

`--rm` stops the worker through the server once the attach ends, or on SIGHUP, SIGINT or SIGTERM.

## Spike results (ur-wdfir, Claude Code 2.1.314)

| Check | Result |
|---|---|
| Skills in `--add-dir <dir>/.claude/skills` load, invocable as `/name` | ✅ |
| `--settings <file>` hooks (SessionStart, Stop) fire, layered on user settings | ✅ |
| `--append-system-prompt-file` content reaches the model | ✅ |
| `@/abs/path` inside a skill expands (skill-hook patching) | ✅ |
| User skill vs `--add-dir` skill of same name | ❌ user wins → why we use a plugin |
| `--plugin-dir` skill invoked `/ur:<name>` with a colliding user skill | ✅ plugin copy runs |
| Natural-language "run the X skill" with collision + precedence prompt (haiku, sonnet) | ✅ `ur:` chosen; without the prompt haiku chose the user copy |

## Known limits and trade-offs

Deliberate:

- **`:ro` is advisory.** A read-only mount is still a writable symlink. The drive map in
  `instructions.md` marks it "read-only: advisory — do not write".
- **Auto mode is trusted.** There is no container sandbox, no hostexec policy and no Squid. The
  deny rules and the instructions are the only guardrails.
- Release leaves the stopped worker's tree in the slot until the next acquire resets it.

Open tickets:

- ur-ezhdb: Slot-release gaps left by ur-j5puq: container launch cleanup race, builderd
  restart with an in-flight launch, migration 005 losers revived by container reconcile, no
  container `stopping` retry.
- ur-rkufm: Container startup reconcile revives a stopped row without relinking its slot
  (container only).
- ur-z5u7e: `WorkerInfo` returns empty `runtime`/`agent_status` for CLI callers; attach works
  around it with `WorkerList`.

Other limits (in code comments):

- Sync startup hooks run with builderd's environment, not the session env.
- An adopted design worker whose agent had already returned to the shell before the restart is
  not detected as exited. A Stop still tears it down.
- Orphan-drive failures at builderd startup are only logged. Drives hold only links, and the next
  startup retries.
- A mount dest under `.ur/` writes the generated files through into the mount source. Dests are
  trusted config.
- With no `instruction_strategy`, the project `instruction_md` and the skill-precedence rule are
  left out (same as the container).
- Forge collaboration instructions (e.g. Gitea → `tea`) are not appended locally:
  `write_customization` passes `forge: None`, even though `UR_WORKER_FORGE` is in the session env.
- `UR_BRAIN` is not set. Brain skills fall back to the `brain` field of `ur project list`.
- `ur worker launch -f` ignores the stop result before relaunching
  (`crates/ur/src/main.rs`), so the reconcile loop retries a failed stop.
- Memory and brain dirs are created by the server process without a chown, so they are
  root-owned on native Linux (fine on Docker Desktop).

## Key files

- `crates/builderd/src/local_worker.rs`: `LocalWorkerManager`, `LocalWorkerRegistry`, adoption, teardown
- `crates/builderd/src/local_workspace.rs`: drive build/teardown, argv, memory link
- `crates/builderd/src/local_settings.rs`: settings.json, deny rules, drive map
- `crates/builderd/src/barrier.rs`: `WorkerBarrier`
- `crates/builderd/src/local_worker_handler.rs`, `workerd_router.rs`: gRPC surface
- `crates/server/src/local_launch.rs`, `local_reconcile.rs`, `launch_runtime.rs`, `workerd_address.rs`
- `crates/workflow_db/src/worker_repo.rs`, `migrations/005_worker_slot_unique_slot.sql`
- `crates/ur/src/attach.rs`, `crates/ur/src/builderd.rs` (`ur stop` sweeps), `crates/ur/src/compose.rs` (loopback worker port)
- `crates/tmux/src/lib.rs` (`Session::on_socket`), `crates/workerd/src/skill_layout.rs`
- `proto/builder_local_worker.proto`, `scripts/build/install-agent-assets.sh`
- `crates/acceptance/tests/e2e.rs`: `scenario_local_runtime`
