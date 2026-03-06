# Ur

Coding LLM coordination framework. Native macOS monolith managing containerized Claude Code workers via tarpc over Unix Domain Sockets.

## Structure

Cargo workspace with four crates:
- `crates/ur/` - Host CLI (TUI, process management, ticket management)
- `crates/urd/` - Daemon server (orchestration, RPC server, container management)
- `crates/agent_tools/` - Worker CLI (runs inside containers, tarpc client)
- `crates/ur_rpc/` - Shared RPC contract (tarpc service traits, data types)

## Development

- `cargo make ci` - Run all CI checks (fmt, clippy, build, test)
- `cargo make fmt-fix` - Fix formatting
- `cargo make clippy` - Run clippy lints
- `cargo make audit` - Check dependency vulnerabilities

## Rust Verification (Bacon)

- Bacon runs as a **persistent background watcher** -- the user starts it once in a terminal. Do NOT launch `bacon` yourself.
- Read `.bacon-locations` to get current diagnostics (errors/warnings from the last compile). This file is auto-updated by bacon's export-locations feature.
- If `.bacon-locations` doesn't exist or is empty, bacon may not be running. Fall back to `cargo check --message-format short 2>&1`.
- If you need to see only errors (no warnings), filter lines starting with `error` from `.bacon-locations`.

## Conventions

- **Plans** (`docs/plans/`): Filenames and content MUST include the relevant ticket number (e.g., `ur-a1b2c`).
- **PR descriptions**: MUST reference the ticket number being addressed.
- **CLAUDE.md per crate and container**: Each crate (`crates/*/`) and container definition must have its own `CLAUDE.md` with crate/container-specific guidance.
