# ur_config (Configuration & Constants)

Shared configuration loading and environment variable constants used across all Ur crates.

- Constants are defined in `src/lib.rs` — see that file for the current list
- Config is loaded from `$UR_CONFIG/ur.toml` (or `~/.ur/ur.toml`)
- All config fields have sensible defaults; missing file = all defaults
- Config sections: `workspace`, `server_port`, `[proxy]` (hostname, allowlist), `[network]` (name, server_hostname)

## Database Configuration

The system uses two separate Postgres databases. Each has its own config section:

### `[ticket_db]` — Ticket database (`ur_tickets`)

```toml
[ticket_db]
host     = "ur-postgres"   # default
port     = 5432            # default
user     = "ur"            # default
password = "ur"            # default; prefer UR_TICKET_DB_PASSWORD env var
name     = "ur_tickets"    # default
```

Password is overridden at runtime by the `UR_TICKET_DB_PASSWORD` environment variable if set.

Nested backup config:

```toml
[ticket_db.backup]
path             = "/path/to/backup/dir"   # omit to disable
interval_minutes = 30                       # default: 30
retain_count     = 3                        # default: 3
```

### `[workflow_db]` — Workflow database (`ur_workflow`)

```toml
[workflow_db]
host     = "ur-postgres"   # default
port     = 5432            # default
user     = "ur"            # default
password = "ur"            # default; prefer UR_WORKFLOW_DB_PASSWORD env var
name     = "ur_workflow"   # default
```

Password is overridden at runtime by the `UR_WORKFLOW_DB_PASSWORD` environment variable if set.

Nested backup config:

```toml
[workflow_db.backup]
path             = "/path/to/backup/dir"   # omit to disable
interval_minutes = 30                       # default: 30
retain_count     = 3                        # default: 3
```

### Environment Variables for Passwords

| Env var | Applies to |
|---------|-----------|
| `UR_TICKET_DB_PASSWORD` | `[ticket_db]` password |
| `UR_WORKFLOW_DB_PASSWORD` | `[workflow_db]` password |

These override the `password` field in the config file. Use env vars in production instead of storing passwords in `ur.toml`.

## Workflow Cycle Limits

### `max_implement_cycles` — per-project override with server default

`[server].max_implement_cycles` sets the default maximum number of implement cycles allowed across all projects. `[projects.<key>].max_implement_cycles` overrides this on a per-project basis.

Precedence (highest to lowest):
1. `[projects.<key>].max_implement_cycles` — project-level override
2. `[server].max_implement_cycles` — server-wide default
3. Built-in default (6) — when neither is set

If neither key is present anywhere, there is **no limit** (the value is treated as `None`).

```toml
[server]
max_implement_cycles = 6   # default for all projects

[projects.my-api]
max_implement_cycles = 10  # this project gets more cycles

[projects.quick-fix]
max_implement_cycles = 3   # this project gets fewer cycles
```

An unset `[projects.<key>].max_implement_cycles` inherits the `[server].max_implement_cycles` value. Setting `max_implement_cycles` in neither section means no cycle limit is enforced.

## Local Projects (`local = true`)

`ProjectConfig.repo` is `Option<String>`. `None` means the project is **local**: an
arbitrary host directory with no git remote, no repo pool, and no dispatch.

- `ProjectConfig::is_local()` — locality predicate (`repo.is_none()`).
- `ProjectConfig::require_repo()` — the remote, or an error naming the project. Call
  this from anything that fundamentally needs a remote (pool clone, PR creation)
  instead of unwrapping, so failures name the cause.
- `resolve_project_repo()` enforces the contract: exactly one of `repo` /
  `local = true`; both is an error; neither is an error naming both; `pool_limit`
  with `local = true` is an error. Other workflow-only fields are accepted and
  ignored so a project can be flipped local ↔ repo-backed by editing one line.

Do not add a parallel `local: bool` to `ProjectConfig` — the `Option` is what forces
every consumer of `repo` to handle absence at compile time. See
`docs/codeflows/config.md#local-projects`.

## Agents (`AgentType`)

`AgentType` is the single source of truth for everything that varies per coding agent:
`Claude` (Claude Code) and `Codex` (OpenAI Codex CLI). Every accessor is an exhaustive match
over the variants — adding a third agent means the compiler flags every accessor that needs a
new arm.

