# Ur

Coding LLM coordination framework. Native macOS monolith managing containerized Claude Code workers via tonic gRPC over TCP.

## Structure

Cargo workspace:
- `crates/ur/` - Host CLI (TUI, process management, ticket management)
- `crates/server/` - Server (`ur-server`, orchestration, gRPC server, container management)
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
- **Clippy line limits**: Functions must stay under clippy's `too_many_lines` threshold (100 lines). When a function exceeds the limit, extract a meaningful, self-contained unit of logic — not an arbitrary slice of lines to appease the linter. The extracted function should represent a coherent concept (e.g., `resolve_project` from a project-resolution closure, not `load_from_part2`). Look for natural boundaries: closures, match arms, or blocks with their own inputs and outputs.

## Development

- **`ur` CLI is pre-built and available on PATH** (`/home/worker/.local/bin/ur`). Use `ur` directly — do NOT use `cargo run --bin ur`. This applies to all `ur` subcommands (ticket, server, etc.).
- **CI runs via the pre-push git hook** — avoid running `cargo make ci` manually. The pre-push hook runs full CI (fmt, clippy, tests, build), so `git push` can take several minutes. When pushing in the background, use a long timeout (≥5 min) when waiting for output.
- `cargo make fmt-fix` - Fix formatting
- `cargo make clippy` - Run clippy lints
- `cargo make audit` - Check dependency vulnerabilities

## Rust Verification (Bacon)

- Bacon runs as a **persistent background watcher** -- the user starts it once in a terminal. Do NOT launch `bacon` yourself.
- Read `.bacon-locations` to get current diagnostics (errors/warnings from the last compile). This file is auto-updated by bacon's export-locations feature.
- If `.bacon-locations` doesn't exist or is empty, bacon may not be running. Fall back to `cargo check --message-format short 2>&1`.
- If you need to see only errors (no warnings), filter lines starting with `error` from `.bacon-locations`.

## Conventions

- **Single config file**: All user configuration lives in `ur.toml` (`$UR_CONFIG/ur.toml`, default `~/.ur/ur.toml`). Do NOT create separate config files for new features — extend `ur.toml` instead. Lua scripts for hostexec commands live in `~/.ur/hostexec/` and are referenced by filename from the `[hostexec.commands]` section of `ur.toml`.
- **Plans** (`docs/plans/`): Filenames and content MUST include the relevant ticket number (e.g., `ur-a1b2c`).
- **PR descriptions**: MUST reference the ticket number being addressed.
- **CLAUDE.md per crate and container**: Each crate (`crates/*/`) and container definition must have its own `CLAUDE.md` with crate/container-specific guidance.
- **Tests**: NEVER `#[ignore]` or skip tests to make them pass. NEVER push with `--no-verify` to bypass failing tests. Always fix the underlying issue — including pre-existing failures encountered during a push.
- **Error propagation**: NEVER allow operations to fail silently. All errors from server-side operations (gRPC handlers, pool management, git operations, container lifecycle) MUST propagate back to the CLI and be displayed to the user. If an async operation can fail, its error must be surfaced — not swallowed, logged-only, or ignored. When adding new server-side functionality, verify the full error path: server error → gRPC Status → CLI display.
- **Acceptance tests**: When adding a new launch mode, flag, or code path (e.g., `-p` project pool launches vs `-w` workspace mounts), the acceptance test suite MUST cover that path. Do not ship a feature whose primary flow is untested in the e2e acceptance tests.
- **Cross-compile**: Use `cargo zigbuild` (requires `zig` + `cargo-zigbuild`) targeting `aarch64-unknown-linux-gnu` / `x86_64-unknown-linux-gnu` to match the debian bookworm container. Match the host arch like the container crate does (`std::env::consts::ARCH`).
- **Container tests**: Docker backend tested locally on macOS and in CI (ubuntu-latest).
- **String-based enum constants**: When a domain concept is represented as strings in proto/gRPC but has a fixed set of values, use defined constants or Rust enums instead of string literals. For example, use `ur_rpc::lifecycle::*` constants (e.g., `lifecycle::OPEN`, `lifecycle::IMPLEMENTING`) for lifecycle statuses, and `ticket_db::model::LifecycleStatus` or `workflow_db::model::*` for server-side code. Apply this pattern to any new string-based enums.
- **No feature flags**: Do NOT add Cargo feature flags unless explicitly instructed. The only remaining feature flags are `temporal` (ur_rpc/server, gating unreleased temporal service), `stream`/`error` (ur_rpc, gating optional dependencies), and `acceptance` (acceptance tests). All gRPC services compile unconditionally.
- **Codeflows** (`docs/codeflows/`): Detailed flow diagrams for cross-cutting concerns (credential injection, process launch, etc.). Consult these before modifying multi-component flows.

@docs/codeflows/CLAUDE.md
