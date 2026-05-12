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
├── RepoPoolManager (pool slot operations)         │
│   └── BuilderPoolClient (6 pool RPCs) ──────────┤
│                                                  │
├── WorkerManager / NetworkManager / SquidManager  │
│   └── BuilderContainerClient ───────────────────┤
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

## BuilderPoolService

Builderd hosts a third gRPC service, `BuilderPoolService`, that owns all pool slot
filesystem and git operations. The server only performs DB orchestration (availability
queries, slot row inserts/deletes, worker-slot linking); builderd does the actual
cloning, resetting, and cleaning on the host filesystem.

### RPCs

| RPC | Purpose | Handler |
|---|---|---|
| `ScanSlots` | Scan the pool directory, return numeric slot indices | `crates/builderd/src/pool_handler.rs` |
| `PrepareNewSlot` | Clone a repo into a fresh slot directory | `crates/builderd/src/pool_handler.rs` |
| `RecycleSlot` | Fetch + reset an existing slot (reclone on failure) | `crates/builderd/src/pool_handler.rs` |
| `PrepareSharedSlot` | Clone or refresh the shared (read-only) slot | `crates/builderd/src/pool_handler.rs` |
| `CheckoutBranch` | Create a worker-specific branch in a slot | `crates/builderd/src/pool_handler.rs` |
| `CleanSlot` | Fetch + reset a slot (without local overlay) before reuse | `crates/builderd/src/pool_handler.rs` |

`PrepareNewSlot` and `RecycleSlot` both apply local overlay files from
`<config_dir>/projects/<project>/local/` after each git operation.
`CleanSlot` does not apply local overlays — it only resets to clean state.

Pool slots live at `<workspace>/pool/<project-key>/<slot-name>/`.
The `shared` slot is a special non-numeric slot for read-only multi-worker mounts.

### Client in the Server

`BuilderPoolClient` (`crates/server/src/builder_pool_client.rs`) is the thin
clone-able wrapper used by `RepoPoolManager`. It connects to builderd via the same
`UR_BUILDERD_ADDR` used by the other builderd clients.

Proto: `proto/builder_pool.proto` (`ur.builder_pool` package).

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

## Client Paths

Builderd has three clients in the server, serving different use cases:

### 1. HostExecServiceHandler (worker → server → builderd)

- **Purpose**: Worker commands (git, gh) via the hostexec pipeline
- **Client**: Creates `BuilderDaemonServiceClient` directly per-request
- **CWD source**: `map_working_dir()` replaces `/workspace` prefix with `%WORKSPACE%`
- **File**: `crates/server/src/grpc_hostexec.rs`

### 2. BuilderPoolClient (server-internal → builderd)

- **Purpose**: Pool slot lifecycle (clone, fetch/reset, clean, branch checkout)
- **Client**: Shared `BuilderPoolClient` wrapper with six typed async methods, one per RPC
- **Used by**: `RepoPoolManager` in `crates/server/src/pool.rs`
- **File**: `crates/server/src/builder_pool_client.rs`

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

### BuilderPoolService (`proto/builder_pool.proto`, package `ur.builder_pool`)

```protobuf
service BuilderPoolService {
  rpc ScanSlots(ScanSlotsRequest) returns (ScanSlotsResponse);
  rpc PrepareNewSlot(PrepareNewSlotRequest) returns (PrepareNewSlotResponse);
  rpc RecycleSlot(RecycleSlotRequest) returns (RecycleSlotResponse);
  rpc PrepareSharedSlot(PrepareSharedSlotRequest) returns (PrepareSharedSlotResponse);
  rpc CheckoutBranch(CheckoutBranchRequest) returns (CheckoutBranchResponse);
  rpc CleanSlot(CleanSlotRequest) returns (CleanSlotResponse);
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

- `crates/builderd/src/main.rs` — CLI, gRPC server setup (all three services)
- `crates/builderd/src/handler.rs` — BuilderDaemonHandler, %WORKSPACE% resolution
- `crates/builderd/src/container_handler.rs` — BuilderContainerHandler (launch/stop/exec/network)
- `crates/builderd/src/pool_handler.rs` — BuilderPoolHandler (clone/reset/clean/checkout slots)
- `crates/server/src/builderd_client.rs` — Shared BuilderdClient (worker hostexec helper)
- `crates/server/src/builder_container_client.rs` — Shared BuilderContainerClient (container ops)
- `crates/server/src/builder_pool_client.rs` — Shared BuilderPoolClient (pool slot ops)
- `crates/server/src/pool.rs` — RepoPoolManager (DB orchestration, delegates to BuilderPoolClient)
- `crates/server/src/grpc_hostexec.rs` — HostExecServiceHandler (worker commands)
- `proto/builder.proto` — BuilderDaemonService proto
- `proto/builder_container.proto` — BuilderContainerService proto
- `proto/builder_pool.proto` — BuilderPoolService proto
- `crates/ur/src/builderd.rs` — Host lifecycle (start/stop)
