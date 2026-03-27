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
git_hooks_dir = "%PROJECT%/.git-hooks"
skill_hooks_dir = "%URCONFIG%/skill-hooks/ur"
claude_md = "%URCONFIG%/projects/ur/CLAUDE.md"
workflow_hooks_dir = "%PROJECT%/.workflow"

[projects.ur.container]
image = "ur-worker:latest"
mounts = ["%URCONFIG%/shared-data:/var/data"]
```

Source: `crates/ur_config/src/lib.rs` — `ProjectConfig` struct (fields: `git_hooks_dir`, `skill_hooks_dir`, `claude_md`, `workflow_hooks_dir`) and `ContainerConfig` (field: `mounts`).

## Mount Destinations

| Config Field | Container Mount Point | Env Var | Read-only? |
|---|---|---|---|
| `git_hooks_dir` | `/var/ur/git-hooks/` | `UR_GIT_HOOKS_DIR` | no |
| `skill_hooks_dir` | `/var/ur/skill-hooks/` | `UR_SKILL_HOOKS_DIR` | no |
| `claude_md` | `/var/ur/project-claude/CLAUDE.md` | `UR_PROJECT_CLAUDE` | yes (`:ro`) |
| `container.mounts` | user-specified `destination` | (none) | no |
| `workflow_hooks_dir` | (not container-mounted — resolved server-side for builderd execution) | — | — |

When any of these resolve to `ProjectRelative`, no volume mount is created — only the env var is set, pointing to `/workspace/<rel_path>`.

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
  │   └─ Extracts: git_hooks_dir, skill_hooks_dir, claude_md, mounts, ports
  │
  ├─ Builds WorkerConfig                         [server/src/worker.rs]
  │
  ▼
WorkerManager::run_and_record()                  [server/src/worker.rs]
  │
  ├─ resolve_claude_md()     ← convention fallback for CLAUDE.md
  │
  ├─ RunOptsBuilder          [server/src/run_opts_builder.rs]
  │   ├─ .add_workspace()          → /workspace mount
  │   ├─ .add_credentials()        → shared OAuth credentials
  │   ├─ .add_git_hooks()          → resolve_template_path → mount or env var
  │   ├─ .add_skill_hooks()        → resolve_template_path → mount or env var
  │   ├─ .add_project_claude_md()  → resolve_template_path → mount or env var
  │   ├─ .add_mounts()             → resolve_template_path → mount for each entry
  │   ├─ .add_context_repos()      → /context/<key>:ro mounts
  │   └─ .build() → RunOpts
  │
  ▼
docker run with -v volumes + -e env vars
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

## Workflow Hooks (Server-Side, Not Container-Mounted)

`workflow_hooks_dir` is the exception — it is **not mounted into worker containers**. Instead, the server resolves it when running workflow verification steps (e.g., pre-push hooks) and executes hooks via builderd on the host.

Source: `crates/server/src/workflow/handlers/verify.rs:140`
