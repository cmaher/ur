# acceptance (Test-Only Crate)

End-to-end acceptance tests that exercise the full stack: server, container lifecycle, and worker command binaries.

- Gated behind the `acceptance` cargo feature: `cargo test -p acceptance --features acceptance`
- Excluded from workspace default-members so `cargo test` does not run them
- Tests require a container runtime (Apple `container` or Docker) — they will not pass in bare CI
- Tests use pre-built binaries from `target/` (ur-server, ur) and worker commands (ur-ping, git) baked into the container image
- Each test creates a temp UR_CONFIG dir and starts its own server instance to avoid conflicts

## Design principle

Tests MUST use only CLI commands (`ur-server`, `ur`, `ur-ping`, `git`) — never programmatic/in-process wiring. The point of acceptance tests is to validate the real user-facing workflow. If a test scenario requires manual setup that the CLI doesn't support, that means the CLI is incomplete and needs a new feature — not that the test should work around it with code.
