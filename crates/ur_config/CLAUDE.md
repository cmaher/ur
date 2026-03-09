# ur_config (Configuration & Constants)

Shared configuration loading and environment variable constants used across all Ur crates.

- Constants are defined in `src/lib.rs` — see that file for the current list
- Config is loaded from `$UR_CONFIG/ur.toml` (or `~/.ur/ur.toml`)
- All config fields have sensible defaults; missing file = all defaults
- Config sections: `workspace`, `daemon_port`, `[proxy]` (port, allowlist), `[network]` (name, urd_hostname)
- Worker commands (`workercmd/*`) depend on this crate for env var constant names only
