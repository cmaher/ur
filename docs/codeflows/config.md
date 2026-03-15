# Unified Project Configuration

How `ur.toml` is parsed, validated, and consumed across the system.

## Config File

Location: `$UR_CONFIG/ur.toml` (default `~/.ur/ur.toml`). Single file — all user configuration lives here. Missing file causes an error with "run 'ur init'" message. Missing keys within the file use defaults.

Loaded by `Config::load()` / `Config::load_from()` in `crates/ur_config/src/lib.rs`.

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `workspace` | path | `<config_dir>/workspace` | Agent workspace directory (host-side) |
| `daemon_port` | u16 | 42069 | TCP port for ur→server gRPC |
| `builderd_port` | u16 | `daemon_port + 2` | TCP port for builderd |
| `compose_file` | path | `<config_dir>/docker-compose.yml` | Docker Compose file path |

## `[proxy]` Section

Forward proxy (Squid) configuration for restricting container network access.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hostname` | string | `"ur-squid"` | Proxy hostname on Docker network |
| `allowlist` | string[] | `["api.anthropic.com", "platform.claude.com", "raw.githubusercontent.com"]` | Domains containers may reach |

## `[network]` Section

Docker network configuration for container networking.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"ur"` | Infrastructure network (internet-connected) |
| `worker_name` | string | `"ur-workers"` | Worker network (internal, no internet) |
| `server_hostname` | string | `"ur-server"` | Server hostname via Docker DNS |
| `agent_prefix` | string | `"ur-agent-"` | Container name prefix for agents |

## `[projects.<key>]` Section

Each project is a TOML table keyed by a short identifier (e.g., `[projects.ur]`).

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `repo` | string | — | yes | Git remote URL |
| `name` | string | `<key>` | no | Display-friendly label |
| `pool_limit` | u32 | 10 | no | Max cached repo clones |
| `hostexec` | string[] | `[]` | no | Additional passthrough commands for hostexec |
| `git_hooks_dir` | string | — | no | Template path to git hook scripts directory |
| `mounts` | string[] | `[]` | no | Volume mounts in `"source:destination"` format |

### Template Path System

`git_hooks_dir` uses template path strings. Three forms are supported:

| Form | Example | Resolves To |
|------|---------|-------------|
| `%PROJECT%/...` | `%PROJECT%/.git-hooks` | `ProjectRelative(".git-hooks")` — path relative to project root inside container (`/workspace/<rel>`) |
| `%URCONFIG%/...` | `%URCONFIG%/hooks/ur` | `HostPath("<config_dir>/hooks/ur")` — absolute host-side path |
| `/absolute/path` | `/opt/hooks` | `HostPath("/opt/hooks")` — literal host-side path |

Validation happens at config load time (`validate_template_str`). Unrecognized `%VAR%` patterns cause config load to fail with a descriptive error including the project key and array index.

Resolution happens at use time (`resolve_template_path` in `crates/ur_config/src/template_path.rs`).

### Mount Format

Mounts use `"source:destination"` format where:

- **Source** (host side): `%URCONFIG%/...` or absolute path. `%PROJECT%` is **not** supported — project-relative paths are already accessible via the workspace mount.
- **Destination** (container side): absolute path (must start with `/`).

Parsed at config load time into `MountConfig { source, destination }`. Source is resolved at use time via `resolve_template_path`.

Example: `"/Users/me/projects/ur/.tickets:/workspace/.tickets"` mounts a host directory into the container's workspace.

### Template Resolution Semantics

How each resolved variant maps to container behavior:

- **`ProjectRelative(rel_path)`**: The path exists inside the already-mounted workspace. No additional volume mount is created. The container path is `/workspace/<rel_path>`. Works for both `-w` workspace mode (user's checkout) and `-p` pool mode (pool slot's clone) — but only if the path exists in the repo checkout. Used by `git_hooks_dir` only.

- **`HostPath(host_path)`**: A host-side directory is volume-mounted into the container at the specified destination. Used for files that live outside the project repo (e.g., `%URCONFIG%/hooks/ur` or `/opt/hooks`). Used by `git_hooks_dir` and `mounts`.

## Config Flow Through the System

```
ur.toml
  → Config::load() (crates/ur_config/src/lib.rs)
    → Config { projects: HashMap<String, ProjectConfig>, ... }

CLI launch request
  → grpc.rs: CoreServiceHandler::worker_launch()
    → reads ProjectConfig fields (git_hooks_dir, mounts, hostexec, etc.)
    → builds ProcessConfig struct (crates/server/src/process.rs)

ProcessConfig
  → ProcessManager::run_and_record()
    → RunOptsBuilder (crates/server/src/run_opts_builder.rs)
      .add_workspace()     — mounts workspace_dir → /workspace
      .add_credentials()   — mounts credentials file
      .add_git_hooks()     — mounts git hooks (HostPath only; ProjectRelative sets env var)
      .add_mounts()        — mounts project-configured volumes (source → destination)
      .add_env_vars()      — proxy vars, agent ID, server addr, skills
      .build() → RunOpts → container runtime
```

### Fields Currently Wired

| ProjectConfig field | Passed to ProcessConfig | Consumed by RunOptsBuilder | Notes |
|--------------------|-----------------------|---------------------------|-------|
| `git_hooks_dir` | yes | `add_git_hooks()` | Full pipeline: config → gRPC → ProcessConfig → RunOptsBuilder |
| `mounts` | yes | `add_mounts()` | Full pipeline: config → gRPC → ProcessConfig → RunOptsBuilder |
| `hostexec` | no (handled separately) | — | Used by HostExecServiceHandler for allowlist |

## Example Config

```toml
workspace = "/Users/me/.ur/workspace"
daemon_port = 42069

[proxy]
hostname = "ur-squid"
allowlist = ["api.anthropic.com", "platform.claude.com"]

[network]
name = "ur"
server_hostname = "ur-server"

[projects.ur]
repo = "https://github.com/cmaher/ur.git"
hostexec = ["ur"]
git_hooks_dir = "%PROJECT%/scripts/git-hooks"
mounts = ["/Users/me/projects/ur/.tickets:/workspace/.tickets"]
```
