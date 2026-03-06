# acceptance (Test-Only Crate)

End-to-end acceptance tests that exercise the full stack: urd daemon, container lifecycle, and agent_tools CLI.

- Gated behind the `acceptance` cargo feature: `cargo test -p acceptance --features acceptance`
- Excluded from workspace default-members so `cargo test` does not run them
- Tests require a container runtime (Apple `container` or Docker) — they will not pass in bare CI
- Tests use pre-built binaries from `target/` (urd, ur) and `agent_tools` baked into the container image
- Each test creates a temp UR_CONFIG dir and starts its own urd instance to avoid conflicts

## Design principle

Tests MUST use only CLI commands (`urd`, `ur`, `agent_tools`) — never programmatic/in-process wiring. The point of acceptance tests is to validate the real user-facing workflow. If a test scenario requires manual setup that the CLI doesn't support (e.g., registering a repo, creating a socket), that means the CLI is incomplete and needs a new feature — not that the test should work around it with code.
