# Project File Mounting

How project-specific files from `$URCONFIG/projects/<key>/` (and other template paths) are mounted into worker containers.

## Key Insight: Volume Mounts, Not Copies

Files are **never copied** into containers. They are Docker **volume-mounted** from the host at container launch time. Host-side files (`%URCONFIG%/...` or absolute paths) get a dedicated mount point. Project-relative files (`%PROJECT%/...`) are accessed through the existing workspace mount at `/workspace/`.

## Template Path System

All project file paths in `ur.toml` use a template string format, resolved at launch time.

| Template Form | Example | Resolves To |
|---|---|---|
| `%PROJECT%/...` | `%PROJECT%/.git-hooks` | `ProjectRelative(".git-hooks")` — no extra mount, accessed at `/workspace/.git-hooks` |
| `%URCONFIG%/...` | `%URCONFIG%/projects/ur/CLAUDE.md` | `HostPath("<config_dir>/projects/ur/CLAUDE.md")` — volume-mounted from host |
| `/absolute/path` | `/opt/hooks/ur` | `HostPath("/opt/hooks/ur")` — volume-mounted from host |

**Validation**: `validate_template_str()` runs at config load time (`Config::load()`), rejecting unrecognized `%VAR%` patterns immediately.

**Resolution**: `resolve_template_path()` runs at container launch time, producing a `ResolvedTemplatePath` enum:
- `ProjectRelative(rel_path)` — env var points to `/workspace/<rel_path>`, no mount added
- `HostPath(host_path)` — volume mount added from `host_path` to a well-known container path

Source: `crates/ur_config/src/template_path.rs`

## Project Config Fields That Use Template Paths

Configured in `ur.toml` under `[projects.<key>]`:

```toml
[projects.ur]
repo = "https://github.com/cmaher/ur.git"
instruction_md = "%URCONFIG%/projects/ur/CLAUDE.md"
memory_dir = "%URCONFIG%/projects/ur/memory"
brain_dir = "%URCONFIG%/projects/ur/brain"

[projects.ur.container]
image = "ur-worker"
mounts = ["%URCONFIG%/shared-data:/var/data"]
```

`instruction_md` was renamed from `claude_md`. The old key is still accepted via a deprecation shim — parsing `claude_md` populates `instruction_md` and emits a `tracing::warn!`; `instruction_md` wins if both are present.

Source: `crates/ur_config/src/lib.rs` — `ProjectConfig` struct (fields: `instruction_md`, `memory_dir`, `brain_dir`) and `ContainerConfig` (field: `mounts`).

## Mount Destinations

| Config Field | Container Mount Point | Env Var | Read-only? |
|---|---|---|---|
| `instruction_md` | `/var/ur/project-instruction/{agent.instruction_filename()}` (`CLAUDE.md` for Claude, `AGENTS.md` for Codex/AGY) | `UR_PROJECT_INSTRUCTION` | yes (`:ro`) |
| `memory_dir` | `~/{agent.home_subdir()}/{agent.memory_subdir()}` (`/home/worker/.claude/projects/-workspace/memory` for Claude); no-op if the agent has no memory-dir concept | (none) | no |
| `brain_dir` | `/brain` | (none) | no |
| `workspace_brain_dir` (top-level) | `/brain` | (none) | no |
| `container.mounts` | user-specified `destination` | (none) | no |
| host hooks overlay — git | `/var/ur/host-hooks/git/` | (none) | yes (`:ro`) |
| host hooks overlay — skills | `/var/ur/host-hooks/skills/` | (none) | yes (`:ro`) |
| host hooks overlay — startup | `/var/ur/host-hooks/startup/` | (none) | yes (`:ro`) |
| host hooks overlay — background startup | `/var/ur/host-hooks/startup-bg/` | (none) | yes (`:ro`) |
| workflow hooks (server-side) | (not container-mounted — resolved server-side) | — | — |

