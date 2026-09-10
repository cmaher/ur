---
name: ur:config
description: Reference for all ur.toml configuration options — top-level fields, [projects], [skills], [worker_modes], [worker_models], [hostexec], [tui], [server], [db], networking, proxy, template paths, and convention-based file layout. Use when adding or modifying ur configuration, debugging config errors, or explaining what a field does.
---

# ur Configuration Reference

All user configuration lives in a single file: `$UR_CONFIG/ur.toml` (default `~/.ur/ur.toml`). Do **not** create separate config files — extend `ur.toml` instead.

Config is loaded by `Config::load()` in `crates/ur_config/src/lib.rs`. Missing file → error ("run 'ur init'"). Missing keys → field-specific defaults.

---

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `agent` | string | `"claude"` | Default agent harness for every worker (`"claude"`, `"codex"`, or `"agy"`), when not overridden by `--agent` or `worker_modes.<mode>.agent`. See "Agent precedence" under the `[worker_models]` section below. An unrecognized value is a startup config error naming the valid agents |
| `workspace` | path | `<config_dir>/workspace` | Host-side worker workspace directory |
| `server_port` | u16 | `12321` | TCP port for ur→server gRPC |
| `worker_port` | u16 | `server_port + 1` | TCP port for the shared worker gRPC server |
| `builderd_port` | u16 | `server_port + 2` | TCP port for builderd |
| `compose_file` | path | `<config_dir>/docker-compose.yml` | Docker Compose file path |
| `git_branch_prefix` | string | `""` | Prefix prepended to worker branch names (e.g. `"feature/"` → `feature/myproc-a1b2`) |
| `logs_dir` | path | `<config_dir>/logs` | Directory for all log files |
| `workspace_brain_dir` | template path | — | Brain mounted read-write at `/brain` for workers with **no project** (bare `-w`). `%URCONFIG%/...` or absolute; `%PROJECT%` rejected. No convention fallback — unset means those workers get no `/brain`. A project's own `brain_dir` always wins; a project worker without one does **not** fall through to this |

---

## `[proxy]` Section

Forward proxy (Squid) configuration controlling what external hosts containers may reach.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hostname` | string | `"ur-squid"` | Proxy hostname via Docker DNS |
| `allowlist` | string[] | all supported-agent domains | Allowed external domains, assembled from `AgentType::ALL` (Claude, Codex, and AGY API/OAuth requirements) |

```toml
[proxy]
hostname = "ur-squid"
allowlist = [
  "api.anthropic.com", "platform.claude.com", "downloads.claude.ai",
  "chatgpt.com", "api.openai.com", "auth.openai.com",
  "oauth2.googleapis.com", "www.googleapis.com", "cloudcode-pa.googleapis.com",
  "daily-cloudcode-pa.googleapis.com", "lh3.googleusercontent.com",
  "accounts.google.com", "registry.npmjs.org",
]
```

Setting `allowlist` replaces the assembled default; it does not merge. An AGY-capable custom
list must retain all six Google hosts shown above. `lh3.googleusercontent.com` looks optional
but is a hard AGY startup dependency because the eligibility check fetches the profile image.

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

Three separate Postgres connection configs. `[db]` — and the top-level `[backup]` table, which
resolves into `[db].backup` — is legacy: it configures neither live pool. It is read only by the
`ur db backup` / `ur db restore` CLI, which dumps `db.name` (`"ur"` by default, not `ur_tickets`
or `ur_workflow`). **Periodic backups come from `[ticket_db.backup]` and `[workflow_db.backup]`
only**, so a config with just a top-level `[backup]` has no periodic backups running.

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

**Both databases share one host directory.** Compose mounts a single `/backup` volume in the
postgres container, sourced from `ticket_db.backup.path`; `workflow_db.backup.path` is never
mounted, so give both sections the *same* path. Automatic dumps are named
`ur-backup-<timestamp>.pgdump` with no database name in them, so the two databases share a
filename space and a `retain_count` budget in that directory.

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

Inject host-side skills into worker containers at runtime. Skills are bind-mounted read-only into `/home/worker/.agent-shared/potential-skills/<name>/` alongside skills baked into the container image.

Scoping is by worker **strategy**, not by project. There is no `[projects.<key>].skills` field. To vary skills per project, define a mode in [`[worker_modes]`](#worker_modes-section) and launch that project's workers with `-m <mode>`.

Three sub-tables by strategy:

| Sub-table | Workers Affected |
|-----------|-----------------|
| `[skills.common]` | All workers |
| `[skills.code]` | `code`-strategy workers (in addition to common) |
| `[skills.design]` | `design`-strategy workers (in addition to common) |

`manual`-strategy workers get `common` plus **both** `code` and `design` globals (`GlobalSkillsConfig::for_strategy`). There is no `[skills.manual]`.

Each key is the skill name; the value is a path to the skill directory on the host (a directory containing `SKILL.md`).

```toml
[skills.common]
my-skill = "/Users/me/.ur/skills/my-skill"

