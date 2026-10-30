# acceptance (Test-Only Crate)

End-to-end acceptance tests that exercise the full stack: server, builderd, and real worker containers.

- Gated behind the `acceptance` cargo feature: `cargo test -p acceptance --features acceptance`
- Excluded from workspace default-members so `cargo test` does not run them
- Tests require Docker — they will not pass in bare CI
- Tests use pre-built binaries from `target/` (`ur`) and the worker binaries (`workertools`, `workerd`) baked into the container images
- All scenarios share a single `ur start` / `ur stop` cycle via `e2e_all` to avoid port collisions and reduce total runtime

## Scope: container-only paths

The suite runs on every `git push` (pre-push hook), and each worker container costs a slot
recycle, `docker run`, workerd init, hooks, and stop. So it keeps **one e2e path per launch
mode and per agent image** — only what needs a real container + builderd + server together —
and nothing else:

| Anchor scenario | Covers |
|---|---|
| `scenario_workspace_mount` | `-w` with no project: `{basename}-man-0`, fallback image at `UR_IMAGE_TAG`, `workspace_brain_dir` rw |
| `scenario_pool_launch` | `-p` code mode (claude): clone, model, git/gh transforms, real `git push`, `--no-verify` blocked, squid, global skills, `.agent-shared/`, `agent_type` |
| `scenario_manual_worker` | `-m manual -p`: process ID, no branch, `git checkout`, `[worker_models]` override |
| `scenario_custom_design_mode` | design-based custom mode: shared slot + custom `model` |
| `scenario_failing_startup_hook` | sync startup-hook failure fails the launch and removes the container |
| `scenario_local_project` | `local = true`: manual `-w` launch, image, script allowlist, no pool dir |
| `scenario_dispatch_creates_workflow` | `-d`: workflow visible, tmux PR label refresh, `flow cancel` kills the worker |
| `scenario_project_gitea_auto_detection` | Gitea forge: tea shim/skill, tea transform, CLAUDE.md section |
| `scenario_mounts` | `mountproj`: host-only `mounts`, `memory_dir`, `brain_dir` precedence, hook overlays, in-repo startup-bg hook |
| `scenario_missing_mount_source` | builderd refuses a missing mount source (no container starts) |
| `scenario_codex_manual_worker` | codex image + layout, dispatched |
| `scenario_agy_manual_worker` | agy image + layout, token mount, Stop hook → NotifyIdle |

**Adding coverage:** a new in-container check goes into the anchor whose configuration it
needs — never a new launch. A new launch mode or agent image gets one new anchor. Anything
observable without a container belongs elsewhere: CLI validation in `crates/ur` unit tests,
RPC/DB behavior in `grpc_ticket.rs` / `pool.rs` / `grpc.rs` unit tests, authorization against
a registered-but-containerless worker in `crates/server/tests/hostexec_script_test.rs`, config
parsing in `ur_config` / `WorkerModesConfig` tests, Lua transforms in `lua_transform.rs`.

`scenario_manual_worker` hard-codes `poolproj-man-0` and `pool/poolproj/0`, so every
`poolproj` launch before it must release slot 0; `run_scenarios` orders it right after
`scenario_pool_launch` and before the `flow cancel` path.

## Test architecture

`e2e_all` is the sole `#[test]` entry point. It creates one shared `TestEnv` (temp config dir, bare repos, `ur start`), runs all scenarios sequentially as plain helper functions, then tears down. Each scenario gets a `&TestEnv` reference and uses its own ticket IDs and container names. Scenarios use `catch_unwind` to force-remove their worker containers on failure before re-raising.

## Design principle

Tests MUST use only CLI commands (`ur`, `git`, the container runtime) — never programmatic/in-process wiring. The point of acceptance tests is to validate the real user-facing workflow. If a test scenario requires manual setup that the CLI doesn't support, that means the CLI is incomplete and needs a new feature — not that the test should work around it with code.

## Image aliases resolve against `UR_IMAGE_TAG`

Every test project configures the bare `ur-worker` alias. The server resolves it against the
launch's agent and `UR_IMAGE_TAG` (forwarded into the server container by `ur start`; see
`docs/codeflows/config.md`), so under `scripts/acceptance-ci.sh` workers run
`ur-worker-<agent>:ci-<label>` — the anchors assert exactly that. `codexproj` / `agyproj` exist
so `--agent codex` / `--agent agy` launches get their own repos and skill-hook overlays; the
same alias resolves to each agent's image.

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
credential. `scenario_codex_manual_worker` therefore only verifies the CLI/workflow side of its
`-d` launch (dispatch accepted, ticket stays open) — the exact agent-phrased dispatch text is
covered by the `implement_commands_*` unit tests in `crates/workerd/src/grpc_service.rs`.

## AGY-agent acceptance coverage

AGY scenarios deliberately provide no host credential fixture. Although the CLI can seed AGY
from the host OS keyring, `allows_in_container_bootstrap` permits these scenarios to launch with
a missing or empty `$UR_CONFIG/agy/antigravity-oauth-token`; the server creates and bind-mounts
that one file and AGY waits for interactive Google sign-in.

CI cannot complete interactive OAuth, so `scenario_agy_manual_worker` is never dispatched
(on a dispatched worker, notify-idle can push `/clear` + `/implement`); workerd unit tests pin
the exact `/clear` then `/implement` phrasing. The scenario verifies the baked global hook
command and invokes `workertools notify-idle --json` in the running container, proving the
command reaches NotifyIdle and returns AGY's required `{}` stdout. The spike covers a
credentialed real-model Stop event.

## Config that isn't live-reloadable

`WorkerModesConfig` (the top-level `agent` default, `[worker_modes]`, `[worker_models]`) is
parsed once from `ur.toml` at server startup, so the shared stack is fixed to its config. The
top-level `agent` default is covered by
`top_level_agent_default_in_full_config_resolves_every_mode` (`crates/server/src/worker.rs`)
against a complete `ur.toml`, rather than by a second e2e stack.