When `instruction_md` resolves to `ProjectRelative`, no volume mount is created — only the env var is set, pointing to `/workspace/<rel_path>`.

## Hook Overlay Model

Git, skill, and startup hooks use a two-layer overlay with **no config fields**. Sources are fixed by convention; the host overlay wins on identical filenames.

### Git Hooks

| Layer | Source (container-visible) | Precedence |
|---|---|---|
| In-repo | `/workspace/ur-hooks/git/` | applied first |
| Host overlay | `/var/ur/host-hooks/git/:ro` | applied second, wins on conflict |

The host overlay path `/var/ur/host-hooks/git/` is volume-mounted from `<config_dir>/projects/<key>/hooks/git/` on the host (via `add_host_hooks_overlay()` in `RunOptsBuilder`). The mount is added only if the host directory exists.

Workerd copies both sources into `/workspace/.git/hooks/` at container startup.

### Skill Hooks

| Layer | Source (container-visible) | Precedence |
|---|---|---|
| In-repo | `/workspace/ur-hooks/skills/` | applied first |
| Host overlay | `/var/ur/host-hooks/skills/:ro` | applied second, wins on conflict |

The host overlay path `/var/ur/host-hooks/skills/` is volume-mounted from `<config_dir>/projects/<key>/hooks/skills/` on the host. Workerd copies both sources into `~/{agent.customization_root()}/{agent.skill_hooks_subdir()}/` at container startup (`~/.claude/skill-hooks/` for Claude and `~/.gemini/config/skill-hooks/` for AGY), via `InitSkillHooksManager`. AGY's `home_subdir()` remains the separate runtime root `.gemini/antigravity-cli`; settings and mutable state belong there, while skills, hooks, and generated instructions use `.gemini/config`.

### Startup Hooks

Startup hooks execute in place after workerd initialization has created hostexec shims and before tmux or the agent starts. Only executable regular files participate; filenames are resolved in lexical order, with the host layer replacing an in-repo file of the same name.

| Kind | In-repo source | Host overlay | Behavior |
|---|---|---|---|
| Synchronous | `/workspace/ur-hooks/startup/` | `/var/ur/host-hooks/startup/:ro` | Run serially with cwd `/workspace`; the first non-zero exit aborts launch and its stderr is returned to the CLI |
| Background | `/workspace/ur-hooks/startup-bg/` | `/var/ur/host-hooks/startup-bg/:ro` | Spawn detached with cwd `/workspace`; workerd logs each name and PID |

The host sources are `<config_dir>/projects/<key>/hooks/startup/` and `<config_dir>/projects/<key>/hooks/startup-bg/`. As with other hook overlays, each mount is added only when its host directory exists.

### Workflow Hooks (Server-Side, Not Container-Mounted)

Workflow hooks also use a two-layer overlay but are resolved and executed server-side via builderd. No container mount is involved.

| Layer | Host Path | Precedence |
|---|---|---|
| Host overlay | `<config_dir>/projects/<key>/hooks/workflow/pre-push` | checked first, wins |
| In-repo | `<slot_path>/ur-hooks/workflow/pre-push` | fallback |

Source: `crates/server/src/workflow/handlers/verify.rs`

## Project Instruction File Convention Fallback

`instruction_md` has a special convention-based fallback when not explicitly configured:

```
1. If instruction_md is set in ur.toml → use it as-is (template resolution)
2. If instruction_md is None → check <config_dir>/projects/<key>/{agent.instruction_filename()} on disk
3. If that file exists → use its absolute path (treated as HostPath)
4. If not → no instruction file mounted
```

This means placing a file at `~/.ur/projects/ur/CLAUDE.md` is enough for Claude; the convention filename is agent-derived, so Codex and AGY use `~/.ur/projects/ur/AGENTS.md` instead. AGY does not discover a workspace `CLAUDE.md`, so workerd folds the configured project instruction content into its generated `~/.gemini/config/AGENTS.md`.