[skills.code]
research-helper = "/Users/me/.ur/skills/research-helper"

[skills.design]
internal-tool = "/opt/skills/internal-tool"
```

**Use absolute host paths. `%URCONFIG%` is a trap here** — unlike every other template field:

`[skills]` values become Docker volume **sources** verbatim (`RunOptsBuilder::add_extra_skills`), and there is no host↔container remapping the way `instruction_md`/`memory_dir`/`brain_dir` get via `convention_check_path`. `Config::load` runs inside the server container where `UR_CONFIG=/config`, so `%URCONFIG%/skills/foo` resolves to `/config/skills/foo` — a path the host Docker daemon cannot see. Docker then binds an auto-created empty directory and the skill silently comes up blank. Existence checks are also skipped in-container (`resolve_skill_section` short-circuits when `UR_HOST_CONFIG` is set), so nothing warns you.

Write `/Users/me/.ur/skills/foo`, not `%URCONFIG%/skills/foo`. The host CLI validates absolute paths at `ur start`, which is where a typo will surface.

**Other path rules:**
- `%PROJECT%/...` is rejected — skills must be project-stable, not workspace-relative.
- A path that does not exist, or is not a directory, is a hard config-load error on the host.

**Override semantics:** A host skill with the same name as a baked-in skill shadows the baked version, so you can patch a shipped skill without rebuilding the image.

**Duplicate names:** the same name in `[skills.common]` and `[skills.code]` (or `design`) is a config error — common is always included, so remove one. The same name in both `code` and `design` is allowed (paths may differ).

**Globals beat `--skills`:** `merge_global_skills` appends globals even when a launch passes an explicit `-s/--skills`. `[skills]` means "everywhere".

---

## `[worker_modes]` Section

Named skill bundles selected with `ur worker launch -m <mode>`. This is how you scope skills to a kind of work — and, in practice, to a project.

Built-in modes come from `WorkerStrategy::skills()` (`crates/server/src/strategy.rs`):

| Mode | Skills |
|------|--------|
| common (all) | `green`, `cli-design`, `reclaude`, `writing-skills`, `rag-docs`, `address-feedback`, `code-review`, `brain`, `brain:init` |
| `code` | common + `implement`, `ship`, `bacon`, `systematic-debugging`, `test-driven-development` |
| `design` | common + `design`, `dispatch` |
| `manual` | common + `implement`, `implement-agents`, `ship`, `bacon`, `systematic-debugging`, `test-driven-development`, `design`, `dispatch` |

### `[worker_modes.<name>]`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `base` | string | **yes** | `"code"`, `"design"`, or `"manual"`. Sets pool-slot semantics (exclusive vs shared) and the default model |
| `skills` | string[] | **yes** | Full skill list — **replaces** the base strategy's list, does not extend it |
| `model` | string | no | Model alias override |
| `effort` | string | no | Reasoning-effort override for the mode's agent |
| `agent` | string | no | Which agent runs this mode (e.g. `"agy"`). When omitted, falls through to the top-level `agent`, then `"claude"`; an unrecognized value is a config error naming the mode |

Custom modes are added alongside the built-in three; defining `[worker_modes.code]` replaces the built-in `code`.

```toml
[worker_modes.ur-review]
base = "code"
skills = ["code-review", "green", "bacon"]
model = "claude-opus-5[1m]"
agent = "claude"
```

### `[worker_models]` Section

Default model and reasoning effort per **agent** and strategy (not per mode).

```toml
[worker_models.claude]
code = { model = "sonnet", effort = "high" }

