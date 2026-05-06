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
│                                     │  builderd [:12323]   │
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

## BuilderContainerService

Builderd also hosts a second gRPC service, `BuilderContainerService`, that owns all worker
container lifecycle operations. Because builderd runs natively on the host, it has direct
access to the Docker socket — the server container does not.

### RPCs

| RPC | When Used | Handler |
|---|---|---|
| `LaunchWorker` | `WorkerManager::run_and_record()` — launch a new worker container | `crates/builderd/src/container_handler.rs` |
| `StopWorker` | `WorkerManager::stop()` — stop and remove a container by ID | `crates/builderd/src/container_handler.rs` |
| `ExecContainer` | `SquidManager::signal_reconfigure()` — one-shot exec (`squid -k reconfigure`) | `crates/builderd/src/container_handler.rs` |
| `InspectNetwork` | `NetworkManager::ensure()` — check whether a Docker network exists before creating it | `crates/builderd/src/container_handler.rs` |

`LaunchWorker` stats every volume host path before calling `docker run` and returns
`FailedPrecondition` if any source is missing. This resolves the namespace-mismatch
problem: the server container cannot reliably validate host paths, but builderd can.

### Client in the Server

`BuilderContainerClient` (`crates/server/src/builder_container_client.rs`) is the thin
clone-able wrapper used by the server. It connects to builderd via the same
`UR_BUILDERD_ADDR` used by `BuilderdClient`.

Proto: `proto/builder_container.proto` (`ur.builder_container` package).

## Two Client Paths

Builderd has two clients in the server, serving different use cases:

### 1. HostExecServiceHandler (worker → server → builderd)

- **Purpose**: Worker commands (git, gh) via the hostexec pipeline
- **Client**: Creates `BuilderDaemonServiceClient` directly per-request
- **CWD source**: `map_working_dir()` replaces `/workspace` prefix with `%WORKSPACE%`
- **File**: `crates/server/src/grpc_hostexec.rs`

### 2. BuilderdClient (server-internal → builderd)

- **Purpose**: Pool git operations (clone, fetch, reset, clean)
- **Client**: Shared `BuilderdClient` wrapper with `exec_and_check()` helper
- **CWD source**: `RepoPoolManager::to_builderd_path()` constructs `%WORKSPACE%/pool/<project>/<slot>`
- **File**: `crates/server/src/builderd_client.rs`

### 3. BuilderContainerClient (server-internal → builderd)

- **Purpose**: Worker container lifecycle (launch, stop, exec, network inspect)
- **Client**: Shared `BuilderContainerClient` wrapper, one method per RPC
- **File**: `crates/server/src/builder_container_client.rs`

## Proto Definitions

### BuilderDaemonService (`proto/builder.proto`, package `ur.builder`)

```protobuf
service BuilderDaemonService {
  rpc Exec(stream BuilderExecMessage) returns (stream ur.core.CommandOutput);
}

message BuilderExecMessage {
  oneof payload {
    BuilderExecRequest start = 1;
    bytes stdin = 2;
  }
}

message BuilderExecRequest {
  string command = 1;
  repeated string args = 2;
  string working_dir = 3;    // Accepts %WORKSPACE% templates
  map<string, string> env = 4;
  bool long_lived = 5;
}
```

### BuilderContainerService (`proto/builder_container.proto`, package `ur.builder_container`)

```protobuf
service BuilderContainerService {
  rpc LaunchWorker(LaunchWorkerRequest) returns (LaunchWorkerResponse);
  rpc StopWorker(StopWorkerRequest) returns (StopWorkerResponse);
  rpc ExecContainer(ExecContainerRequest) returns (ExecContainerResponse);
  rpc InspectNetwork(InspectNetworkRequest) returns (InspectNetworkResponse);
}
```

## Connection Path

```
ur-server container
  → UR_BUILDERD_ADDR env var (e.g., http://host.docker.internal:12323)
    → Docker host gateway resolves to host IP
      → builderd listening on 127.0.0.1:<builderd_port>
```

## Key Files

- `crates/builderd/src/main.rs` — CLI, gRPC server setup (both services)
- `crates/builderd/src/handler.rs` — BuilderDaemonHandler, %WORKSPACE% resolution
- `crates/builderd/src/container_handler.rs` — BuilderContainerHandler (launch/stop/exec/network)
- `crates/server/src/builderd_client.rs` — Shared BuilderdClient (pool git ops)
- `crates/server/src/builder_container_client.rs` — Shared BuilderContainerClient (container ops)
- `crates/server/src/grpc_hostexec.rs` — HostExecServiceHandler (worker commands)
- `proto/builder.proto` — BuilderDaemonService proto
- `proto/builder_container.proto` — BuilderContainerService proto
- `crates/ur/src/builderd.rs` — Host lifecycle (start/stop)
