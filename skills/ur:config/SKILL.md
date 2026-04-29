---
name: ur:config
description: Reference for all ur.toml configuration options — top-level fields, [projects], [skills], [hostexec], [tui], [server], [db], networking, proxy, template paths, and convention-based file layout. Use when adding or modifying ur configuration, debugging config errors, or explaining what a field does.
---

# ur Configuration Reference

All user configuration lives in a single file: `$UR_CONFIG/ur.toml` (default `~/.ur/ur.toml`). Do **not** create separate config files — extend `ur.toml` instead.

Config is loaded by `Config::load()` in `crates/ur_config/src/lib.rs`. Missing file → error ("run 'ur init'"). Missing keys → field-specific defaults.

---

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `workspace` | path | `<config_dir>/workspace` | Host-side worker workspace directory |
| `server_port` | u16 | `12321` | TCP port for ur→server gRPC |
| `worker_port` | u16 | `server_port + 1` | TCP port for the shared worker gRPC server |
| `builderd_port` | u16 | `server_port + 2` | TCP port for builderd |
| `compose_file` | path | `<config_dir>/docker-compose.yml` | Docker Compose file path |
| `git_branch_prefix` | string | `""` | Prefix prepended to worker branch names (e.g. `"feature/"` → `feature/myproc-a1b2`) |
| `logs_dir` | path | `<config_dir>/logs` | Directory for all log files |

---

## `[proxy]` Section

Forward proxy (Squid) configuration controlling what external hosts containers may reach.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hostname` | string | `"ur-squid"` | Proxy hostname via Docker DNS |
| `allowlist` | string[] | `["api.anthropic.com", "platform.claude.com"]` | Allowed external domains |

```toml
[proxy]
hostname = "ur-squid"
allowlist = ["api.anthropic.com", "platform.claude.com", "registry.npmjs.org"]
```

---

## `[network]` Section

Docker network topology for containers.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"ur"` | Infrastructure network (internet-connected; server + squid) |
| `worker_name` | string | `"ur-workers"` | Worker network (internal, no direct internet) |
| `server_hostname` | string | `"ur-server"` | Server hostname via Docker DNS |
| `worker_prefix` | string | `"ur-worker-"` | Container name prefix for worker containers |

---

## `[server]` Section

Server runtime tuning. Most defaults are fine; adjust only if you know why.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `container_command` | string | `"docker"` | Container runtime (`"docker"`, `"nerdctl"`, etc.). Resolution: ur.toml → `UR_CONTAINER` env var → `"docker"` |
| `stale_worker_ttl_days` | u64 | `7` | Days before stale workers are cleaned up |
| `max_implement_cycles` | i32 \| null | `6` | Max workflow implement cycles before stalling. `null` = no limit |
| `poll_interval_ms` | u64 | `500` | Background polling loop interval (ms) |
| `github_scan_interval_secs` | u64 | `30` | GitHub poller scan interval (seconds) |
| `builderd_retry_count` | u32 | `3` | Max builderd gRPC retry attempts |
| `builderd_retry_backoff_ms` | u64 | `200` | Base backoff for builderd retries (exponential, ms) |
| `ui_event_fallback_interval_ms` | u64 | `5000` | LISTEN/NOTIFY timeout; also poll interval when LISTEN unavailable |

```toml
[server]
container_command = "docker"
max_implement_cycles = 10
github_scan_interval_secs = 60
```

---

## `[db]`, `[ticket_db]`, `[workflow_db]` Sections

Three separate Postgres connection configs. `[db]` is the legacy shared config; prefer `[ticket_db]` and `[workflow_db]` for new installations.

All three sections share the same fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `host` | string | `"ur-postgres"` | Database hostname |
| `port` | u16 | `5432` | Database port |
| `user` | string | `"ur"` | Database user |
| `password` | string | `"ur"` | Database password |
| `name` | string | `"ur"` / `"ur_tickets"` / `"ur_workflow"` | Database name (per section) |
| `bind_address` | string | — | Host interface to bind the postgres port on (e.g. a Tailscale IP) |

Password env var overrides: `UR_TICKET_DB_PASSWORD`, `UR_WORKFLOW_DB_PASSWORD`.

### `[*.backup]` Sub-section

