# HostExec Design (ur-7jle)

General host command execution replacing dedicated git/gh passthrough with a Lua-configured command gateway.

## Architecture

```
Worker Container          ur-server (container)         ur-hostd (host)
+--------------+         +-------------------+         +--------------+
| git status   |         |                   |         |              |
|   |          |  gRPC   | HostExec service   |  gRPC   | Execute cmd  |
| bash shim    |--------→|  1. Allowlist check|--------→| on real host |
|   |          |         |  2. Lua transform  |         | Stream output|
| ur-tools     |         |  3. CWD mapping    |         |              |
| host-exec    |<--------|  4. Forward to     |<--------|              |
|              | stream  |     hostd          | stream  |              |
+--------------+         +-------------------+         +--------------+
```

## Protocol

### hostexec.proto (package `ur.hostexec`, worker <-> ur-server)

```protobuf
service HostExecService {
  rpc Exec(HostExecRequest) returns (stream ur.core.CommandOutput);
  rpc ListCommands(ListHostExecCommandsRequest) returns (ListHostExecCommandsResponse);
}

message HostExecRequest {
  string command = 1;
  repeated string args = 2;
  string working_dir = 3;  // container-side cwd
}

message ListHostExecCommandsRequest {}

message ListHostExecCommandsResponse {
  repeated string commands = 1;
}
```

### hostd.proto (package `ur.hostd`, ur-server <-> ur-hostd)

```protobuf
service HostDaemonService {
  rpc Exec(HostDaemonExecRequest) returns (stream ur.core.CommandOutput);
}

message HostDaemonExecRequest {
  string command = 1;
  repeated string args = 2;
  string working_dir = 3;  // host-side path (already mapped)
}
```

Feature-gated in ur_rpc: `hostexec` and `hostd` features.

## Lua Configuration

### Defaults (compiled into ur-server)

git and gh are built-in commands with default Lua scripts embedded via `include_str!`. No user config needed for these.

### User config: `~/.ur/hostexec/allowlist.toml`

Extends built-in defaults. User-provided `lua = "file.lua"` overrides `default_script = true`.

```toml
[commands]
tk = {}                          # passthrough, no transform
# git = { lua = "my-git.lua" }  # override default with custom script
```

### Lua transform interface

Each Lua script exports a `transform` function:

```lua
-- Receives: command, args (table), working_dir
-- Returns: modified args table
-- Raise error to reject the request

function transform(command, args, working_dir)
    local blocked = {"-C", "--git-dir", "--work-tree", "-ccore.worktree"}
    for _, arg in ipairs(args) do
        for _, b in ipairs(blocked) do
            if arg == b or arg:find("^" .. b .. "=") then
                error("blocked flag: " .. arg)
            end
        end
    end
    return args
end
```

### Lua runtime

Use `mlua` (actively maintained, good ergonomics) with Lua 5.4. The Lua environment must be sandboxed — remove `io`, `os`, `loadfile`, `dofile` from globals. Create one `mlua::Lua` instance at server startup, reuse across requests (transform functions are stateless).

### Processing pipeline (ur-server)

1. Receive `HostExecRequest` from worker
2. Check command against merged allowlist (defaults + user config) — reject if not listed
3. Map `working_dir`: replace `/workspace` prefix with host-side workspace path for the process
4. If Lua script configured, run `transform(command, args, working_dir)` via mlua — reject on error, use returned args
5. Forward transformed `HostDaemonExecRequest` to ur-hostd

## Components

### ur-hostd (`crates/hostd/`)

New crate. Standalone binary running on macOS host.

- tonic gRPC server on `127.0.0.1:<hostd_port>` (default 42070, configurable in `ur.toml`)
- Implements `HostDaemonService` — spawns processes, streams `CommandOutput`
- Trusts ur-server completely (no validation)
- Extract `spawn_child_output_stream` from `crates/server/src/stream.rs` into a shared crate (e.g., `ur_common` or `ur_rpc`) so both ur-server and ur-hostd can use it

Lifecycle:
- `ur start`: spawns ur-hostd as background process, writes PID to `~/.ur/hostd.pid`, then `docker compose up`
- `ur stop`: kills ur-hostd via PID file, then `docker compose down`
- Stale PID file detection (process dead but file exists) with cleanup

