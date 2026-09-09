# acceptance (Test-Only Crate)

End-to-end acceptance tests that exercise the full stack: server, container lifecycle, and worker command binaries.

- Gated behind the `acceptance` cargo feature: `cargo test -p acceptance --features acceptance`
- Excluded from workspace default-members so `cargo test` does not run them
- Tests require Docker — they will not pass in bare CI
- Tests use pre-built binaries from `target/` (ur-server, ur) and worker commands (ur-ping, git) baked into the container image
- All scenarios share a single `ur start` / `ur stop` cycle via `e2e_all` to avoid port collisions and reduce total runtime

## Test architecture

`e2e_all` is the sole `#[test]` entry point. It creates one shared `TestEnv` (temp config dir, bare repo, `ur start`), runs all scenarios sequentially as plain helper functions, then tears down. Each scenario gets a `&TestEnv` reference and uses its own ticket IDs and container names. Scenarios use `catch_unwind` to force-remove their worker containers on failure before re-raising.

## Design principle

Tests MUST use only CLI commands (`ur-server`, `ur`, `ur-ping`, `git`) — never programmatic/in-process wiring. The point of acceptance tests is to validate the real user-facing workflow. If a test scenario requires manual setup that the CLI doesn't support, that means the CLI is incomplete and needs a new feature — not that the test should work around it with code.

## Codex-agent credentials

Codex-agent scenarios never drive a real Codex model turn — they only assert on the launch
path itself (agent resolution reaching `agent_type` in `ur worker list`, the in-container
`~/.codex/` layout, dispatch phrasing reaching the tmux pane). `check_credentials_seeded`
(`crates/server/src/grpc.rs`) gates every launch on that agent's host credentials file
existing and being at least 10 bytes, regardless of whether the launch will ever actually use
it — so a codex launch with no seeded credentials fails before the container even starts.

`seed_dummy_codex_credentials` (`tests/e2e.rs`) writes a placeholder blob directly to
`$UR_CONFIG/codex/auth.json` (`AgentType::host_credentials_path`) before any codex-agent
launch. This is a `std::fs::write`, not a CLI command, but it does not violate the design
principle above: it is a *test fixture* standing in for a real Codex OAuth login (which no CLI
command can perform non-interactively in CI), not a workaround for a missing CLI feature. The
launch path itself — resolution, the credential gate, the mount, the container boot — is still
exercised entirely through `ur worker launch`.

## Isolated stacks for config that isn't live-reloadable

`WorkerModesConfig` (the top-level `agent` default, `[worker_modes]`, `[worker_models]`) is
parsed once from `ur.toml` at server startup and is not part of `ProjectRegistry`'s live
reload — unlike per-project config, there is no way to change it against an already-running
server. `scenario_default_agent_config` therefore cannot use the shared `TestEnv` stack (which
is fixed to the claude default for every other scenario); it runs its own `ur start`/`ur stop`
cycle against a second `ur.toml` with `agent = "codex"`, using `test_names("agent-default")` and
a `server_port` chosen well clear of the shared stack's derived port range so the two stacks
never collide.
