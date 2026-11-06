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

`PrepareNewSlot` and `RecycleSlot` both apply local overlay files from
`<config_dir>/projects/<project>/local/` after each git operation.
Releasing a slot is DB-only (unlink); the next acquire's `RecycleSlot` resets it.

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

## BuilderLocalWorkerService and the routed WorkerDaemonService

Builderd also hosts host-local workers (`runtime = "local"`): the agent runs natively in a
pool slot inside a tmux session `ur-<id>` on builderd's dedicated socket
(`tmux -L ur`, `ur_config::LOCAL_TMUX_SOCKET`), and builderd serves each worker's
`WorkerDaemonServiceImpl` (workerd lib) in-process.

| RPC | Purpose | Handler |
|---|---|---|
| `LaunchLocalWorker` | Build the drive, run startup hooks, start the session and agent | `crates/builderd/src/local_worker_handler.rs` |
| `StopLocalWorker` | Teardown; unknown/already-stopped ids succeed | `crates/builderd/src/local_worker_handler.rs` |
| `ListLocalWorkers` | Registered ids with tmux session liveness | `crates/builderd/src/local_worker_handler.rs` |

`LocalWorkerManager::launch` (`crates/builderd/src/local_worker.rs`):

```
InvalidArgument if the id contains '.' or ':' (tmux would rename the session) or the
    request env lacks UR_SERVER_ADDR / UR_WORKER_SECRET
  → AlreadyExists if the id is registered or its session exists (live worker untouched)
  → LocalWorkspaceManager.build: agent check (supports_local_runtime, else FailedPrecondition
    before any fs work), then drive, memory link, .ur/ customization; self-cleans
  → sync startup hooks in the slot (failure → FailedPrecondition with hook stderr)
  → tmux -L ur new-session -d -s ur-<id> -c <slot> -e <env…>
  → background startup hooks in window :bg (new-window -c <slot>)
  → LocalWorkerRegistry insert + exit watcher   (before the agent: SessionStart calls NotifyIdle)
  → agent argv typed into window 0
any failure after build → teardown → error
```

Session env: the request env (the server supplies `UR_WORKER_SECRET`,
`UR_SERVER_ADDR=127.0.0.1:<worker_port>`, `UR_PROJECT`, `UR_WORKER_FORGE`) followed by
builderd's own `UR_WORKER_ID`, `UR_WORKERD_ADDR`, `UR_AGENT_TYPE`, `UR_DRIVE`,
`UR_RUNTIME=local` and (when set) `UR_WORKER_INSTRUCTION_STRATEGY`, which the request cannot
override. Adoption reads them back. The drive's `.ur/owner` records the agent, slot path and
memory link target; teardown removes the slot memory symlink only while it still points at
that target, and an orphan drive whose slot a registered worker uses keeps the link. builderd derives `UR_WORKERD_ADDR`
from its own listen address (an unspecified bind maps to `127.0.0.1`), because on Linux it
binds the Docker bridge IP, which the server cannot know.

Skills: each requested skill is installed from its `host_skills` entry (the server's host
`[skills]` paths) when present, else `<agent_shared>/potential-skills/<name>` — host shadows
baked, as in containers. The drive's `.ur/skill-sources/potential-skills/` holds one link per
skill to its resolved source, and `check_assets` checks the same resolution.

### Server side

`WorkerManager::run_and_record` branches on `WorkerConfig.runtime`; the local path lives in
`crates/server/src/local_launch.rs` (`build_local_launch_request`). A pool launch's
`provisioning` row and slot link were inserted when the slot was claimed (see
`pool-git-builder-flow.md#slot-claim-ur-j5puq`); `record_provisioning_worker` fills in the
session name `ur-<id>` and the secret → `LaunchLocalWorker` → finalize (`provisioning →
running`, compare-and-set). A failed launch is stopped like any worker
(`WorkerManager::abandon_launch` → `stop_by_worker_id`; both runtimes), leaving a `stopped`
row, or `stopping` (still holding the slot) if builderd's Stop fails. If finalize finds the row
no longer `provisioning`, a stop landed mid-launch and its stopper finishes it; the launch just
returns "stopped during launch". No Docker network, Squid env, image or credential gate.
Mounts, memory/brain dirs, instruction file and host skills are host paths (templates resolve
against the host config dir).

