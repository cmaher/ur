# acceptance (Test-Only Crate)

End-to-end acceptance tests that exercise the full stack: server, container lifecycle, and worker command binaries.

- Gated behind the `acceptance` cargo feature: `cargo test -p acceptance --features acceptance`
- Excluded from workspace default-members so `cargo test` does not run them
- Tests require Docker — they will not pass in bare CI
- Tests use pre-built binaries from `target/` (ur-server, ur) and worker commands (ur-ping, git) baked into the container image
- All scenarios share a single `ur start` / `ur stop` cycle via `e2e_all` to avoid port collisions and reduce total runtime

## Test architecture

`e2e_all` is the sole `#[test]` entry point. It creates one shared `TestEnv` (temp config dir, bare repo, RAG docs, `ur start`), runs all scenarios sequentially as plain helper functions, then tears down. Each scenario gets a `&TestEnv` reference and uses its own ticket IDs and container names. Scenarios use `catch_unwind` to force-remove their worker containers on failure before re-raising.

## Design principle

Tests MUST use only CLI commands (`ur-server`, `ur`, `ur-ping`, `git`) — never programmatic/in-process wiring. The point of acceptance tests is to validate the real user-facing workflow. If a test scenario requires manual setup that the CLI doesn't support, that means the CLI is incomplete and needs a new feature — not that the test should work around it with code.
