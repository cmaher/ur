# tmux

Typed Rust interface over the tmux CLI. Used by workerd (session creation, send-keys) and
the host CLI (attach command generation).

- `Session` is the primary type — created via `Session::create()`, then used for `send_keys()`, `set_status_left()`, etc.
- Messages sent via `send_keys()` are automatically escaped (single-quote wrapping)
- `send_keys_raw()` bypasses escaping for control sequences like `Enter`, `C-c`
- `attach_command()` returns command parts for use with container runtime `exec_interactive`
- All async operations use `tokio::process::Command` under the hood
