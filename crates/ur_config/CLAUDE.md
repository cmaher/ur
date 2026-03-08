# ur_config (Configuration & Constants)

Shared configuration loading and environment variable constants used across all Ur crates.

- Constants: `UR_CONFIG_ENV`, `URD_ADDR_ENV`, `DEFAULT_DAEMON_PORT`
- Config is loaded from `$UR_CONFIG/ur.toml` (or `~/.ur/ur.toml`)
- All config fields have sensible defaults; missing file = all defaults
- Worker commands (`workercmd/*`) depend on this crate for env var constant names only
