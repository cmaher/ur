# Configuration Reference

All configuration lives in a single file: `$UR_CONFIG/ur.toml` (default `~/.ur/ur.toml`).

Run `ur init` to generate the file with sensible defaults. All fields are optional — missing keys use defaults.

## Top-Level Fields

```toml
workspace = "~/.ur/workspace"
server_port = 12321
worker_port = 12322
builderd_port = 12323
compose_file = "~/.ur/docker-compose.yml"
logs_dir = "~/.ur/logs"
git_branch_prefix = ""
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `workspace` | path | `<config_dir>/workspace` | Worker workspace directory (host-side) |
| `server_port` | u16 | `12321` | TCP port for ur→server gRPC |
| `worker_port` | u16 | `server_port + 1` | TCP port for shared worker gRPC server |
| `builderd_port` | u16 | `server_port + 2` | TCP port for builderd |
| `compose_file` | path | `<config_dir>/docker-compose.yml` | Docker Compose file path |
| `logs_dir` | path | `<config_dir>/logs` | Directory for log files |
| `git_branch_prefix` | string | `""` | Prefix prepended to worker-ID branch names |

## `[proxy]`

Forward proxy (Squid) configuration for restricting container network access.

```toml
[proxy]
hostname = "ur-squid"
allowlist = ["api.anthropic.com", "platform.claude.com"]
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hostname` | string | `"ur-squid"` | Proxy hostname on Docker network |
| `allowlist` | string[] | `["api.anthropic.com", "platform.claude.com"]` | Domains containers may reach |

## `[network]`

Docker network configuration for container networking.

```toml
[network]
name = "ur"
worker_name = "ur-workers"
server_hostname = "ur-server"
worker_prefix = "ur-worker-"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"ur"` | Infrastructure network name (internet-connected) |
| `worker_name` | string | `"ur-workers"` | Worker network name (internal, no internet) |
| `server_hostname` | string | `"ur-server"` | Server hostname via Docker DNS |
| `worker_prefix` | string | `"ur-worker-"` | Container name prefix for workers |

## `[server]`

Server runtime configuration.

```toml
[server]
container_command = "docker"
stale_worker_ttl_days = 7
max_implement_cycles = 6
poll_interval_ms = 500
github_scan_interval_secs = 30
builderd_retry_count = 3
builderd_retry_backoff_ms = 200
ui_event_poll_interval_ms = 200
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `container_command` | string | `"docker"` | Container runtime command. Resolution order: ur.toml > `UR_CONTAINER` env var > `"docker"` |
| `stale_worker_ttl_days` | u64 | `7` | Days before stale workers are cleaned up |
| `max_implement_cycles` | i32 | `6` | Max implement cycles before stalling workflow |
| `poll_interval_ms` | u64 | `500` | Background polling loop interval in ms |
| `github_scan_interval_secs` | u64 | `30` | GitHub poller scan interval in seconds |
| `builderd_retry_count` | u32 | `3` | Max builderd gRPC retry attempts |
| `builderd_retry_backoff_ms` | u64 | `200` | Base backoff in ms for builderd retries (exponential) |
| `ui_event_poll_interval_ms` | u64 | `200` | UI event poll interval in ms |

## `[backup]`

Periodic database backup configuration.

```toml
[backup]
path = "~/.ur/backups"
enabled = true
interval_minutes = 30
retain_count = 3
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | path | — | Directory to write backup files. Unset disables backups. |
| `enabled` | bool | `true` | Whether periodic backups are enabled |
| `interval_minutes` | u64 | `30` | Interval between backups in minutes |
| `retain_count` | u64 | `3` | Number of backup files to retain (older ones deleted) |

## `[hostexec]`

Host execution command configuration. Commands defined here allow workers to execute processes on the host via gRPC.

Built-in default commands (have default Lua scripts): `git`, `gh`, `cargo`, `docker`, `ur`.

```toml
[hostexec.commands.git]
default_script = true

[hostexec.commands.bacon]
lua = "bacon.lua"
long_lived = true
bidi = true
```

### `[hostexec.commands.<name>]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `lua` | string | — | Path to Lua script, relative to `$UR_CONFIG/hostexec/` |
| `default_script` | bool | `false` | Use built-in default Lua script for this command (if one exists) |
| `long_lived` | bool | `false` | Process runs indefinitely (daemon) |
| `bidi` | bool | `false` | Bidirectional streaming. Requires `long_lived = true` — config load fails if `bidi = true` and `long_lived = false` |

## `[tui]`

