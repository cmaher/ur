# Unified Project Configuration

How `ur.toml` is parsed, validated, and consumed across the system.

## Config File

Location: `$UR_CONFIG/ur.toml` (default `~/.ur/ur.toml`). Single file — all user configuration lives here. Missing file causes an error with "run 'ur init'" message. Missing keys within the file use defaults.

Loaded by `Config::load()` / `Config::load_from()` in `crates/ur_config/src/lib.rs`.

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `agent` | string | `"claude"` | Default agent harness for every worker (`"claude"` or `"codex"`). An unrecognized value is a startup config error naming the valid agents — never a silent fallback. See [Agent Default Precedence](#agent-default-precedence) |
| `workspace` | path | `<config_dir>/workspace` | Worker workspace directory (host-side) |
| `server_port` | u16 | 12321 | TCP port for ur→server gRPC |
| `builderd_port` | u16 | `server_port + 2` | TCP port for builderd |
| `compose_file` | path | `<config_dir>/docker-compose.yml` | Docker Compose file path |
| `workspace_brain_dir` | template path | — | Brain directory mounted read-write at `/brain` for workers launched **without a project** (`-w` workspace mode). `%URCONFIG%/...` or absolute path; `%PROJECT%` rejected. No convention fallback — unset means no `/brain` for project-less workers. See [project-file-mounting.md](project-file-mounting.md) |

## `[skills]` Section

Host-side skills injected into worker containers at runtime. Skills are bind-mounted read-only into `/home/worker/.claude/potential-skills/<name>/` alongside skills baked into the container image.

Three sub-tables are supported:

| Sub-table | Scope |
|-----------|-------|
| `[skills.common]` | All modes |
| `[skills.code]` | `code`-strategy modes only |
| `[skills.design]` | `design`-strategy modes only |

Each key is the skill name as it will appear in `potential-skills/`; the value is the host path to the skill directory. Paths support `%URCONFIG%/...` (resolves to `<config_dir>/...`) or absolute paths.

**Visibility caveat**: paths must be accessible from the server process. Use `%URCONFIG%/...` to ensure the path is within the already-mounted config directory when the server runs inside a container.

Example:

```toml
[skills.common]
my-skill = "%URCONFIG%/skills/my-skill"

[skills.code]
research-helper = "%URCONFIG%/skills/research-helper"

[skills.design]
internal-tool = "/opt/skills/internal-tool"
```

Merge order: mode-specific (`code`/`design`) keys shadow same-named `common` keys. The merged set is processed by `WorkerManager::merge_global_skills()` and mounted via `RunOptsBuilder::add_extra_skills()`.

## `[proxy]` Section

Forward proxy (Squid) configuration for restricting container network access.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hostname` | string | `"ur-squid"` | Proxy hostname on Docker network |
| `allowlist` | string[] | `["api.anthropic.com", "platform.claude.com"]` | Domains containers may reach (GCS bucket for Claude Code dist is allowed via URL regex in squid.conf) |

## `[network]` Section

Docker network configuration for container networking.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"ur"` | Infrastructure network (internet-connected) |
| `worker_name` | string | `"ur-workers"` | Worker network (internal, no internet) |
| `server_hostname` | string | `"ur-server"` | Server hostname via Docker DNS |
| `worker_prefix` | string | `"ur-worker-"` | Container name prefix for workers |

## `[server]` Section

Server runtime configuration. Replaces hard-coded constants and the `UR_CONTAINER` env var with configurable values.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `container_command` | string | `"docker"` | Container runtime command. Resolution: ur.toml > `UR_CONTAINER` env var > `"docker"` |
| `stale_worker_ttl_days` | u64 | 7 | Days before stale workers are cleaned up |
| `max_transition_attempts` | i32 | 3 | Max lifecycle transition attempts before giving up |
| `poll_interval_ms` | u64 | 500 | Background polling loop interval (ms) |
| `github_scan_interval_secs` | u64 | 30 | GitHub poller scan interval (seconds) |

## `[projects.<key>]` Section

Each project is a TOML table keyed by a short identifier (e.g., `[projects.ur]`).

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `repo` | string | — | yes, unless `local = true` | Git remote URL |
| `local` | bool | `false` | no | Repo-less local project: no remote, no pool, no dispatch. Mutually exclusive with `repo`. See [Local Projects](#local-projects) |
| `name` | string | `<key>` | no | Display-friendly label |
| `pool_limit` | u32 | 10 | no | Max cached repo clones. Rejected when `local = true` |
| `hostexec` | string[] | `[]` | no | Additional passthrough commands for hostexec |
| `mounts` | string[] | `[]` | no | Volume mounts in `"source:destination"` format |
| `brain_dir` | template path | — | no | Per-project brain directory, mounted read-write at `/brain`. `%URCONFIG%/...` or absolute path; `%PROJECT%` rejected (brain must be project-stable). Convention fallback: `<config_dir>/projects/<key>/brain/` when unset. See [project-file-mounting.md](project-file-mounting.md) |

### Local Projects

`local = true` declares a project with **no git remote** — an arbitrary host
directory rather than a repo the workflow clones and pushes. `ProjectConfig.repo`
is `Option<String>`, and `None` *is* the locality signal: `ProjectConfig::is_local()`
reads it, and `require_repo()` produces the error for code paths that genuinely need
a remote. Making the field optional rather than adding a parallel boolean is
deliberate — the compiler forces every consumer of `repo` to state what it does when
there is none.

Validation lives in `resolve_project_repo` (`crates/ur_config/src/lib.rs`):

| Config | Outcome |
|---|---|
| `repo = "..."` | Normal pool-backed project |
| `local = true`, no `repo` | Local project, `repo: None` |
| Both | **Error** — mutually exclusive |
| Neither | **Error** — names both `repo` and `local = true` |
| `local = true` + `pool_limit` | **Error** — `pool_limit` implies a pool |

Workflow-only fields (`protected_branches`, `max_implement_cycles`,
`max_fix_attempts`, `push_again_exit_code`, `ignored_workflow_checks`) are accepted
and ignored for local projects, so a project can be flipped between local and
repo-backed by editing one line.

Locality is enforced at four independent layers, so no path can reach a pool
operation with a repo-less project:

| Layer | Guard |
|---|---|
| `ur` CLI | `reject_unsupported_local_launch` (`crates/ur/src/main.rs`) — pre-flight on `--dispatch`, non-manual modes, and missing `-w` |
| Server launch | `LaunchManager::reject_unsupported_local_launch` → `CoreError::LocalProjectUnsupportedLaunch` (`FailedPrecondition`). Backstop for the TUI and workerd |
| Pool | `RepoPoolManager::resolve_pool_project` — refuses before any DB query or builderd RPC, so no half-prepared slot dir is left behind |
| TUI | `Model::dispatch_is_blocked_by_locality` → banner instead of a `Cmd`; `dispatch_label` renders such tickets blocked (`□`) |

Only `-m manual` **with** `-w <dir>` is supported. Everything else about the project
— image, mounts, ports, `hostexec`, `hostexec_scripts`, `instruction_md`, `brain_dir`,
`memory_dir`, TUI theme — resolves identically to a pool-backed project, because
`-p <key> -w <dir>` already bypasses the pool while still applying project config.

### Mount Format

Mounts use `"source:destination"` format where:

- **Source** (host side): `%URCONFIG%/...` or absolute path. `%PROJECT%` is **not** supported — project-relative paths are already accessible via the workspace mount.
- **Destination** (container side): absolute path (must start with `/`).

Parsed at config load time into `MountConfig { source, destination }`. Source is resolved at use time via `resolve_template_path`.

Example: `"/Users/me/projects/ur/.tickets:/workspace/.tickets"` mounts a host directory into the container's workspace.

### Template Resolution Semantics

How each resolved variant maps to container behavior:

- **`ProjectRelative(rel_path)`**: The path exists inside the already-mounted workspace. No additional volume mount is created. The container path is `/workspace/<rel_path>`. Works for both `-w` workspace mode (user's checkout) and `-p` pool mode (pool slot's clone) — but only if the path exists in the repo checkout.

- **`HostPath(host_path)`**: A host-side directory is volume-mounted into the container at the specified destination. Used for files that live outside the project repo (e.g., `%URCONFIG%/...` or `/opt/...`). Used by `mounts` and `instruction_md`.

## Agent Default Precedence

Which agent a worker runs is resolved in this order, highest priority first:

```
--agent flag (ur worker launch)
  → worker_modes.<mode>.agent          (per-mode override, crates/server/src/worker.rs)
  → top-level `agent` key in ur.toml   (Config.agent / WorkerModesConfig's default_agent)
  → claude                             (built-in floor when the key is omitted)
```

`Config.agent` (`crates/ur_config/src/lib.rs`) and `WorkerModesConfig`'s `default_agent`
(`crates/server/src/worker.rs`) both come from the *same* top-level `agent` key, parsed
independently from the same `ur.toml` document by `ur_config::resolve_top_level_agent` —
`WorkerModesConfig::from_toml` already re-parses the whole file to pull out `[worker_modes]`
and `[worker_models]`, so reading one more top-level key needs no new plumbing.

`--agent` on `ur worker launch` (`crates/ur/src/main.rs`) sets `WorkerLaunchRequest.agent_type`
via `resolve_launch_agent_flag`; omitting it sends an empty string, which is exactly what the
server already treated as "resolve it yourself" via `resolve_mode`'s `requested_agent`
parameter — the CLI flag is real now, but the empty-means-unset contract it relies on isn't
new. `ur worker reseed-credentials`/`save-credentials --agent` follow the same top-level
default (via `resolve_agent_flag`) instead of a hardcoded `claude`, since the CLI process loads
`Config` directly and has no reason to fall back to a literal.

**Consequence worth flagging:** default models are agent-derived (`AgentType::default_model`),
so setting `agent = "codex"` globally does not just swap which binary runs — the built-in
`code` mode also resolves to `gpt-5.6-terra` instead of `sonnet`, because nothing in `code`'s
definition names a model directly. Use `[worker_models.<agent>]` (per-agent model overrides,
below) if a mode's model needs to stay agent-independent.

Deliberately out of scope: a per-project `[projects.<key>].agent` override. A project that
needs a specific harness declares a custom mode with an explicit `agent` field instead.

## `[worker_models]` Section

Overrides the built-in default model per worker strategy (`code`/`design`/`manual`). Two
shapes live under the same table:

```toml
[worker_models]            # applies to any agent
code = "sonnet"

[worker_models.codex]      # wins over the flat table, for codex modes only
code = "gpt-5.6-terra"
design = "gpt-5.6-sol"
```

`RawWorkerModels::from_value` (`crates/server/src/worker.rs`) classifies each key under
`[worker_models]` by its TOML value type — a table means a per-agent override
(`AgentType::parse`d from the key name), anything else joins the flat table — since a plain
`#[derive(Deserialize)]` struct can't express "some values are strings, some are tables" in one
shape. Both the flat table and every per-agent table use `deny_unknown_fields`, so a typo'd
strategy key errors in either; an unrecognized agent table name (e.g. `[worker_models.codexx]`)
errors naming the valid agents.

Model resolution precedence, most specific first:

```
custom mode's explicit `model`
  → [worker_models.<agent>].<strategy>
  → [worker_models].<strategy>
  → agent.default_model(strategy)
```

## Image Aliases (`container.image`)

`container.image` (`[projects.<key>.container]`) holds a logical alias (`"ur-worker"` or
`"ur-worker-rust"`) or a full image reference (containing `:` or `/`) — **exactly as written**.
Parse time (`validate_image_alias`, `crates/ur_config/src/lib.rs`) only checks that the value
is syntactically valid; it does not resolve an alias to a tag, because the agent that will run
the launch isn't known until `resolve_mode` runs at launch time (see [Agent Default
Precedence](#agent-default-precedence) above).

Resolution happens at the single point in the launch path where the agent is already settled —
`resolve_worker_image` in `crates/server/src/grpc.rs`, called from `LaunchManager::build_worker_config`
— via `AgentType::resolve_image`:

```
<alias>                     → <alias>-<agent-name>:latest        ("ur-worker" + codex → "ur-worker-codex:latest")
<full reference (":" or "/")>  → passthrough, unchanged
<unrecognized alias>        → error naming the value, the agent, and the valid aliases
```

`resolve_worker_image` picks the raw value first (`ur --image` request field, else the
project's `container.image`, else `AgentType::fallback_image()` — deliberately the
rust-toolchain alias, so a project configuring nothing doesn't silently lose the rust
toolchain), then resolves it against the launch's resolved agent. This is the same decision
point for both the project-configured image and the CLI override, so neither can end up
resolved against the wrong agent.

Because resolution is agent-derived, an alias can never disagree with the agent, and existing
`ur.toml` files and `--image` flags keep working untouched — `"ur-worker"` and
`"ur-worker-rust"` remain the only two alias names.

## Config Flow Through the System

```
ur.toml
  → Config::load() (crates/ur_config/src/lib.rs)
    → Config { projects: HashMap<String, ProjectConfig>, ... }

CLI launch request
  → grpc.rs: CoreServiceHandler::worker_launch()
    → reads ProjectConfig fields (instruction_md, mounts, hostexec, etc.)
    → resolve_mode(mode, requested_agent) resolves the agent
    → builds WorkerConfig struct (crates/server/src/worker.rs)

WorkerConfig
  → WorkerManager::run_and_record()
    → RunOptsBuilder (crates/server/src/run_opts_builder.rs)
      .add_workspace()             — mounts workspace_dir → /workspace
      .add_credentials(agent)      — mounts credentials file; no-op if agent.auth() is None
      .add_host_hooks_overlay()    — convention hook dirs → /var/ur/host-hooks/{git,skills}/:ro
      .add_project_instruction(agent) — instruction_md convention fallback → mount or env var
      .add_mounts()                — mounts project-configured volumes (source → destination)
      .add_env_vars()              — proxy vars, worker ID, server addr, skills, UR_AGENT_TYPE
      .build() → RunOpts → container runtime
```

### Fields Currently Wired

| ProjectConfig field | Passed to WorkerConfig | Consumed by RunOptsBuilder | Notes |
|--------------------|-----------------------|---------------------------|-------|
| `mounts` | yes | `add_mounts()` | Full pipeline: config → gRPC → WorkerConfig → RunOptsBuilder |
| `brain_dir` | yes | `add_brain_dir()` | Full pipeline: config → gRPC → WorkerConfig → `resolve_brain_dir()` → RunOptsBuilder; read-write mount at `/brain`, convention fallback. Project-less workers fall back to the top-level `workspace_brain_dir` (carried on `WorkerConfig.workspace_brain_dir`, set by `LaunchManager`) |
| `hostexec` | no (handled separately) | — | Used by HostExecServiceHandler for allowlist |
| (hook overlay) | via project_key + host_config_dir | `add_host_hooks_overlay()` | Convention paths; no config fields |

## Example Config

```toml
workspace = "/Users/me/.ur/workspace"
server_port = 12321

[proxy]
hostname = "ur-squid"
allowlist = ["api.anthropic.com", "platform.claude.com"]

[network]
name = "ur"
server_hostname = "ur-server"

[projects.ur]
repo = "https://github.com/cmaher/ur.git"
hostexec = ["ur"]
mounts = ["/Users/me/projects/ur/.tickets:/workspace/.tickets"]

[skills.common]
my-skill = "%URCONFIG%/skills/my-skill"

[skills.code]
research-helper = "%URCONFIG%/skills/research-helper"

[skills.design]
internal-tool = "/opt/skills/internal-tool"
```
