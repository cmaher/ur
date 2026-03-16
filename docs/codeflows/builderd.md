# Builderd Architecture

## Overview

Builderd is the host-side execution daemon. It runs natively on macOS (not containerized),
receives gRPC requests from the ur-server container, and spawns processes with full host
credentials (SSH keys, git config, etc.). It resolves `%WORKSPACE%` template variables in
working directories to decouple the server from knowing the builder's filesystem layout.

## Request Flow

```
ur-server (container)
│
├── HostExecServiceHandler (worker commands: git, gh)
│   └── BuilderDaemonServiceClient::exec() ──────┐
│                                                  │
├── RepoPoolManager (pool git operations)          │
│   └── BuilderdClient::exec_and_check() ─────────┤
│                                                  │
│                                                  ▼
│                                     ┌─────────────────────┐
│                                     │  builderd [:42071]   │
│                                     │  (host, native)      │
│                                     │                      │
│                                     │  1. Resolve %WORKSPACE% │
│                                     │  2. Spawn process    │
│                                     │  3. Stream output    │
│                                     └─────────────────────┘
```

## %WORKSPACE% Template Resolution

The server never sends resolved host paths. Instead, it sends `%WORKSPACE%`-prefixed
paths which builderd resolves at exec time.

### Resolution Logic (handler.rs)

```
if working_dir starts with "%WORKSPACE%" AND workspace is configured:
    replace "%WORKSPACE%" prefix with workspace path
else:
    use working_dir as-is
```

### Workspace Configuration

Priority: `--workspace` CLI flag > `BUILDERD_WORKSPACE` env var

The `--workspace` flag is passed by `ur start` from `config.workspace` in ur.toml.

### Examples

| Server Sends | Builderd Workspace | Resolved Path |
|---|---|---|
| `%WORKSPACE%/pool/ur/0` | `/Users/me/.ur/workspace` | `/Users/me/.ur/workspace/pool/ur/0` |
| `%WORKSPACE%` | `/Users/me/.ur/workspace` | `/Users/me/.ur/workspace` |
| `/absolute/path` | (any) | `/absolute/path` (no replacement) |

## Two Client Paths

Builderd has two clients in the server, serving different use cases:

### 1. HostExecServiceHandler (worker → server → builderd)

- **Purpose**: Worker commands (git, gh, tk) via the hostexec pipeline
- **Client**: Creates `BuilderDaemonServiceClient` directly per-request
- **CWD source**: `map_working_dir()` replaces `/workspace` prefix with `%WORKSPACE%`
- **File**: `crates/server/src/grpc_hostexec.rs`

### 2. BuilderdClient (server-internal → builderd)

- **Purpose**: Pool git operations (clone, fetch, reset, clean)
- **Client**: Shared `BuilderdClient` wrapper with `exec_and_check()` helper
- **CWD source**: `RepoPoolManager::to_builderd_path()` constructs `%WORKSPACE%/pool/<project>/<slot>`
- **File**: `crates/server/src/builderd_client.rs`

## Proto Definition

```protobuf
// proto/builder.proto
package ur.builder;

service BuilderDaemonService {
  rpc Exec(BuilderExecRequest) returns (stream ur.core.CommandOutput);
}

message BuilderExecRequest {
  string command = 1;
  repeated string args = 2;
  string working_dir = 3;    // Accepts %WORKSPACE% templates
  map<string, string> env = 4;
}
```

## Connection Path

```
ur-server container
  → UR_BUILDERD_ADDR env var (e.g., http://host.docker.internal:42071)
    → Docker host gateway resolves to host IP
      → builderd listening on 127.0.0.1:<builderd_port>
```

## Key Files

- `crates/builderd/src/main.rs` — CLI, gRPC server setup
- `crates/builderd/src/handler.rs` — BuilderDaemonHandler, %WORKSPACE% resolution
- `crates/server/src/builderd_client.rs` — Shared BuilderdClient (pool ops)
- `crates/server/src/grpc_hostexec.rs` — HostExecServiceHandler (worker commands)
- `proto/builder.proto` — gRPC service definition
- `crates/ur/src/builderd.rs` — Host lifecycle (start/stop)
