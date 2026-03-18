# ur_config (Configuration & Constants)

Shared configuration loading and environment variable constants used across all Ur crates.

- Constants are defined in `src/lib.rs` — see that file for the current list
- Config is loaded from `$UR_CONFIG/ur.toml` (or `~/.ur/ur.toml`)
- All config fields have sensible defaults; missing file = all defaults
- Config sections: `workspace`, `server_port`, `[proxy]` (hostname, allowlist), `[network]` (name, server_hostname)
- Proxy constants: `DEFAULT_PROXY_HOSTNAME` ("ur-squid"), `SQUID_PORT` (3128); `hostname` replaces the old `port` field
- `Config::squid_dir()` returns `$UR_CONFIG/squid/` — where SquidManager writes config files
- Worker binaries (`workertools`, `workerd`, `ur-ping`) depend on this crate for env var constant names only