Nested under any database section as `[db.backup]`, `[ticket_db.backup]`, or `[workflow_db.backup]`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | path | — | Directory for backup files. Omit to disable |
| `interval_minutes` | u64 | `30` | Minutes between automatic backups |
| `enabled` | bool | `true` | Toggle periodic backups (manual `ur db backup` still works) |
| `retain_count` | u64 | `3` | Number of backup files to keep (oldest deleted after each backup) |

```toml
[ticket_db]
name = "ur_tickets"

[ticket_db.backup]
path = "/var/backups/ur"
interval_minutes = 60
retain_count = 5
```

---

## `[skills]` Section

Inject host-side skills into worker containers at runtime. Skills are bind-mounted read-only into `/home/worker/.claude/potential-skills/<name>/` alongside skills baked into the container image.

Three sub-tables by strategy:

| Sub-table | Workers Affected |
|-----------|-----------------|
| `[skills.common]` | All workers |
| `[skills.code]` | `code`-strategy workers (in addition to common) |
| `[skills.design]` | `design`-strategy workers (in addition to common) |

Each key is the skill name; the value is a path to the skill directory on the host.

```toml
[skills.common]
my-skill = "%URCONFIG%/skills/my-skill"

[skills.code]
research-helper = "%URCONFIG%/skills/research-helper"

[skills.design]
internal-tool = "/opt/skills/internal-tool"
```

**Path rules:**
- `%URCONFIG%/...` — resolves to `<config_dir>/...`. **Preferred** — the config dir is already mounted into the server container.
- Absolute paths — must be visible to the **server process** (not just the host shell). Paths outside the server container's mount namespace produce empty mounts silently.

**Override semantics:** A host skill with the same name as a baked-in skill shadows the baked version. This lets you patch or replace a shipped skill without rebuilding the image.

**Merge order:** Mode-specific keys shadow same-named `common` keys.

---

## `[hostexec]` Section

Register custom host-exec commands and configure Lua transform scripts. Workers invoke these commands via the three-hop gRPC pipeline (worker shim → ur-server → builderd).

**Built-in defaults** (always available, no config needed): `git`, `gh`, `cargo`, `docker`, `ur`, `make`, `go`, `bazel`.

### `[hostexec.commands.<name>]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `lua` | string | — | Path to a Lua script relative to `$UR_CONFIG/hostexec/` |
| `default_script` | bool | `false` | Use the built-in default Lua script for this command |
| `long_lived` | bool | `false` | Process runs indefinitely (daemon mode) |
| `bidi` | bool | `false` | Use bidirectional streaming (requires `long_lived = true`) |

```toml
# Simple passthrough (no Lua transform)
[hostexec.commands.jq]

# Custom Lua transform
[hostexec.commands.git]
lua = "my-git.lua"          # reads from ~/.ur/hostexec/my-git.lua

# Restore built-in default script
[hostexec.commands.cargo]
default_script = true

# Long-running daemon with bidi streaming
[hostexec.commands.my-daemon]
long_lived = true
bidi = true
```

**Per-project access control:** Registering a command in `[hostexec]` adds it to the global registry but does **not** grant it to any project. Grant it to a project via `[projects.<key>].hostexec = ["jq"]`. Commands not in the registry are added as plain passthrough when granted.

**Lua transform signature:** `function transform(command, args, worker_id) ... end`. Return modified `args` table. Omitting a return allows the command through unchanged.

Lua scripts live in `$UR_CONFIG/hostexec/` (i.e., `~/.ur/hostexec/`).

---

## `[projects.<key>]` Section

Each project is a TOML table keyed by a short identifier (e.g., `[projects.ur]`).

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `repo` | string | — | **yes** | Git remote URL |
| `name` | string | `<key>` | no | Display label |
| `pool_limit` | u32 | `10` | no | Max cached repo clones in the pool |
| `hostexec` | string[] | `[]` | no | Additional host-exec commands workers may call for this project |
| `hostexec_scripts` | string[] | `[]` | no | Relative paths to host-exec scripts workers may invoke |
| `git_hooks_dir` | template path | — | no | Directory of git hook scripts |
| `skill_hooks_dir` | template path | — | no | Directory of skill hook snippets (copied to `~/.claude/skill-hooks/`) |
| `claude_md` | template path | — | no | Project-level CLAUDE.md. Falls back to `<config_dir>/projects/<key>/CLAUDE.md` |
| `workflow_hooks_dir` | template path | — | no | Workflow hook scripts (server-side, not container-mounted) |
| `max_fix_attempts` | u32 | `10` | no | Fix loop iterations before stalling the agent |
| `protected_branches` | string[] | `["main", "master"]` | no | Branch patterns that cannot be force-pushed (supports globs) |
| `ignored_workflow_checks` | string[] | `[]` | no | CI check names to skip when evaluating workflow status |