[worker_models.codex]
code = { model = "gpt-5.6-terra", effort = "high" }
```

Every `[worker_models]` key is an agent table (`claude`, `codex`, or `agy`). Each strategy entry is an
inline table with independently optional `model` and `effort`; flat strings are a hard migration
error. Unknown agents, strategies, and unsupported agent effort values are rejected at parse time.

| Strategy | Claude | Codex | AGY |
|----------|--------|-------|-----|
| `code` | `sonnet` | `gpt-5.6-terra` | `gemini-3.8-flash` |
| `design` | `opus` | `gpt-5.6-sol` | `gemini-3.8-flash` |
| `manual` | `opus` | `gpt-5.6-sol` | `gemini-3.8-flash` |

All agents default to `effort = "medium"`. AGY accepts the shared effort vocabulary
(`low`, `medium`, `high`, `xhigh`, `max`, `ultra`) at config-parse time and leaves the
model/effort matrix to AGY itself. Use the base model ID, such as `gemini-3.8-flash`, rather
than an effort-suffixed model ID when setting an AGY model.

```toml
[worker_models.agy]
code = { model = "gemini-3.8-flash", effort = "high" }
```

```toml
[worker_models.claude]
manual = { model = "claude-opus-5[1m]", effort = "high" }
```

**Model and effort precedence:** each field resolves independently: `worker_modes.<mode>.<field>` → `[worker_models.<agent>].<base>.<field>` → `agent.default_model(strategy)` / `agent.default_effort()` (`AgentType`, `crates/ur_config/src/agent.rs`). Resolution happens after the agent is resolved, so an explicit `--agent` launch override uses that agent's defaults.

**Agent precedence** (`resolve_mode`, and see the top-level `agent` field above): `--agent` launch flag → `worker_modes.<mode>.agent` → top-level `agent` key → `"claude"`.

**Skill precedence** (`resolve_skills`): explicit `-s/--skills` → `worker_modes.<mode>.skills` → `worker_modes.code`. `[skills]` globals are appended in every case.

**Parsing note:** neither section is part of `RawConfig`. The server re-parses `ur.toml` for them via `WorkerModesConfig::from_toml`, so errors surface at server startup rather than at `Config::load()`.

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
| `repo` | string | — | **yes**, unless `local = true` | Git remote URL |
| `local` | bool | `false` | no | Declares a repo-less **local project**: no git remote, no pool, no dispatch. Mutually exclusive with `repo` |
| `name` | string | `<key>` | no | Display label |
| `pool_limit` | u32 | `10` | no | Max cached repo clones in the pool. **Not valid with `local = true`** |
| `hostexec` | string[] | `[]` | no | Additional host-exec commands workers may call for this project |
| `hostexec_scripts` | string[] | `[]` | no | Relative paths to host-exec scripts workers may invoke |
| `instruction_md` | template path | — | no | Project-level instruction file (e.g. CLAUDE.md). Falls back to `<config_dir>/projects/<key>/CLAUDE.md`. The old key `claude_md` is still accepted (deprecation warning) |
| `memory_dir` | template path | — | no | Claude auto-memory dir, mounted read-write. `%PROJECT%` rejected. Falls back to `<config_dir>/projects/<key>/memory/` |
| `brain_dir` | template path | — | no | Per-project brain dir, mounted read-write at `/brain`. `%PROJECT%` rejected. Falls back to `<config_dir>/projects/<key>/brain/` |
| `max_fix_attempts` | u32 | `10` | no | Fix loop iterations before stalling the agent |
| `max_implement_cycles` | u32 | — | no | Overrides `[server].max_implement_cycles` for this project |
| `push_again_exit_code` | i32 | — | no | Exit code the verify hook returns to mean "push again" without charging an implement cycle |
| `protected_branches` | string[] | `["main", "master"]` | no | Branch patterns that cannot be force-pushed (supports globs) |
| `ignored_workflow_checks` | string[] | `[]` | no | CI check names to skip when evaluating workflow status |

Both `memory_dir` and `brain_dir` are `create_dir_all`'d and chowned to the worker UID before mounting, and these per-project fields only apply when the worker has a project key. `memory_dir` is never mounted in bare `-w` workspace mode; a bare `-w` worker gets `/brain` only from the top-level `workspace_brain_dir`. Parallel workers on one project share a single `memory_dir`, so simultaneous `MEMORY.md` writes can race; there is no mitigation.

**Removed fields — setting one is a hard config-load error, not a warning:**

| Removed Field | Replacement |
|---|---|
| `git_hooks_dir` | `<config_dir>/projects/<key>/hooks/git/` or in-repo `ur-hooks/git/` |
| `skill_hooks_dir` | `<config_dir>/projects/<key>/hooks/skills/` or in-repo `ur-hooks/skills/` |
| `workflow_hooks_dir` | `<config_dir>/projects/<key>/hooks/workflow/` or in-repo `ur-hooks/workflow/` |
| `mounts` at project root | move inside `[projects.<key>.container]` |

### `[projects.<key>.container]`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `image` | string | **yes** | Container image alias or full reference. Aliases (`"ur-worker"`, `"ur-worker-rust"`) resolve per-agent at launch time; see the table below. Use `"image:tag"` or `"registry/image:tag"` for a full reference, used unchanged for any agent |
| `mounts` | string[] | no | Extra volume mounts: `"source:destination"`. Source supports `%URCONFIG%/...` or absolute paths (`%PROJECT%` **not allowed** here) |
| `ports` | string[] | no | Port mappings: `"host_port:container_port"` |

| Alias | Claude | Codex | AGY |
|-------|--------|-------|-----|
| `ur-worker` | `ur-worker-claude:latest` | `ur-worker-codex:latest` | `ur-worker-agy:latest` |
| `ur-worker-rust` | `ur-worker-rust-claude:latest` | `ur-worker-rust-codex:latest` | `ur-worker-rust-agy:latest` |

### AGY credentials and paths

AGY stores runtime state under `~/.gemini/antigravity-cli/`, but ur-managed customizations
(skills, skill hooks, and `AGENTS.md`) under `~/.gemini/config/`. Its renewable OAuth bundle
is the single read-write bind mount
`$UR_CONFIG/agy/antigravity-oauth-token` →
`~/.gemini/antigravity-cli/antigravity-oauth-token`; ur never mounts all of `~/.gemini`,
because that tree also contains per-worker SQLite state, logs, updater state, and installation
identity. `ur init` creates the parent and empty token at mode 0600. An empty token is
intentional: launch the worker and complete sign-in in its pane. There is no host seed or save
round trip, and a missing token must not block launch. AGY refreshes expired credentials in
place; the single-file cache remains valid even when concurrent workers refresh it.

### `[projects.<key>.tui]`

| Field | Type | Description |
|-------|------|-------------|
| `theme` | string | Per-project theme override (overrides the global `[tui].theme`) |

```toml
[projects.ur]
repo = "https://github.com/org/ur.git"
pool_limit = 5
hostexec = ["jq", "rg"]
instruction_md = "%URCONFIG%/projects/ur/CLAUDE.md"
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

