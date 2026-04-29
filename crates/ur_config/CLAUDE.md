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

## Other Config

- Proxy constants: `DEFAULT_PROXY_HOSTNAME` ("ur-squid"), `SQUID_PORT` (3128); `hostname` replaces the old `port` field
- `Config::squid_dir()` returns `$UR_CONFIG/squid/` — where SquidManager writes config files
- Worker binaries (`workertools`, `workerd`, `ur-ping`) depend on this crate for env var constant names only