### `[projects.<key>.container]`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `image` | string | **yes** | Container image. Aliases: `"ur-worker"` → `ur-worker:latest`, `"ur-worker-rust"` → `ur-worker-rust:latest`. Use `"image:tag"` or `"registry/image:tag"` for custom images |
| `mounts` | string[] | no | Extra volume mounts: `"source:destination"`. Source supports `%URCONFIG%/...` or absolute paths (`%PROJECT%` **not allowed** here) |
| `ports` | string[] | no | Port mappings: `"host_port:container_port"` |

### `[projects.<key>.tui]`

| Field | Type | Description |
|-------|------|-------------|
| `theme` | string | Per-project theme override (overrides the global `[tui].theme`) |

```toml
[projects.ur]
repo = "https://github.com/org/ur.git"
pool_limit = 5
hostexec = ["jq", "rg"]
git_hooks_dir = "%PROJECT%/.git-hooks"
skill_hooks_dir = "%URCONFIG%/skill-hooks/ur"
claude_md = "%URCONFIG%/projects/ur/CLAUDE.md"
max_fix_attempts = 8
protected_branches = ["main", "master", "release/*"]
ignored_workflow_checks = ["flaky-integration-test"]

[projects.ur.container]
image = "ur-worker"
mounts = ["%URCONFIG%/shared-data:/var/data:ro"]
ports = ["8080:8080"]

[projects.ur.tui]
theme = "dark"
```

---

## Template Path System

Fields like `git_hooks_dir`, `skill_hooks_dir`, `claude_md`, and `workflow_hooks_dir` use template strings resolved at container launch time.

| Form | Example | Resolves To | Effect |
|------|---------|-------------|--------|
| `%PROJECT%/...` | `%PROJECT%/.git-hooks` | `ProjectRelative` | No extra mount; path accessible at `/workspace/<rel_path>` via existing workspace mount |
| `%URCONFIG%/...` | `%URCONFIG%/hooks/ur` | `HostPath` | Volume-mounted from `<config_dir>/hooks/ur` |
| `/absolute/path` | `/opt/hooks` | `HostPath` | Volume-mounted from that path |

Validation runs at config load time. Unrecognized `%VAR%` patterns cause an immediate error.

### Mount Destinations

| Config Field | Container Path | Env Var |
|---|---|---|
| `git_hooks_dir` | `/var/ur/git-hooks/` | `UR_GIT_HOOKS_DIR` |
| `skill_hooks_dir` | `/var/ur/skill-hooks/` | `UR_SKILL_HOOKS_DIR` |
| `claude_md` | `/var/ur/project-claude/CLAUDE.md` | `UR_PROJECT_CLAUDE` |
| `container.mounts` | user-specified destination | (none) |
| `workflow_hooks_dir` | (not container-mounted — used server-side) | — |

---

## Convention-Based File Layout

Several behaviors trigger automatically when files exist at expected paths under `<config_dir>/projects/<key>/` — no explicit `ur.toml` config needed.

| Convention Path | Effect |
|-----------------|--------|
| `~/.ur/projects/<key>/CLAUDE.md` | Auto-mounted as the project CLAUDE.md if `claude_md` is not set in ur.toml |
| `~/.ur/projects/<key>/local/` | Files here are recursively copied into pool slot workspaces at acquire time (mirrors workspace root; pool mode only) |
| `~/.ur/hostexec/<script.lua>` | Referenced by filename from `[hostexec.commands.<name>].lua` |

**Local files example** — to enable sccache for all pool workers without touching ur.toml:

```
~/.ur/projects/ur/local/.cargo/config.toml
```

Files are copied after clone/reset and before container launch. Removed on slot release (`git clean -fdx`).

---

## `[tui]` Section

TUI display settings.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `theme` | string | `"system"` | Active theme. Built-in themes: `abyss`, `acid`, `aqua`, `autumn`, `black`, `bumblebee`, `business`, `caramellatte`, `cmyk`, `coffee`, `corporate`, `cupcake`, `cyberpunk`, `dark`, `dim`, `dracula`, `emerald`, `fantasy`, `forest`, `garden`, `halloween`, `lemonade`, `light`, `lofi`, `luxury`, `night`, `nord`, `pastel`, `retro`, `silk`, `sunset`, `synthwave`, `valentine`, `winter`, `wireframe`. Or a name from `[tui.themes]` |
| `keymap` | string | `"default"` | Active keymap. `"default"` or a name from `[tui.keymaps]` |
| `key_repeat_interval_ms` | u64 | `200` | Min interval between repeated nav actions when holding a key (ms) |