**Codex never needs its own `AGENTS.md`:** the codex image bakes `project_doc_fallback_filenames = ["CLAUDE.md"]` into `~/.codex/config.toml` (`containers/worker-codex/codex-config.toml`), so codex reads a repo's existing `CLAUDE.md` when no `AGENTS.md` is present — no ur-managed repo has to carry both files just to support both agents.

Source: `resolve_project_instruction()` in `crates/server/src/worker.rs`

## memory_dir Convention Fallback

`memory_dir` follows the same convention pattern as `instruction_md`. When not set in `ur.toml`, the server checks for a directory at the convention path:

```
1. If memory_dir is set in ur.toml → use it as-is (template resolution)
2. If memory_dir is None → check <config_dir>/projects/<key>/memory/ on disk
3. If that directory exists → use its absolute path (treated as HostPath)
4. If not → no memory_dir mounted
```

This means creating `~/.ur/projects/ur/memory/` on the host is enough — no config change needed.

**No-project rule**: `memory_dir` is only mounted when a project key is associated with the worker. Workers launched without a project (`-w` workspace mode with no project config) never get a `memory_dir` mount.

**Auto-create and chown**: When `memory_dir` resolves to a host path, `add_memory_dir()` calls `create_dir_all` and `chown` to `WORKER_UID` before adding the volume mount (skipped entirely if `agent.memory_subdir()` is `None`). This ensures the non-root worker user can write to the directory on first use without manual host-side setup.

**Concurrent-write caveat**: Multiple parallel workers on the same project share a single `memory_dir` host path. If parallel workers (or the host Claude Code session) write to `MEMORY.md` simultaneously, updates can race and overwrite each other. This is a known limitation; no mitigation is in place.

Source: `resolve_memory_dir()` in `crates/server/src/worker.rs`, `add_memory_dir()` in `crates/server/src/run_opts_builder.rs`

## brain_dir Convention Fallback

`brain_dir` is a per-project, user-managed context directory mounted **read-write** at `/brain`. It follows the same convention pattern as `memory_dir`. When not set in `ur.toml`, the server checks for a directory at the convention path:

```
1. If brain_dir is set in ur.toml → use it as-is (template resolution)
2. If brain_dir is None → check <config_dir>/projects/<key>/brain/ on disk
3. If that directory exists → use its absolute path (treated as HostPath)
4. If not → no brain_dir mounted
```

This means creating `~/.ur/projects/ur/brain/` on the host is enough — no config change needed. Unlike `memory_dir`, the mount is **read-write** because the `/brain:init` skill writes the scaffold and `/brain/working/` is worker scratch. The server also sets `UR_BRAIN=/brain`, and records the resolved host path on the worker row (`worker.brain_path`) so hostexec can map `/brain` back to the host for `git`.

**Brain skills** (`brain`, `brain:init`, `brain:dream`, `brain:dream-init`, one copy each in `containers/worker-base/potential-skills/`) resolve the brain root as `$UR_BRAIN`, falling back on the host to the `brain` field of `ur project list --output json` (`ProjectConfig::brain_path`: `brain_dir` resolved, else the convention path), so host and workers agree on projects that override `brain_dir`. `brain:init` makes the brain its own git repository; `brain:dream` consolidates it (lint, reconcile-then-promote finished streams, merge, reindex, scheduled review of durable docs) and keeps state and reports in `<brain>/.dream/`, with git history as the checkpoint. Its mechanical work is the `ur dream` CLI, which reads the TOML config block in `<brain>/DREAM.md` and writes only `.dream/state.json` (ref checks, review log with adaptive intervals). Workers run brain git through hostexec with a `/brain` working directory, which maps to the worker's host brain path for `git` only (see [host-exec-flow.md](host-exec-flow.md)). `brain:dream` and `brain:dream-init` are installed for the Manual strategy only. On the host, install them by symlinking each skill directory into `~/.claude/skills/` under the same bare name (no `ur:` prefix) so cross-references resolve. Everything project-specific lives in `<brain>/DREAM.md`; the skills carry no project content and their examples are placeholders only.