TUI display, theme, keymap, and notification settings.

```toml
[tui]
theme = "system"
keymap = "default"
key_repeat_interval_ms = 200
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `theme` | string | `"system"` | Active theme name (built-in or custom from `[tui.themes.<name>]`) |
| `keymap` | string | `"default"` | Active keymap name (built-in or custom from `[tui.keymaps.<name>]`) |
| `key_repeat_interval_ms` | u64 | `200` | Minimum interval in ms between repeated navigation actions when holding a key |

### Built-in Themes

`abyss`, `acid`, `aqua`, `autumn`, `black`, `bumblebee`, `business`, `caramellatte`, `cmyk`, `coffee`, `corporate`, `cupcake`, `cyberpunk`, `dark`, `dim`, `dracula`, `emerald`, `fantasy`, `forest`, `garden`, `halloween`, `lemonade`, `light`, `lofi`, `luxury`, `night`, `nord`, `pastel`, `retro`, `silk`, `sunset`, `synthwave`, `system`, `valentine`, `winter`, `wireframe`

### `[tui.themes.<name>]`

Define a custom theme. All fields are optional.

```toml
[tui.themes.mytheme]
bg = "#1a1a2e"
fg = "#e0e0e0"
border = "#333355"
border_focused = "#6666aa"
accent = "#ff6600"
```

| Field | Type | Description |
|-------|------|-------------|
| `bg` | string | Background color |
| `fg` | string | Foreground color |
| `border` | string | Border color |
| `border_focused` | string | Focused border color |
| `border_rounded` | bool | Use rounded border characters |
| `header_bg` | string | Header background |
| `header_fg` | string | Header foreground |
| `selected_bg` | string | Selected item background |
| `selected_fg` | string | Selected item foreground |
| `status_bar_bg` | string | Status bar background |
| `status_bar_fg` | string | Status bar foreground |
| `error_fg` | string | Error text color |
| `warning_fg` | string | Warning text color |
| `success_fg` | string | Success text color |
| `info_fg` | string | Info text color |
| `muted_fg` | string | Muted text color |
| `accent` | string | Accent color |
| `highlight` | string | Highlight color |
| `shadow` | string | Shadow color |
| `overlay_bg` | string | Overlay background |

### `[tui.keymaps.<name>]`

Define a custom keymap. All fields are optional; each accepts an array of key binding strings.

```toml
[tui.keymaps.vim]
quit = ["q", "Ctrl-c"]
scroll_up = ["k"]
scroll_down = ["j"]
```

| Field | Type | Description |
|-------|------|-------------|
| `quit` | string[] | Quit the application |
| `focus_next` | string[] | Focus next element |
| `focus_prev` | string[] | Focus previous element |
| `scroll_up` | string[] | Scroll up |
| `scroll_down` | string[] | Scroll down |
| `page_up` | string[] | Page up |
| `page_down` | string[] | Page down |
| `select` | string[] | Select item |
| `cancel` | string[] | Cancel action |
| `refresh` | string[] | Refresh view |
| `filter` | string[] | Open filter |
| `help` | string[] | Show help |
| `new_flow` | string[] | Create new flow |
| `stop_flow` | string[] | Stop flow |
| `view_logs` | string[] | View logs |
| `toggle_panel` | string[] | Toggle panel |

### `[tui.ticket.filter]`

Filter which tickets appear in the TUI ticket view.

```toml
[tui.ticket.filter]
statuses = ["open", "in_progress"]
projects = ["ur"]
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `statuses` | string[] | `["open", "in_progress"]` | Ticket statuses to display |
| `projects` | string[] | all | Project keys to display |

### `[tui.notifications]`

Control TUI notification events.

