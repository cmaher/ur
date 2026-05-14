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
claude_md = "%URCONFIG%/projects/ur/CLAUDE.md"
memory_dir = "%URCONFIG%/projects/ur/memory"

[projects.ur.container]
image = "ur-worker:latest"
mounts = ["%URCONFIG%/shared-data:/var/data"]
```

Source: `crates/ur_config/src/lib.rs` — `ProjectConfig` struct (fields: `claude_md`, `memory_dir`) and `ContainerConfig` (field: `mounts`).

## Mount Destinations

| Config Field | Container Mount Point | Env Var | Read-only? |
|---|---|---|---|
| `claude_md` | `/var/ur/project-claude/CLAUDE.md` | `UR_PROJECT_CLAUDE` | yes (`:ro`) |
| `memory_dir` | `/home/worker/.claude/projects/-workspace/memory` | (none) | no |
| `container.mounts` | user-specified `destination` | (none) | no |
| host hooks overlay — git | `/var/ur/host-hooks/git/` | (none) | yes (`:ro`) |
| host hooks overlay — skills | `/var/ur/host-hooks/skills/` | (none) | yes (`:ro`) |
| workflow hooks (server-side) | (not container-mounted — resolved server-side) | — | — |

When `claude_md` resolves to `ProjectRelative`, no volume mount is created — only the env var is set, pointing to `/workspace/<rel_path>`.

## Hook Overlay Model

Git and skill hooks use a two-layer overlay with **no config fields**. Sources are fixed by convention; the host overlay wins on identical filenames.

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

The host overlay path `/var/ur/host-hooks/skills/` is volume-mounted from `<config_dir>/projects/<key>/hooks/skills/` on the host. Workerd copies both sources into `~/.claude/skill-hooks/` at container startup.

### Workflow Hooks (Server-Side, Not Container-Mounted)

Workflow hooks also use a two-layer overlay but are resolved and executed server-side via builderd. No container mount is involved.

| Layer | Host Path | Precedence |
|---|---|---|
| Host overlay | `<config_dir>/projects/<key>/hooks/workflow/pre-push` | checked first, wins |
| In-repo | `<slot_path>/ur-hooks/workflow/pre-push` | fallback |

Source: `crates/server/src/workflow/handlers/verify.rs`

## CLAUDE.md Convention Fallback

`claude_md` has a special convention-based fallback when not explicitly configured:

```
1. If claude_md is set in ur.toml → use it as-is (template resolution)
2. If claude_md is None → check <config_dir>/projects/<key>/CLAUDE.md on disk
3. If that file exists → use its absolute path (treated as HostPath)
4. If not → no CLAUDE.md mounted
```

This means placing a file at `~/.ur/projects/ur/CLAUDE.md` is enough — no config change needed.

Source: `resolve_claude_md()` in `crates/server/src/worker.rs:723`

## memory_dir Convention Fallback

`memory_dir` follows the same convention pattern as `claude_md`. When not set in `ur.toml`, the server checks for a directory at the convention path:

```
1. If memory_dir is set in ur.toml → use it as-is (template resolution)
2. If memory_dir is None → check <config_dir>/projects/<key>/memory/ on disk
3. If that directory exists → use its absolute path (treated as HostPath)
4. If not → no memory_dir mounted
```

This means creating `~/.ur/projects/ur/memory/` on the host is enough — no config change needed.

**No-project rule**: `memory_dir` is only mounted when a project key is associated with the worker. Workers launched without a project (`-w` workspace mode with no project config) never get a `memory_dir` mount.

**Auto-create and chown**: When `memory_dir` resolves to a host path, `add_memory_dir()` calls `create_dir_all` and `chown` to `WORKER_UID` before adding the volume mount. This ensures the non-root worker user can write to the directory on first use without manual host-side setup.

**Concurrent-write caveat**: Multiple parallel workers on the same project share a single `memory_dir` host path. If parallel workers (or the host Claude Code session) write to `MEMORY.md` simultaneously, updates can race and overwrite each other. This is a known limitation; no mitigation is in place.

Source: `resolve_memory_dir()` in `crates/server/src/worker.rs`, `add_memory_dir()` in `crates/server/src/run_opts_builder.rs`

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
  │   └─ Extracts: claude_md, mounts, ports
  │
  ├─ Builds WorkerConfig                         [server/src/worker.rs]
  │
  ▼
WorkerManager::run_and_record()                  [server/src/worker.rs]
  │
  ├─ resolve_claude_md()     ← convention fallback for CLAUDE.md
  │
  ├─ RunOptsBuilder          [server/src/run_opts_builder.rs]
  │   ├─ .add_workspace()              → /workspace mount
  │   ├─ .add_credentials()            → shared OAuth credentials
  │   ├─ .add_host_hooks_overlay()     → <config_dir>/projects/<key>/hooks/git/ → /var/ur/host-hooks/git/:ro
  │   │                                  <config_dir>/projects/<key>/hooks/skills/ → /var/ur/host-hooks/skills/:ro
  │   │                                  (each mount added only if the host dir exists)
  │   ├─ .add_project_claude_md()      → resolve_template_path → mount or env var
  │   ├─ .add_memory_dir()             → create_dir_all + chown → /home/worker/.claude/projects/-workspace/memory
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

### Copy Timing

The copy runs **after** clone/reset and **before** container launch:

1. `acquire_slot()` clones or resets the slot (via builderd)
2. `apply_local_files()` copies from `<config_dir>/projects/<key>/local/` into the slot
3. Worker branch checkout (`git checkout -b <worker_id>`)
4. Container launched with the slot as `/workspace`

On slot release, `git clean -fdx` (part of `reset_slot()`) removes the copied files, and they are re-copied on the next acquire.

### Error Behavior

- **Missing directory**: If `<config_dir>/projects/<key>/local/` does not exist or is empty, the copy step is a **no-op** (no error).
- **Copy failure**: If the directory exists but a copy operation fails (e.g., permission error, disk full), the error **propagates from `acquire_slot()`** and surfaces to the CLI as an acquire error. The slot is not handed out.

### Implementation

Server-side copy using `std::fs`. The server container has bind-mount access to both the config directory (same access used by `resolve_claude_md()` convention fallback) and pool slot directories (via the workspace bind mount). No builderd involvement needed.

Source: `apply_local_files()` in `crates/server/src/pool.rs`

## Full Flow (Updated)

The flow diagram in the [Full Flow](#full-flow) section above covers volume-mounted project files. The local project files step occurs in a different code path — inside `RepoPoolManager::acquire_slot()`:

```
RepoPoolManager::acquire_slot()                 [server/src/pool.rs]
  │
  ├─ clone_slot() or reset_slot()     ← git ops via builderd
  │
  ├─ apply_local_files()              ← NEW: convention-based copy
  │   │
  │   ├─ Reads <host_config_dir>/projects/<key>/local/
  │   │   └─ If missing or empty → no-op, return Ok
  │   │
  │   ├─ Recursively copies files into slot workspace
  │   │   └─ Overwrites existing files (local file wins)
  │   │
  │   └─ On failure → Err propagates, slot not acquired
  │
  ├─ checkout_branch()                ← worker-specific branch
  │
  ▼
Slot returned → container launch with /workspace mount
```