**Template restriction**: like `memory_dir`, `%PROJECT%` is rejected for `brain_dir` at config validation time — the brain must be project-stable, not workspace-relative. Only `%URCONFIG%/...` and absolute paths are valid.

**No-project rule**: the per-project `brain_dir` and its convention fallback only apply when a project key is associated with the worker. Workers launched without a project fall back to the top-level `workspace_brain_dir` instead (see below).

**Auto-create and chown**: When `brain_dir` resolves to a host path, `add_brain_dir()` calls `create_dir_all` and `chown` to `WORKER_UID` before adding the volume mount, so the non-root worker user can write to the directory on first use. The create/chown target the *container-visible* path (`convention_check_path`) while the mount source stays the host path — inside the server container the host path is not reachable, so creating it there would leave a stray directory in the server's filesystem and let Docker auto-create the real host dir as root.

Source: `resolve_brain_dir()` in `crates/server/src/worker.rs`, `add_brain_dir()` in `crates/server/src/run_opts_builder.rs`

## workspace_brain_dir (Project-Less Workers)

`workspace_brain_dir` is a **top-level** `ur.toml` key (not per-project) holding the brain for workers that have no project — i.e. `ur worker launch -w <path>` where no project key is derived. It mounts read-write at the same `/brain` path, so the `brain` and `brain:init` skills (which are in `common_skills()` for every strategy) work identically in workspace mode.

```toml
workspace_brain_dir = "%URCONFIG%/brain"
```

Full resolution order in `resolve_brain_dir()`:

```
1. projects.<key>.brain_dir set          → use it
2. project_key empty (-w, no project)    → workspace_brain_dir, or None when unset
3. <config_dir>/projects/<key>/brain/    → use it if it exists on disk
4. None                                  → no /brain mount
```

Consequences of that ordering:

- A project's own brain always wins; the workspace brain never overrides it.
- A project worker whose brain is unconfigured gets **no** brain — it does not fall through to the workspace brain. The workspace default is for project-less workers only.
- `-w <path> -p <project>` is a project worker (`project_key` is non-empty), so it uses the project rules.
- There is **no convention fallback** for `workspace_brain_dir`: it must be set explicitly, so upgrading never starts mounting a brain nobody configured. The configured directory itself is auto-created (and chowned) on first launch.

**Template restriction**: `%PROJECT%` is rejected at config load time — a workspace-mode worker has no project to resolve against. Only `%URCONFIG%/...` and absolute paths are valid.

**Plumbing**: `LaunchManager.workspace_brain_dir` (from `Config::workspace_brain_dir`) is copied onto `WorkerConfig.workspace_brain_dir` in `build_worker_config()`, and consumed by `resolve_brain_dir()` — the single place the decision is made.

Source: `Config::workspace_brain_dir` in `crates/ur_config/src/lib.rs`, `LaunchManager` in `crates/server/src/grpc.rs`, `resolve_brain_dir()` in `crates/server/src/worker.rs`

## Container Mounts

`container.mounts` uses `"source:destination"` format with a restriction: `%PROJECT%` is **not allowed** as a mount source. Project-relative paths are already accessible through the workspace mount, so an explicit mount would be redundant. Only `%URCONFIG%/...` and absolute paths are valid sources.

Source: `parse_mount_entry()` in `crates/ur_config/src/lib.rs`

## Full Flow