### `[tui.ticket.filter]`

Persisted ticket panel filter defaults.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `statuses` | string[] | `["open", "in_progress"]` | Statuses to show |
| `projects` | string[] | all | Projects to show |

### `[tui.notifications]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `flow_stalled` | bool | `true` | Notify when a flow stalls |
| `flow_in_review` | bool | `true` | Notify when a flow enters review |

### `[tui.themes.<name>]`

Define a custom theme. All color values are strings (hex `"#1a1b26"` or named `"red"`).

| Color Key | Purpose |
|-----------|---------|
| `bg` / `fg` | Background / foreground |
| `border` / `border_focused` | Panel borders |
| `border_rounded` | bool — rounded border corners |
| `header_bg` / `header_fg` | Header bar |
| `selected_bg` / `selected_fg` | Selected row highlight |
| `status_bar_bg` / `status_bar_fg` | Bottom status bar |
| `error_fg` / `warning_fg` / `success_fg` / `info_fg` / `muted_fg` | Semantic text colors |
| `accent` / `highlight` / `shadow` / `overlay_bg` | Decorative colors |

```toml
[tui.themes.my-dark]
bg = "#1a1b26"
fg = "#c0caf5"
border = "#3b4261"
border_focused = "#7aa2f7"
selected_bg = "#364a82"
error_fg = "#f7768e"
```

### `[tui.keymaps.<name>]`

Override key bindings for any action. Each value is a list of key strings.

| Action | Description |
|--------|-------------|
| `quit` | Quit the TUI |
| `focus_next` / `focus_prev` | Move focus between panels |
| `scroll_up` / `scroll_down` / `page_up` / `page_down` | Scroll content |
| `select` / `cancel` | Confirm / dismiss |
| `refresh` | Reload data |
| `filter` | Open filter input |
| `help` | Show help |
| `new_flow` / `stop_flow` | Start or stop a workflow |
| `view_logs` | Open log viewer |
| `toggle_panel` | Toggle side panel |

```toml
[tui]
keymap = "vim"

[tui.keymaps.vim]
scroll_up = ["k"]
scroll_down = ["j"]
page_up = ["ctrl-u"]
page_down = ["ctrl-d"]
quit = ["q", "ctrl-c"]
```

---

## Full Annotated Example

```toml
workspace = "/Users/me/.ur/workspace"
server_port = 12321
git_branch_prefix = "agent/"
logs_dir = "/Users/me/.ur/logs"

[proxy]
allowlist = ["api.anthropic.com", "platform.claude.com", "crates.io"]

[server]
container_command = "docker"
max_implement_cycles = 8
github_scan_interval_secs = 45

[ticket_db.backup]
path = "/var/backups/ur/tickets"
interval_minutes = 60
retain_count = 5

[workflow_db.backup]
path = "/var/backups/ur/workflow"
interval_minutes = 60
retain_count = 5

[skills.common]
ur-ticket = "%URCONFIG%/skills/ur:ticket"
ur-config = "%URCONFIG%/skills/ur:config"

[skills.code]
implement = "%URCONFIG%/skills/implement"

[hostexec.commands.jq]
# passthrough — no Lua transform needed

[hostexec.commands.rg]
# passthrough

[projects.myrepo]
repo = "https://github.com/org/myrepo.git"
pool_limit = 8
hostexec = ["jq", "rg"]
git_hooks_dir = "%PROJECT%/.git-hooks"
claude_md = "%URCONFIG%/projects/myrepo/CLAUDE.md"
protected_branches = ["main", "release/*"]
ignored_workflow_checks = ["slow-e2e"]

[projects.myrepo.container]
image = "ur-worker"
mounts = ["%URCONFIG%/shared-certs:/etc/ssl/certs:ro"]

[tui]
theme = "dark"
keymap = "default"

[tui.ticket.filter]
statuses = ["open", "in_progress"]
projects = ["myrepo"]

[tui.notifications]
flow_stalled = true
flow_in_review = false
```

$ARGUMENTS
