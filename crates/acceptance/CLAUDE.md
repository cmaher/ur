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
`~/.codex/` layout). `check_credentials_seeded` (`crates/server/src/grpc.rs`) gates every
launch on that agent's host credentials file existing and being at least
`ur_config::MIN_SEEDED_CREDENTIALS_BYTES` bytes, regardless of whether the launch will ever
actually use it — so a codex launch with no seeded credentials fails before the container even
starts.

`seed_dummy_codex_credentials` (`tests/e2e.rs`) writes a placeholder blob directly to
`$UR_CONFIG/codex/auth.json` (`AgentType::host_credentials_path`) before any codex-agent
launch. This is a `std::fs::write`, not a CLI command, but it does not violate the design
principle above: it is a *test fixture* standing in for a real Codex OAuth login (which no CLI
command can perform non-interactively in CI), not a workaround for a missing CLI feature. The
launch path itself — resolution, the credential gate, the mount, the container boot — is still
exercised entirely through `ur worker launch`.

**Limit of the dummy credential:** it is enough to pass `check_credentials_seeded` and boot the
container, but not enough for codex itself to reach an idle prompt. The real codex CLI performs
an `account/read` check during its own TUI bootstrap ("plan type is required for chatgpt
authentication") and exits immediately when that fails, before ever firing hooks. That is fine
for scenarios that only need the container **healthy** (workerd's own healthz server comes up
independently of whether the spawned `codex` process succeeds) — but it means dispatch can never
be observed reaching a real agent pane: `NotifyIdle`/`UpdateAgentStatus(idle)` only fires once
codex is actually running, so `AwaitingDispatch` never advances to `Implementing` under a dummy
credential. `scenario_codex_dispatch` therefore only verifies the CLI/workflow side (dispatch
accepted, ticket stays open) — the exact agent-phrased dispatch text is covered by unit tests in
`crates/workerd/src/grpc_service.rs` instead of end-to-end.

**Image aliases are pre-resolved, so `resolve_image` is not under test.**
`render_projects_toml` turns every `ProjectEntry.image` into a full, CI-tagged reference
(`ur-worker-rust-codex:ci-<label>`) because the suite builds CI-tagged images — and
`AgentType::resolve_image` passes any value containing `:` through unchanged. So no scenario
exercises launch-time alias-to-tag resolution; `scenario_codex_image_template` pins that a full
reference reaches `docker run` untouched per agent, and the alias path is unit-tested
(`resolve_image_per_alias_and_agent`, `resolve_worker_image_*`). Covering it end-to-end would
require `resolve_image` to honor `UR_IMAGE_TAG` instead of hardcoding `:latest`. Don't "fix"
this by writing a bare alias into a test config: an unknown alias is rejected at parse time by
`validate_image_alias`, and a known one resolves to a `:latest` tag CI never builds, so the
stack would fail to start or the launch would fail to pull.

## AGY-agent acceptance coverage

AGY has no host credential source. Its scenarios launch with a missing or empty
`$UR_CONFIG/agy/antigravity-oauth-token`; the server creates and bind-mounts that one file and
AGY waits for interactive Google sign-in. Dedicated `agyproj` and `rustagyproj` entries ensure
the pre-resolved CI-tagged references select AGY images.

CI cannot complete interactive OAuth, so `scenario_agy_dispatch` verifies the image launch
and workflow/CLI side while workerd unit tests pin the exact `/clear` then `/implement`
phrasing. `scenario_agy_stop_hook` verifies the baked global hook command and invokes
`workertools notify-idle --json` in the running container, proving the command reaches
NotifyIdle and returns AGY's required `{}` stdout. The spike covers a credentialed real-model
Stop event.

## Isolated stacks for config that isn't live-reloadable

`WorkerModesConfig` (the top-level `agent` default, `[worker_modes]`, `[worker_models]`) is
parsed once from `ur.toml` at server startup and is not part of `ProjectRegistry`'s live
reload — unlike per-project config, there is no way to change it against an already-running
server. `scenario_default_agent_config` therefore cannot use the shared `TestEnv` stack (which
is fixed to the claude default for every other scenario); it runs its own `ur start`/`ur stop`
cycle against a second `ur.toml` with `agent = "codex"`, using `test_names("agent-default")` and
a `server_port` chosen well clear of the shared stack's derived port range so the two stacks
never collide.