### Local projects (`local = true`)

A **local project** is a directory on the host with no git remote to clone from — an
arbitrary project you want to work on with `ur` rather than a repo the workflow
manages. Set `local = true` and omit `repo`:

```toml
[projects.myapp]
local = true
name = "My App"
hostexec = ["make", "npm"]
hostexec_scripts = ["scripts/deploy.sh"]
brain_dir = "%URCONFIG%/projects/myapp/brain"

[projects.myapp.container]
image = "ur-worker"
mounts = ["%URCONFIG%/shared-data:/var/data:ro"]
```

Everything except the repo works exactly as it does for a pool-backed project:
image selection, mounts, ports, `hostexec` / `hostexec_scripts`, `instruction_md`,
`brain_dir`, `memory_dir`, and the per-project TUI theme.

**What works:**

- Tickets can be created against the project (`ur ticket create --project myapp`).
- Manual workers with a workspace mount:
  `ur worker launch -m manual -w . -p myapp`. If the directory's basename matches
  the project key, `-p` is derived from the cwd, so `umanw` alone is enough.
- `ur project add --local <dir>` writes the entry (key derived from the directory
  name; the directory need not be a git repo). `ur project remove` needs no
  `--force`, since there is no pool to destroy.

**What is refused, and why:**

| Action | Result |
|---|---|
| `--dispatch` | Rejected — no repo to clone, branch to push, or PR to open |
| `-m code` / `-m design` | Rejected — both own a ticket branch in a pool slot |
| Launch without `-w` | Rejected — there is no repo pool to fall back on |
| `--context-repos myapp` | Rejected — context repos mount a shared pool clone |
| Dispatch from the TUI | Refused client-side with a banner; such tickets render as blocked (`□`) |

