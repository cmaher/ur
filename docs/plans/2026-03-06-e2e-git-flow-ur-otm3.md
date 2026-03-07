# E2E Git Flow Acceptance Test (ur-otm3)

Milestone 1 capstone. Validates the full ur CLI → urd server → agent_tools worker RPC pipeline by launching a real container and executing git commands inside it.

## Epic: ur-otm3

## Tasks

### 1. Move UDS socket into UR_CONFIG directory (ur-70xq)

urd currently takes `--socket-dir` defaulting to `/tmp/ur/sockets/`. Change so that urd derives the socket path from its config directory: `$UR_CONFIG/ur.sock` (or `~/.ur/ur.sock` by default).

- urd: remove `--socket-dir`, socket path comes from `Config::load()` → `config_dir.join("ur.sock")`
- ur: default `--socket` to `$UR_CONFIG/ur.sock` instead of hardcoded `/tmp/ur/sockets/ur.sock`
- agent_tools: keep `--socket` / `UR_SOCKET` override (container mounts socket to a fixed path)

This naturally isolates test instances: each test creates a temp dir, sets `UR_CONFIG`, and gets its own socket.

### 2. Cross-compile agent_tools for linux-musl (ur-mm8q)

Add a cargo-make task that:
1. Detects host arch (`aarch64` or `x86_64`)
2. Cross-compiles agent_tools for the matching `*-unknown-linux-musl` target
3. Copies the binary into `containers/claude-worker/agent_tools`

Must support both aarch64 and x86_64. The container crate's Apple runtime already passes `--arch` matching `std::env::consts::ARCH`, so the cross-compile target must match.

This is a **build prerequisite** — runs before image build, not inside tests.

### 3. Acceptance test crate (ur-nzlf)

**Deps:** ur-70xq, ur-mm8q, ur-be36

New workspace member: `crates/acceptance/`. Gated behind a cargo feature flag (`acceptance`) so `cargo test --workspace` skips it.

Tests use pre-built binaries from `target/debug/` (urd, ur) and the pre-built container image. Test scenario:

1. Create temp directory, set `UR_CONFIG` pointing to it
2. Write minimal `ur.toml` with workspace pointing to a temp repo dir
3. Start urd binary as a child process
4. Use ur binary to `process launch test-1` (builds image, starts container)
5. Exec `agent_tools ping` inside the container → assert stdout contains "pong"
6. Create a test repo in the workspace, register it with urd's repo registry
7. Exec `agent_tools git status` inside container → assert exit code 0
8. Use ur binary to `process stop ur-agent-test-1`
9. Kill urd, clean up temp dirs

### 4. CI acceptance test step (ur-tgqr)

**Deps:** ur-nzlf

Add a step/job in `.github/workflows/ci.yml` after `cargo make ci`:

1. Install musl cross-compilation toolchain
2. Run the cargo-make cross-compile task
3. Build the container image (requires Docker — CI runs ubuntu-latest)
4. Run `cargo test -p acceptance --features acceptance`

Separate from the main CI job so regular checks stay fast.

## Dependency Graph

```
ur-70xq (socket-in-config) ──┐
ur-mm8q (cross-compile)    ──┼──► ur-nzlf (acceptance crate) ──► ur-tgqr (CI step)
ur-be36 (RPC streaming)    ──┘
```