Stop (`stop_by_worker_id`, both runtimes): `begin_stopping` (active → `stopping`), builderd
Stop (`StopLocalWorker`; container: `StopWorker` with the worker id), then `finish_stop`
(`stopping → stopped` + delete this worker's own link; a no-op for an already `stopped` row)
only on Ok. On failure the row stays `stopping`. Release never touches slot contents; the next
owner's `RecycleSlot` resets them. Client: `BuilderLocalWorkerClient`
(`crates/server/src/builder_local_worker_client.rs`).

### Stop is a barrier (ur-j5puq)

builderd orders Launch and Stop per worker id (`barrier.rs`, `WorkerBarrier`), in both
`LocalWorkerManager` and the container service (`LaunchWorker`/`StopWorker` carry `worker_id`).
Stop first records an in-memory tombstone for the id, then takes the id's lock, waiting for any
in-flight launch, then tears down (local: including a drive an unregistered id left; container:
the requested container and the one that launch started). A launch takes the lock and is refused
with `FailedPrecondition` if the id is tombstoned. Launches run on their own task, so a cancelled
RPC cannot abandon one halfway. Contract: Stop(id) returning Ok means the id is not running in
this builderd and never will be. Tombstones expire after an hour (`TOMBSTONE_TTL`); a builderd
restart drops them safely (in-flight launches die with it, and worker ids are never reused). A
`StopWorker` without `worker_id` (pre-launch name cleanup, orphan removal) is a plain stop.

### Termination paths and reconciliation (ur-j5puq)

Invariant (builderd): after startup, for sessions whose `UR_DRIVE` is under this builderd's
workspace (`<workspace>/drive/`), a `ur-<id>` session exists only while `<id>` is registered,
so `ListLocalWorkers` never omits one of its live sessions. Every ur instance on the host
shares the `ur` socket; a session is ours only if its `UR_DRIVE` is set and under our drive
root (`owns_session`). Any other `ur-*` session (another instance's, or one made by hand with
no `UR_DRIVE`) is never adopted, killed or listed. Wrongly releasing a live worker's slot would
let the next worker reset it and destroy uncommitted work, so the server releases a local slot only after
`StopLocalWorker` succeeded (also when builderd no longer lists the id).

Teardown (stop, failed launch, agent exit, external kill): kill the session (abort the exit
watcher unless it is the caller) → `LocalWorkspaceManager.teardown` → deregister. A failed
step stops there and the worker stays registered (listed, `session_alive` false once killed),
so the server's retried Stop runs teardown again.

| Trigger | Path |
|---|---|
| Agent exit, external `kill-session` | exit watcher (pane dead, probe failure, or — code/design workers only — the agent's pane back at a shell) → exit sink → teardown; the server sees the id leave the list on its next pass. A manual worker whose agent quits keeps its shell and stays until stopped |
| builderd restart / host reboot | `adopt_existing` before `serve`: adopt or kill each of our `ur-<id>` sessions, skip any that are not ours, tear down orphan drives (below) |
| Server start (before serving or dispatching) | `LocalWorkerReconcileManager::startup`: every local `provisioning` row (a launch the previous server never finished, slot still linked) becomes `stopping`; then one pass |
| Every 15s | `LocalWorkerReconcileManager` pass (below) |
| `ur stop` | `ListLocalWorkers` → `StopLocalWorker` each from the host, before `compose down` and again after it; a failure aborts `ur stop` |
| `ur start` | waits for builderd's port; reports its log paths if builderd exits (e.g. failed adoption) |

**Adoption** (`LocalWorkerManager::adopt_existing`, in `main.rs` before `serve`): each
`ur-<id>` session is re-registered from its env (`UR_WORKER_ID` must match, `UR_SERVER_ADDR`,
`UR_WORKER_SECRET`, a local-capable `UR_AGENT_TYPE`, `UR_DRIVE` = `drive/<id>`) and start dir
(`#{session_path}` = slot), with a new exit watcher. A session whose `UR_DRIVE` is missing or
outside this builderd's drive root is not ours and is skipped. One of ours with unusable env is
killed (a failed kill aborts startup); a tmux error reading a live session aborts startup (a
session that ended meanwhile is skipped). Then every `drive/<id>` without a registered worker
gets `teardown_orphan` (links only). Listing failure aborts startup; no tmux installed means no
sessions. Orphan drive failures are logged only (known limit: drives hold only links; the next
startup retries).

**Reconcile pass** (`crates/server/src/local_reconcile.rs`): read active local rows, then
`ListLocalWorkers`; a list error changes nothing. `running` + listed alive → none; `running`
not listed, `running` + session dead, or `stopping` → `stop_by_worker_id`; `provisioning` →
none; a listed id with no active row (re-read first) → `StopLocalWorker`.
`WorkerRepo::reconcile_workers` (container restart reconciliation) skips local rows.

The routed `WorkerDaemonService` (`crates/builderd/src/workerd_router.rs`, `WorkerdRouter`)
reads the `ur-worker-id` header (`ur_config::WORKER_ID_HEADER`) and delegates to that
worker's daemon: missing header → `InvalidArgument`, unknown id → `NotFound`. It adds no auth;
builderd is unauthenticated and already exposes `Exec`.

Agent assets come from `--agent-shared` (default `$UR_CONFIG/agent-shared`), which
`scripts/build/install.sh` fills in the same order as `containers/worker-base/Dockerfile`.

Proto: `proto/builder_local_worker.proto` (`ur.builder_local_worker` package).

## Client Paths

Builderd has three clients in the server, serving different use cases:

### 1. HostExecServiceHandler (worker → server → builderd)

- **Purpose**: Worker commands (git, gh) via the hostexec pipeline
- **Client**: Creates `BuilderDaemonServiceClient` directly per-request
- **CWD source**: `map_working_dir()` replaces `/workspace` prefix with `%WORKSPACE%`
- **File**: `crates/server/src/grpc_hostexec.rs`

### 2. BuilderPoolClient (server-internal → builderd)

- **Purpose**: Pool slot lifecycle (clone, fetch/reset, branch checkout)
- **Client**: Shared `BuilderPoolClient` wrapper with five typed async methods, one per RPC
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
}
```

### BuilderLocalWorkerService (`proto/builder_local_worker.proto`, package `ur.builder_local_worker`)

```protobuf
service BuilderLocalWorkerService {
  rpc LaunchLocalWorker(LaunchLocalWorkerRequest) returns (LaunchLocalWorkerResponse);
  rpc StopLocalWorker(StopLocalWorkerRequest) returns (StopLocalWorkerResponse);
  rpc ListLocalWorkers(ListLocalWorkersRequest) returns (ListLocalWorkersResponse);
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

- `crates/builderd/src/main.rs` — CLI, gRPC server setup (all five services)
- `crates/builderd/src/handler.rs` — BuilderDaemonHandler, %WORKSPACE% resolution
- `crates/builderd/src/container_handler.rs` — BuilderContainerHandler (launch/stop/exec/network)
- `crates/builderd/src/pool_handler.rs` — BuilderPoolHandler (clone/reset/clean/checkout slots)
- `crates/builderd/src/local_worker.rs` — LocalWorkerManager, LocalWorkerRegistry (local worker lifecycle)
- `crates/builderd/src/local_worker_handler.rs` — BuilderLocalWorkerHandler
- `crates/builderd/src/workerd_router.rs` — WorkerdRouter (routed WorkerDaemonService)
- `crates/server/src/builderd_client.rs` — Shared BuilderdClient (worker hostexec helper)
- `crates/server/src/builder_container_client.rs` — Shared BuilderContainerClient (container ops)
- `crates/server/src/builder_pool_client.rs` — Shared BuilderPoolClient (pool slot ops)
- `crates/server/src/pool.rs` — RepoPoolManager (DB orchestration, delegates to BuilderPoolClient)
- `crates/server/src/grpc_hostexec.rs` — HostExecServiceHandler (worker commands)
- `proto/builder.proto` — BuilderDaemonService proto
- `proto/builder_container.proto` — BuilderContainerService proto
- `proto/builder_pool.proto` — BuilderPoolService proto
- `proto/builder_local_worker.proto` — BuilderLocalWorkerService proto
- `crates/ur/src/builderd.rs` — Host lifecycle (start/stop)