ur-server connects via `host.docker.internal:<hostd_port>` (Docker Desktop / OrbStack on macOS). For Linux CI (`extra_hosts: ["host.docker.internal:host-gateway"]` in compose). Configured through docker-compose environment as `HOSTD_ADDR`.

### ur-workerd (`crates/workercmd/workerd/`)

New crate. Daemon inside worker container.

- On startup: calls `ListHostExecCommands` on ur-server, gets command list
- Creates bash shims in `/home/worker/.local/bin/<command>`:
  ```bash
  #!/bin/sh
  exec ur-tools host-exec <command> "$@"
  ```
- `/home/worker/.local/bin` added to PATH in container image (prepended so shims shadow system binaries)
- Stays running as background daemon (future uses)
- Started by container entrypoint as background process before exec claude
- Retry with backoff if ur-server is not yet ready at startup

### ur-tools (`crates/workercmd/tools/`)

New unified worker binary with subcommands. `host-exec` is the first subcommand.

- `ur-tools host-exec <command> [args...]`
- Captures cwd, sends `HostExecRequest { command, args, working_dir: cwd }`
- Streams `CommandOutput` to stdout/stderr, exits with remote exit code
- Same streaming pattern as current git/gh worker binaries
- `ur-ping` remains separate for now (can be migrated to a subcommand later)

### Cleanup

Removed:
- `proto/git.proto`, `proto/gh.proto`
- `crates/server/src/grpc_git.rs`, `crates/server/src/grpc_gh.rs`
- `crates/server/src/git_exec.rs` — `validate_args` logic moves to default `git.lua`; `RepoRegistry` is retained and moved (used by `ProcessManager` for workspace path tracking and CWD mapping)
- `crates/workercmd/git/`, `crates/workercmd/gh/`
- `git` and `gh` features from `ur_rpc` (Cargo.toml, build.rs, lib.rs cfg blocks)
- `git` and `gh` features from `crates/server/Cargo.toml` (and `default` feature list)
- `#[cfg(feature = "git")]` / `#[cfg(feature = "gh")]` blocks in `crates/server/src/lib.rs`
- Baked-in `/usr/local/bin/git` and `/usr/local/bin/gh` from worker container Dockerfile
- `git` and `github-cli` apk packages from server container Dockerfile (no longer needed — server validates/forwards, doesn't execute)

Modified:
- `ur_rpc/build.rs` — add `hostexec` and `hostd` features
- `crates/server/src/grpc_server.rs` — register `HostExecService` on per-agent servers
- `crates/ur/` — `start` spawns ur-hostd + PID, `stop` kills + cleans
- `ur_config` — add `hostd_port`, hostexec config paths
- `docker-compose.yml` template — pass `HOSTD_ADDR=host.docker.internal:<port>` to ur-server; add `extra_hosts` for Linux
- Worker container Dockerfile — add ur-workerd + ur-tools, add `/home/worker/.local/bin` to PATH, update entrypoint

New crates require CLAUDE.md files per project convention:
- `crates/hostd/CLAUDE.md`
- `crates/workercmd/workerd/CLAUDE.md`
- `crates/workercmd/tools/CLAUDE.md`

## CWD Mapping

Straightforward prefix replacement: `/workspace` -> host workspace path registered for the process (via `RepoRegistry`).

Git `-C` flags: stripped for now (same as current behavior). Future nested repo name mapping tracked in ur-pc18.

## Security

The security boundary is at ur-server's Lua validation layer. ur-hostd trusts ur-server completely.

Passthrough commands (no Lua script) execute with no argument filtering. This is acceptable for commands like `tk` where the threat model is low. Commands that interact with host credentials or config (git, gh) should always have a Lua transform.

The default `git.lua` blocks flags that enable sandbox escape (`-C`, `--git-dir`, `--work-tree`, `-ccore.worktree`). The default `gh.lua` provides baseline validation.

## Future: Per-Project Config

The allowlist/Lua config is global for now (`~/.ur/hostexec/`). Future work will add per-project configuration (project concept TBD).