Two accessors are deliberately `None` for `Codex`:

- `memory_subdir()` — Claude's memory dir is Claude Code's own transcript-adjacent layout;
  Codex has no directory equivalent (it keeps memories in a sqlite database). Callers gate on
  `if let Some(memory_subdir) = agent.memory_subdir()` and no-op otherwise (e.g.
  `RunOptsBuilder::add_memory_dir` in `crates/server`).
- (Claude's `settings_filename()` and `auth()` are both `Some` for every current agent, but
  follow the same optional pattern for an agent with no settings-file or no-credentials
  concept.)

`proxy_domains()` returns the domains an agent needs through the forward proxy, and
`default_proxy_allowlist()` (`lib.rs`) folds it over `AgentType::ALL` for the `[proxy].allowlist`
default. Careful: that config field is *not* what Squid reads — the live list is
`$UR_CONFIG/squid/allowlist.txt`, seeded once by `ur init` from `DEFAULT_ALLOWLIST`
(`crates/ur/src/init.rs`), so adding an agent means adding its domains **in both places**.
`allowlist_covers_every_agents_proxy_domains` (`crates/ur/src/init.rs`) is the test that fails
when they drift. See `docs/codeflows/server-lifecycle.md#squid-allowlist-default`.

Command phrasing (`clear_command()`, `skill_invocation(skill, args)`) is agent-owned too, not
a workerd concern: Claude has a custom slash command per skill (`/implement ur-x`) and resets
context with `/clear`, but Codex has no custom slash commands — it discovers skills via the
`skill_search` tool from the same `SKILL.md` directory format, so its invocation instead names
the skill explicitly (`` Run the `implement` skill. Arguments: ur-x ``), and it resets context
with `/new`. `skill_invocation` takes `args: &[&str]` rather than a pre-joined string so a
multi-argument skill (e.g. `address-feedback`, which takes a ticket and a PR number) doesn't
push joining logic out to the caller.

## Top-level `agent` default

`Config.agent: AgentType` comes from the top-level `agent` key in `ur.toml` (omitted → claude).
`resolve_top_level_agent` is `pub` specifically so `crates/server`'s `WorkerModesConfig::from_toml`
— which independently re-parses the same `ur.toml` document to pull `[worker_modes]` and
`[worker_models]` — resolves the identical key the identical way (same default, same error
message naming the valid agents) instead of reimplementing the logic. See
`docs/codeflows/config.md#agent-default-precedence` for the full precedence chain
(`--agent` flag → `worker_modes.<mode>.agent` → this top-level default → claude).

## Agent Auth (`AgentAuth` / `AuthSource`)

`AgentType::auth()` returns an optional [`AgentAuth`] describing how an agent's credentials
are sourced and where its files live, for agents that need credentials at all (`None` means
no auth profile).

- `AgentAuth.source: AuthSource` — either `Keychain { service, linux_fallback }` (macOS
  keychain, falling back to a home-relative file on Linux) or `HostFile { path_from_home }`
  (always a plain file under the host user's home directory, no keychain).
- `AgentAuth.credentials_path` / `AgentAuth.app_config_path` — both home-relative paths
  (e.g. `.claude/.credentials.json`, `.claude.json`), used as-is when joining onto the
  *worker's* home directory inside a container.
- Host-side storage under `$UR_CONFIG/<agent-name>/` flattens every agent's files into one
  directory, so callers building a host path take only the filename component (via
  `Path::file_name()`) of `credentials_path` / `app_config_path` rather than the full
  relative path.
- `credentials_file_is_seeded(path)` / `MIN_SEEDED_CREDENTIALS_BYTES` — the one definition of
  "this file holds real credentials, not the empty stub a Docker bind-mount left behind." Both
  the CLI's seeding and `ur start` warning (`crates/ur`) and the server's pre-launch gate
  (`check_credentials_seeded`, `crates/server/src/grpc.rs`) go through it, so the byte
  threshold can't drift between them. Never reads the file's contents.

## Other Config

- Proxy constants: `DEFAULT_PROXY_HOSTNAME` ("ur-squid"), `SQUID_PORT` (3128); `hostname` replaces the old `port` field
- `Config::squid_dir()` returns `$UR_CONFIG/squid/` — where SquidManager writes config files
- Worker binaries (`workertools`, `workerd`, `ur-ping`) depend on this crate for env var constant names only