```
ur.toml
  │
  ├─ Config::load()                          [ur_config/src/lib.rs]
  │   ├─ validate_template_str() on each template field
  │   └─ parse_mount_entry() for container.mounts
  │
  ▼
CLI: ur worker launch <ticket> -p <project>
  │
  ├─ gRPC → WorkerLaunchRequest
  │
  ▼
ur-server: CoreServiceHandler::worker_launch()   [server/src/grpc.rs]
  │
  ├─ Reads ProjectConfig from projects HashMap
  │   └─ Extracts: instruction_md, mounts, ports
  │
  ├─ Resolves agent via resolve_mode(mode, requested_agent)   [server/src/worker.rs]
  │
  ├─ Builds WorkerConfig                         [server/src/worker.rs]
  │   └─ Copies LaunchManager.workspace_brain_dir onto WorkerConfig
  │
  ▼
WorkerManager::run_and_record()                  [server/src/worker.rs]
  │
  ├─ resolve_project_instruction()     ← convention fallback, agent.instruction_filename()
  │
  ├─ RunOptsBuilder          [server/src/run_opts_builder.rs]
  │   ├─ .add_workspace()              → /workspace mount
  │   ├─ .add_credentials(agent)       → shared OAuth credentials; no-op if agent.auth() is None
  │   ├─ .add_host_hooks_overlay()     → <config_dir>/projects/<key>/hooks/git/ → /var/ur/host-hooks/git/:ro
  │   │                                  <config_dir>/projects/<key>/hooks/skills/ → /var/ur/host-hooks/skills/:ro
  │   │                                  <config_dir>/projects/<key>/hooks/startup/ → /var/ur/host-hooks/startup/:ro
  │   │                                  <config_dir>/projects/<key>/hooks/startup-bg/ → /var/ur/host-hooks/startup-bg/:ro
  │   │                                  (each mount added only if the host dir exists)
  │   ├─ .add_project_instruction(agent) → resolve_template_path → mount or env var
  │   ├─ .add_memory_dir(agent)         → create_dir_all + chown → ~/{agent.home_subdir()}/{agent.memory_subdir()}
  │   │                                  (no-op if the agent has no memory-dir concept)
  │   ├─ .add_brain_dir()              → create_dir_all + chown → /brain (read-write)
  │   │                                  (project brain, or workspace_brain_dir when
  │   │                                   the worker has no project)
  │   ├─ .add_mounts()                 → resolve_template_path → mount for each entry
  │   ├─ .add_context_repos()          → /context/<key>:ro mounts
  │   └─ .build() → RunOpts
  │
  ▼
LaunchWorker RPC → builderd (host, native)
  │
  ├─ Validates each volume source path against the host filesystem
  │   (host-namespace check — avoids false-negative from server container paths)
  │
  └─ docker run with -v volumes + -e env vars
```

## Local Project Files (Pool Mode Only)

Convention-based file overlay that copies host-side files into pool slots at acquire time. This is **pool mode only** — workspace mode (`-w`) is unaffected.

### Convention Path

```
<config_dir>/projects/<key>/local/
```

The directory tree mirrors the workspace root. Files are recursively copied into the slot's workspace directory, preserving structure:

```
~/.ur/projects/ur/local/
  .cargo/
    config.toml      → copied to <slot>/.cargo/config.toml
  .env.local          → copied to <slot>/.env.local
```

### Example: sccache Configuration

To enable sccache for all pool workers on a project, place a Cargo config at the convention path:

```
~/.ur/projects/ur/local/.cargo/config.toml
```

With contents:

```toml
[build]
rustc-wrapper = "/usr/bin/sccache"
```

Every pool slot acquired for the `ur` project will have `.cargo/config.toml` copied into its workspace root, enabling sccache for all Cargo builds without any `ur.toml` configuration.

### Overriding Tracked Files (e.g., CLAUDE.md)

The same `local/` overlay mechanism works for **tracked** files that already exist in the repository (e.g., `CLAUDE.md`, `.cargo/config.toml` if it is committed). After copying, `LocalOverlayManager::apply()` runs:

```
git update-index --skip-worktree <file>
```

for every file that was placed on top of a tracked path. `--skip-worktree` tells git to ignore the working-tree content of that file: the index records the original repo version while the working tree holds the overridden content.