Config errors: `repo` together with `local = true`, and `pool_limit` with
`local = true`. Workflow-only fields (`protected_branches`,
`max_implement_cycles`, `max_fix_attempts`, `push_again_exit_code`,
`ignored_workflow_checks`) are accepted and ignored, so a project can be flipped
between local and repo-backed by editing one line.

---

## Template Path System

The `instruction_md` field (and `container.mounts` source) uses template strings resolved at container launch time.

| Form | Example | Resolves To | Effect |
|------|---------|-------------|--------|
| `%PROJECT%/...` | `%PROJECT%/CLAUDE.md` | `ProjectRelative` | No extra mount; path accessible at `/workspace/<rel_path>` via existing workspace mount |
| `%URCONFIG%/...` | `%URCONFIG%/projects/ur/CLAUDE.md` | `HostPath` | Volume-mounted from `<config_dir>/projects/ur/CLAUDE.md` |
| `/absolute/path` | `/opt/docs/CLAUDE.md` | `HostPath` | Volume-mounted from that path |

Validation runs at config load time. Unrecognized `%VAR%` patterns cause an immediate error.

### Mount Destinations

| Config Field | Container Path | Env Var |
|---|---|---|
| `instruction_md` | `/var/ur/project-instruction/CLAUDE.md` (agent-derived filename) | `UR_PROJECT_INSTRUCTION` |
| `container.mounts` | user-specified destination | (none) |
| host hooks overlay — git | `/var/ur/host-hooks/git/` | (none) |
| host hooks overlay — skills | `/var/ur/host-hooks/skills/` | (none) |

### Hook Convention Paths (No Config Fields)

Git and skill hooks use a two-layer overlay resolved from fixed convention paths — no `ur.toml` fields needed.

| Layer | Host Path | Container Path | Precedence |
|-------|-----------|----------------|------------|
| Host overlay — git | `~/.ur/projects/<key>/hooks/git/` | `/var/ur/host-hooks/git/:ro` | wins on conflict |
| In-repo — git | `<workspace>/ur-hooks/git/` | `/workspace/ur-hooks/git/` | applied first |
| Host overlay — skills | `~/.ur/projects/<key>/hooks/skills/` | `/var/ur/host-hooks/skills/:ro` | wins on conflict |
| In-repo — skills | `<workspace>/ur-hooks/skills/` | `/workspace/ur-hooks/skills/` | applied first |

Workflow hooks (server-side, not container-mounted) follow the same overlay order:
1. `~/.ur/projects/<key>/hooks/workflow/pre-push` (host overlay — wins)
2. `<slot>/ur-hooks/workflow/pre-push` (in-repo — fallback)

---

## Convention-Based File Layout

Several behaviors trigger automatically when files exist at expected paths under `<config_dir>/projects/<key>/` — no explicit `ur.toml` config needed.

| Convention Path | Effect |
|-----------------|--------|
| `~/.ur/projects/<key>/CLAUDE.md` | Auto-mounted as the project instruction file if `instruction_md` is not set in ur.toml |
| `~/.ur/projects/<key>/hooks/git/` | Host overlay for git hooks — mounted at `/var/ur/host-hooks/git/:ro`, wins over in-repo `ur-hooks/git/` |
| `~/.ur/projects/<key>/hooks/skills/` | Host overlay for skill hooks — mounted at `/var/ur/host-hooks/skills/:ro`, wins over in-repo `ur-hooks/skills/` |
| `~/.ur/projects/<key>/hooks/workflow/pre-push` | Host overlay for workflow verify hook — wins over in-repo `ur-hooks/workflow/pre-push` |
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
workspace_brain_dir = "%URCONFIG%/brain"

[proxy]
allowlist = ["api.anthropic.com", "platform.claude.com", "crates.io"]

[server]
container_command = "docker"
max_implement_cycles = 8
github_scan_interval_secs = 45

[ticket_db.backup]
path = "/var/backups/ur"
interval_minutes = 60
retain_count = 5

[workflow_db.backup]
path = "/var/backups/ur"   # must match [ticket_db.backup] — only that one is mounted
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
instruction_md = "%URCONFIG%/projects/myrepo/CLAUDE.md"
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
