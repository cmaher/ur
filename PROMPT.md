You're verifying and creating a PR for the HostExec epic (ur-7jle). The working directory has all changes unstaged.

## What was implemented

The HostExec system replaces dedicated git/gh command passthrough with a general, Lua-configured host command execution gateway. 15 tickets were implemented:

### New crates
- `crates/hostd/` — ur-hostd host daemon (tonic gRPC server, spawns processes on host)
- `crates/workercmd/tools/` — ur-tools binary with `host-exec` subcommand (streams CommandOutput)
- `crates/workercmd/workerd/` — ur-workerd shim generator daemon (creates bash shims at container startup)

### New modules in existing crates
- `crates/server/src/hostexec/` — config manager (TOML allowlist + defaults) and Lua transform sandbox (mlua 5.4)
- `crates/server/src/grpc_hostexec.rs` — HostExec gRPC service handler (allowlist check, CWD mapping, Lua transform, forward to hostd)
- `crates/ur/src/hostd.rs` — ur-hostd lifecycle management (start/stop with PID file)

### New proto definitions
- `proto/hostexec.proto` — HostExecService (worker <-> ur-server)
- `proto/hostd.proto` — HostDaemonService (ur-server <-> ur-hostd)

### Modified
- `crates/ur_rpc/` — added hostexec/hostd features, extracted spawn_child_output_stream to shared stream module
- `crates/ur_config/` — added DEFAULT_HOSTD_PORT, HOSTD_PID_FILE, HOSTD_ADDR_ENV, HOSTEXEC_DIR, HOSTEXEC_ALLOWLIST_FILE constants + hostd_port config field
- `crates/ur/src/init.rs` — creates ~/.ur/hostexec/ directory
- `crates/ur/src/main.rs` — start_hostd/stop_hostd in start/stop commands
- `crates/server/src/main.rs` — loads hostexec config, passes hostd_addr
- `crates/server/src/grpc.rs` — CoreServiceHandler gets hostexec_config + hostd_addr fields
- `crates/server/src/grpc_server.rs` — registers HostExecService in build_agent_routes
- `crates/server/tests/grpc_test.rs` — updated for new CoreServiceHandler fields
- `crates/ur/src/compose.rs` — updated test for new Config field

### Removed (old git/gh passthrough)
- `proto/git.proto`, `proto/gh.proto`
- `crates/server/src/grpc_git.rs`, `crates/server/src/grpc_gh.rs`
- `crates/workercmd/git/`, `crates/workercmd/gh/`
- git/gh features from ur_rpc and server
- `crates/server/src/git_exec.rs` renamed to `registry.rs` (kept RepoRegistry only)

### Container/build changes
- `containers/claude-worker/Dockerfile` — ur-tools + ur-workerd replace git/gh binaries
- `containers/claude-worker/entrypoint.sh` — starts ur-workerd in background
- `containers/docker-compose.yml` — UR_HOSTD_ADDR env + extra_hosts
- `containers/claude-worker-rust/Dockerfile` — removed git proxy workaround
- `containers/server/Dockerfile` — removed git/github-cli packages
- `scripts/build/stage-workercmd.sh` + `stage-workercmd-native.sh` — stage ur-tools/ur-workerd
- `docs/codeflows/host-exec-flow.md` — new codeflow doc

## Verification steps

1. Run `cargo fmt --all --check` — should pass
2. Run `cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::excessive_nesting` — should pass
3. Run `cargo test --workspace --exclude container` — 1 pre-existing failure (`prepare_creates_repo_and_registers` needs real git which isn't available in container). All other tests should pass including 16 new ones.
4. Spot-check that proto files, Lua scripts, and CLAUDE.md files look correct
5. Verify no references to old git_exec/grpc_git/grpc_gh remain in source code (only in docs/plans/)

## PR details

- Branch from master, name: `hostexec-ur-7jle`
- Target: master
- Title: `feat: HostExec general host command execution gateway`
- Reference ticket ur-7jle in the PR body
- The design doc is at `docs/plans/2026-03-10-hostexec-ur-7jle-design.md`
