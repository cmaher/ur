# acceptance (Test-Only Crate)

End-to-end acceptance tests that exercise the full stack: urd daemon, container lifecycle, and agent_tools CLI.

- Gated behind the `acceptance` cargo feature: `cargo test -p acceptance --features acceptance`
- Excluded from workspace default-members so `cargo test` does not run them
- Tests require a container runtime (Apple `container` or Docker) — they will not pass in bare CI
- Tests use pre-built binaries from `target/` (urd, ur) and `agent_tools` baked into the container image
- Each test creates a temp UR_CONFIG dir and starts its own urd instance to avoid conflicts