**Symlink handling:** If the destination path in the slot is a symlink (e.g., `CLAUDE.md → AGENTS.md`), the symlink is removed and replaced with a regular file containing the overlay content. The symlink target path is also pinned with `skip-worktree` so that a subsequent `git checkout` does not restore the target file and break the clean working tree.

**Practical effect inside a worker:**

- `git status` is clean — the override is invisible.
- `git add .`, `git commit -a`, and `git push` cannot include the overridden content.
- `git checkout`, `switch`, `rebase`, `merge`, `pull` and `stash` are never blocked by an override, even when the target commit changes the overridden path (see [Hiding Overlays from Git](#hiding-overlays-from-git)).
- Workers operate as if the file came from the repo, but Claude Code reads the host-supplied version.

### Hiding Overlays from Git

`skip-worktree` alone is not enough: git refuses to rewrite a skip-worktree path, so `git checkout <branch>` fails with "local changes would be overwritten" whenever the branch has a different version of an overridden file, and untracked overlays (e.g. `.cargo/config.toml`) still show up in `git status`. builderd therefore brackets every worker git command run in an overlaid pool slot so git never sees the overlays at all.

`LocalOverlayManager` (`crates/builderd/src/overlay.rs`) keeps the slot's overlay state under `<slot>/.git/ur-overlay/` (outside the working tree, untouched by `git clean`):

- `files/` — a copy of every overlay file at its slot-relative path
- `pinned` — the tracked paths pinned with `skip-worktree` on the last reveal (including symlink targets)

When builderd's `Exec` runs a short-lived `git` or `gh` command (`OVERLAY_BRACKETED_COMMANDS`) whose working dir is inside `<workspace>/pool/<project>/<slot>/` and that slot has overlay state:

1. **Conceal** (first concurrent command in the slot only — a per-slot counter makes overlapping commands share one conceal):
   a. Copy each overlay's current working-tree content back into `files/`, so worker edits to an overlay persist
   b. `git update-index --no-skip-worktree` on the `pinned` paths
   c. Remove untracked overlays; `git checkout -- <tracked overlays>` restores the index version (and restores symlinks the overlay replaced)
2. Spawn the command; git sees a working tree identical to the repo
3. **Reveal** (last concurrent command only) before the exit-code frame is forwarded: copy `files/` back into the working tree and re-pin tracked paths with `skip-worktree`

A conceal failure fails the command with `FailedPrecondition` (after a best-effort reveal); a reveal failure replaces the exit-code frame with an `Internal` status, so both surface to the worker. builderd keeps draining the child's output after a caller disconnects, so overlays come back only once the command has actually exited.

Consequence: anything the git command runs in the slot — including pre-commit and pre-push hooks — sees the repo version of overridden files, not the override. `skip-worktree` is kept for git invoked outside the bracket (e.g. by hand on the host in a slot).

**CLAUDE.md example:**

Drop a project-specific CLAUDE.md at:

```
~/.ur/projects/<key>/local/CLAUDE.md
```

Workers on that project will read your local CLAUDE.md instead of the repo's version, with no `ur.toml` change required and no risk of accidentally committing the override.

**Reverting:** Removing the file from `local/` reverts the slot's working copy to repo HEAD on the next recycle. `reset_slot()` clears all `skip-worktree` bits and discards `.git/ur-overlay/` before resetting, so `git reset --hard` and `git clean -fdx` operate normally and a slot cleaned by `CleanSlot` gets no overlays back.

**Pool mode only.** This mechanism is exclusive to pool slots. Workers launched with `-w` (workspace mode) mount the caller's working directory directly and do not use the `local/` overlay.

### Copy Timing

The copy runs **after** clone/reset and **before** container launch:

1. `acquire_slot()` clones or resets the slot (via builderd)
2. `LocalOverlayManager::apply()` runs against `<config_dir>/projects/<key>/local/`:
   a. Replace the slot's overlay state: copy `local/` into `<slot>/.git/ur-overlay/files/`
   b. Reveal: if a destination is a symlink, record the symlink target and remove the symlink
   c. Copy each overlay file into the slot workspace (overwrites existing content)
   d. If the file (or former symlink target) is tracked by git, run `git update-index --skip-worktree <file>` and record it in `.git/ur-overlay/pinned`
3. Worker branch checkout (`git checkout -b <worker_id>`)
4. Container launched with the slot as `/workspace`

On slot release, `reset_slot()` first clears all `skip-worktree` bits (`git update-index --no-skip-worktree`) and discards `.git/ur-overlay/` so that `git reset --hard origin/master` and `git clean -fdx` can restore every file to repo HEAD. Local files are re-copied on the next acquire.

### Error Behavior

- **Missing directory**: If `<config_dir>/projects/<key>/local/` does not exist or is empty, the copy step is a **no-op** (no error).
- **Copy failure**: If the directory exists but a copy operation fails (e.g., permission error, disk full), the error **propagates from `LocalOverlayManager::apply()`** and surfaces to the CLI as an acquire error. The slot is not handed out.
- **`git update-index` failure**: If `git update-index --skip-worktree` fails for a file (e.g., the git index is corrupt, or the file path is malformed), the error propagates from `LocalOverlayManager::apply()` and the slot is not acquired. This prevents a worker from launching in a state where the override would be accidentally staged or committed.

### Implementation

Builderd-side copy using `std::fs` and `git update-index`. Builderd runs natively on the host with direct access to both the config directory and pool slot directories, making it the right place for filesystem and git operations on pool slots.

Source: `LocalOverlayManager` in `crates/builderd/src/overlay.rs` (called from `crates/builderd/src/pool_handler.rs`; git bracketing from `crates/builderd/src/handler.rs`)

## Full Flow (Updated)

The flow diagram in the [Full Flow](#full-flow) section above covers volume-mounted project files. The local project files step occurs in a different code path — inside `BuilderPoolHandler` (builderd) via `RecycleSlot`/`PrepareNewSlot` RPCs:

```
BuilderPoolHandler::recycle_slot() / prepare_new_slot()   [builderd/src/pool_handler.rs]
  │
  ├─ clone_slot() or reset_slot()     ← git ops, native filesystem access
  │   └─ reset_slot() first clears skip-worktree bits
  │       (git update-index --no-skip-worktree on all tracked local-overlay paths)
  │       so reset --hard and clean -fdx restore repo HEAD cleanly
  │
  ├─ LocalOverlayManager::apply()     ← convention-based copy + skip-worktree
  │   │
  │   ├─ Reads <config_dir>/projects/<key>/local/
  │   │   └─ If missing or empty → no-op, return Ok
  │   │
  │   ├─ Copies it to <slot>/.git/ur-overlay/files/ (used by the git bracket)
  │   │
  │   ├─ Recursively copies files into slot workspace
  │   │   ├─ If destination is a symlink → record target, remove symlink
  │   │   └─ Copy overlay file (local file wins)
  │   │
  │   ├─ For each copied file (and former symlink targets) tracked by git:
  │   │   └─ git update-index --skip-worktree <file>  (recorded in .git/ur-overlay/pinned)
  │   │       (override invisible to git status, add, commit, push)
  │   │
  │   └─ On copy or git update-index failure → Err propagates, slot not acquired
  │
  ├─ checkout_branch()                ← worker-specific branch (separate RPC)
  │
  ▼
Slot returned to server → container launch with /workspace mount

Worker `git`/`gh` via hostexec → builderd Exec            [builderd/src/handler.rs]
  ├─ conceal_for(working_dir)   ← restore repo versions, remove untracked overlays
  ├─ run command                ← git sees a clean repo working tree
  └─ reveal_after(lease)        ← overlays back + re-pinned, then exit code forwarded
```
