# Ur

Coding LLM coordination framework. Native macOS monolith managing containerized Claude Code workers via tonic gRPC over Unix Domain Sockets.

## Structure

Cargo workspace:
- `crates/ur/` - Host CLI (TUI, process management, ticket management)
- `crates/urd/` - Daemon server (orchestration, gRPC server, container management)
- `crates/ur_rpc/` - Shared RPC contract (protobuf/tonic service definitions)
- `crates/workercmd/` - Worker binaries for containers (`ur-ping`, `git` proxy)

## Code Style

- Always use named fields for structs, not tuple structs, unless explicitly stated otherwise
- Don't add `new()` constructors for plain data structs; use struct literal syntax instead. Exception: structs wrapping collections (e.g., HashMap) benefit from `new()` for cleaner instantiation.
- Prefer manager structs with methods over free functions, named with a `Manager` suffix (e.g., `LineOfEffectManager`). Managers hold references to other managers, config, connections, or databases — never data. Data is always passed through method parameters. This allows adding system dependencies later without changing call sites.
- All managers must implement `Clone` and accept their dependencies via constructor parameters (dependency injection). Never create sub-managers internally — inject them from above so the top-level orchestrator controls the full dependency graph.
- It is OK for managers to exceed the number of arguments allowed by clippy in their constructors.
- Never write TODO stubs or placeholder implementations. Always write the real thing unless explicitly told otherwise.
- Prefer modules with smaller files (<1k non-test lines). Split large files into submodules when they exceed this threshold.

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
- **Tests**: NEVER `#[ignore]` or skip tests to make them pass. Fix the underlying issue.
- **Cross-compile**: Always support both `aarch64-unknown-linux-musl` and `x86_64-unknown-linux-musl` targets for container binaries. Match the host arch like the container crate does (`std::env::consts::ARCH`).
- **Container tests**: Apple backend tested locally on macOS, Docker backend tested in CI (ubuntu-latest).