```toml
[tui.notifications]
flow_stalled = true
flow_in_review = true
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `flow_stalled` | bool | `true` | Notify when a flow stalls |
| `flow_in_review` | bool | `true` | Notify when a flow enters review |

## `[projects.<key>]`

Each project is a TOML table keyed by a short identifier (e.g., `[projects.ur]`).

```toml
[projects.ur]
repo = "https://github.com/cmaher/ur.git"
name = "Ur"
pool_limit = 10
hostexec = ["ur"]
git_hooks_dir = "%PROJECT%/ur-hooks/git"
skill_hooks_dir = "%URCONFIG%/skills/ur"
claude_md = "%PROJECT%/CLAUDE.md"
workflow_hooks_dir = "%URCONFIG%/hooks/ur/workflow"
max_fix_attempts = 10
protected_branches = ["main", "master"]
ignored_workflow_checks = []
```

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `repo` | string | — | yes | Git remote URL |
| `name` | string | `<key>` | no | Display-friendly label |
| `pool_limit` | u32 | `10` | no | Max cached repo clones in pool |
| `hostexec` | string[] | `[]` | no | Additional passthrough commands granted to this project |
| `git_hooks_dir` | string | — | no | Template path to directory of git hook scripts |
| `skill_hooks_dir` | string | — | no | Template path to directory of skill hook snippets |
| `claude_md` | string | — | no | Template path to project-level CLAUDE.md file |
| `workflow_hooks_dir` | string | — | no | Template path to directory of workflow hook scripts |
| `max_fix_attempts` | u32 | `10` | no | Max fix loop iterations before stalling agent |
| `protected_branches` | string[] | `["main", "master"]` | no | Branches that cannot be force-pushed (glob patterns supported) |
| `ignored_workflow_checks` | string[] | `[]` | no | CI check names to ignore when evaluating workflow status |

### `[projects.<key>.container]`

Container configuration for the project's workers.

```toml
[projects.ur.container]
image = "ur-worker-rust"
mounts = ["/Users/me/.ur/data:/data:ro"]
ports = ["8080:3000"]
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `image` | string | `"ur-worker"` | Container image name. Built-in aliases: `ur-worker`, `ur-worker-rust` (resolve to `:latest`). Custom images use full reference. |
| `mounts` | string[] | `[]` | Volume mounts (see format below) |
| `ports` | string[] | `[]` | Port mappings in `"host_port:container_port"` format |

#### Mount Format

Format: `"source:destination"` or `"source:destination:ro"`

- **Source** (host side): absolute path or `%URCONFIG%/...` for config-relative paths. `%PROJECT%` is not supported in mount sources.
- **Destination** (container side): must be an absolute path.
- **`:ro` suffix**: makes the mount read-only.

Examples:
```toml
mounts = [
  "/Users/me/projects/ur/.tickets:/workspace/.tickets",
  "%URCONFIG%/hooks/ur:/workspace/hooks:ro",
]
```

#### Port Mapping Format

Format: `"host_port:container_port"` — both must be valid u16 values (0–65535).

### `[projects.<key>.tui]`

Per-project TUI settings.

```toml
[projects.ur.tui]
theme = "nord"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `theme` | string | — | Per-project theme override (unset = use global `[tui].theme`) |

## Template Paths

Several project fields (`git_hooks_dir`, `skill_hooks_dir`, `claude_md`, `workflow_hooks_dir`) accept template path strings. Three forms are supported:

| Form | Example | Resolves To |
|------|---------|-------------|
| `%PROJECT%/...` | `%PROJECT%/.git-hooks` | Path relative to the project root inside the container (`/workspace/<rel>`) |
| `%URCONFIG%/...` | `%URCONFIG%/hooks/ur` | Absolute host-side path under the config directory |
| `/absolute/path` | `/opt/hooks` | Literal host-side path |

Template paths are validated at config load time. Unrecognized `%VAR%` patterns cause a descriptive error.

## Full Example

```toml
workspace = "/Users/me/.ur/workspace"
server_port = 12321
git_branch_prefix = "ur-"

[proxy]
hostname = "ur-squid"
allowlist = ["api.anthropic.com", "platform.claude.com", "registry.npmjs.org"]

[network]
name = "ur"
worker_name = "ur-workers"
server_hostname = "ur-server"
worker_prefix = "ur-worker-"

[server]
container_command = "docker"
stale_worker_ttl_days = 7
max_implement_cycles = 6

[backup]
path = "/Users/me/.ur/backups"
interval_minutes = 30
retain_count = 3

[hostexec.commands.git]
default_script = true

[hostexec.commands.gh]
default_script = true

[hostexec.commands.ur]
default_script = true

[tui]
theme = "nord"
keymap = "default"

[tui.notifications]
flow_stalled = true
flow_in_review = true

[tui.ticket.filter]
statuses = ["open", "in_progress"]
projects = ["ur"]

[projects.ur]
repo = "https://github.com/cmaher/ur.git"
pool_limit = 10
hostexec = ["ur"]
git_hooks_dir = "%PROJECT%/ur-hooks/git"
skill_hooks_dir = "%URCONFIG%/skills/ur"
claude_md = "%PROJECT%/CLAUDE.md"
protected_branches = ["main", "master"]

[projects.ur.container]
image = "ur-worker-rust"
mounts = ["/Users/me/projects/ur/.tickets:/workspace/.tickets"]
```
