# ur_config (Configuration & Constants)

Shared configuration loading and environment variable constants used across all Ur crates.

- Constants are defined in `src/lib.rs` — see that file for the current list
- Config is loaded from `$UR_CONFIG/ur.toml` (or `~/.ur/ur.toml`)
- All config fields have sensible defaults; missing file = all defaults
- `node_id` has been removed from config — the server derives it from hostname at runtime via `resolve_node_id()`
- Config sections: `workspace`, `server_port`, `[proxy]` (hostname, allowlist), `[network]` (name, server_hostname)
- Database sections:
  - `[db]` — legacy single database config (`DatabaseConfig`), defaults: host=ur-postgres, port=5432, user/pass=ur, name=ur
  - `[ticket_db]` — ticket database config (`TicketDbConfig`), default name=ur_tickets; password overridden by `UR_TICKET_DB_PASSWORD` env var
  - `[workflow_db]` — workflow database config (`WorkflowDbConfig`), default name=ur_workflow; password overridden by `UR_WORKFLOW_DB_PASSWORD` env var
  - Both `[ticket_db]` and `[workflow_db]` are fully optional — all fields default to the same values as `[db]` except the database name
  - Each supports a nested `[ticket_db.backup]` / `[workflow_db.backup]` section
- Proxy constants: `DEFAULT_PROXY_HOSTNAME` ("ur-squid"), `SQUID_PORT` (3128); `hostname` replaces the old `port` field
- `Config::squid_dir()` returns `$UR_CONFIG/squid/` — where SquidManager writes config files
- Worker binaries (`workertools`, `workerd`, `ur-ping`) depend on this crate for env var constant names only
