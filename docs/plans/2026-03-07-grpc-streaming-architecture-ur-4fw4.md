# Unified gRPC Streaming Architecture (ur-4fw4)

Replace tarpc with tonic gRPC over UDS. Introduce per-service CLI proxy binaries for workers, feature-gated at compile time.

## Motivation

- **Apple container compatibility:** Apple's `container` runtime runs VMs; UDS cannot cross the VM boundary via `--volume`. The current architecture creates ephemeral side-channel sockets for streaming, requiring a directory mount. Apple's `--publish-socket` proxies a single socket into the VM. Moving to one socket eliminates the problem.
- **Streaming everywhere:** tarpc's `#[tarpc::service]` macro generates request/response methods only. We want server-streaming RPCs for CLI proxy commands (git, temporal) so output arrives in real time.
- **Extensibility:** Future services (jira API, workerd messaging) need a clean plugin model. Per-service protos with cargo feature flags provide compile-time service composition.

## Architecture

### Transport

tonic gRPC over a single Unix domain socket per agent process. HTTP/2 transport handles multiplexing — multiple concurrent streaming RPCs over one connection.

- Docker/nerdctl: socket directory mounted via `--volume`
- Apple: single socket via `--publish-socket`

### Service Categories

**CLI proxy services** (streaming): Transparent command passthrough. Worker sends args, urd validates (blocks dangerous flags like `-C`, `--git-dir`), sets the working directory, execs the real CLI on the host, streams stdout/stderr/exit code back.

- Git: proxies to host `git` in the process's repo directory
- Temporal: proxies to host `temporal` CLI

**API services** (request/response): Structured domain RPCs. Not CLI passthrough.

- Jira: urd makes HTTP API calls, returns structured data
- Core: ping, process launch/stop

**Workerd messaging** (out of scope): urd→worker instruction channel, covered by a separate ticket.

### Proto Structure

One `.proto` per service, stored in `proto/`:

```
proto/
  core.proto      # Ping, process management (always-on)
  git.proto       # Git CLI proxy (feature-gated)
  temporal.proto  # Temporal CLI proxy (feature-gated)
```

Shared streaming message in `core.proto`:

```protobuf
message CommandOutput {
  oneof payload {
    bytes stdout = 1;
    bytes stderr = 2;
    int32 exit_code = 3;
  }
}
```

CLI proxy services import `CommandOutput` and define a single RPC:

```protobuf
service GitService {
  rpc Exec(ExecRequest) returns (stream CommandOutput);
}

message ExecRequest {
  repeated string args = 1;
}
```

Validation errors (unknown command, blocked flags) return gRPC status errors before the stream opens. Execution output flows as streamed `CommandOutput` frames.

### Crate Structure

```
crates/
  ur_rpc/         # Proto codegen via tonic-build, feature-gated per service
  urd/            # gRPC server, feature-gated service handlers
  ur/             # Host CLI, tonic client
  workercmd/
    git/          # -> /usr/local/bin/git in container
    temporal/     # -> /usr/local/bin/temporal in container
    ur-ping/      # -> /usr/local/bin/ur-ping in container
```

### Feature Flags

Cargo features gate compilation across all relevant crates:

- `ur_rpc`: `core` (always-on), `git`, `temporal`
- `urd`: same flags gate which service handlers register on the gRPC server
- Each `workercmd` crate depends on `ur_rpc` with its specific feature

### Worker CLI Binaries

Each proxy service compiles to a standalone binary that replaces the real tool name in the container:

- `crates/workercmd/git/` compiles to a binary named `git`, installed at `/usr/local/bin/git`
- `crates/workercmd/temporal/` compiles to `temporal`
- `crates/workercmd/ur-ping/` compiles to `ur-ping`

Each binary: parse args, connect to `$UR_SOCKET`, call the service RPC, write stdout/stderr chunks in real time, exit with the returned code. Workers see normal CLIs with `--help`.

### Process Identification

Derived from the per-process socket. Each agent process has its own UDS. urd's accept loop knows the process ID from which socket accepted the connection. No env var needed.

### Build Pipeline

`cargo make` builds enabled workercmd binaries into a staging directory. The Dockerfile copies that entire directory:

```dockerfile
COPY workercmd/ /usr/local/bin/
```

Conditionality is in what gets built, not in the Dockerfile. The Dockerfile is static.

## Migration

### Removed

- `tarpc` dependency
- `crates/agent_tools/` — replaced by `crates/workercmd/*`
- `ur_rpc::stream` module (side-channel socket machinery)
- `exec_git`, `exec_git_stream` from the service trait
- Side-channel socket creation in `git_exec.rs`

### Changed

- `ur_rpc` — tonic-build proto codegen replaces tarpc service macro
- `urd` — tonic gRPC server replaces tarpc server; service handlers registered conditionally by feature flag
- `ur` — tonic client replaces tarpc client
- `container` crate — Apple runtime uses `--publish-socket` for the single control socket
- Dockerfile — installs individual workercmd binaries instead of single `agent_tools`

### Kept

- `RepoRegistry`, arg validation logic — reused by the git service handler
- `ProcessManager` — wired to tonic instead of tarpc
- Per-process socket pattern

## Testing

- **Unit:** Each service handler tested in isolation (arg validation, directory resolution). Existing `git_exec` tests migrate to git service handler.
- **Integration:** tonic gRPC server over UDS, client connects, verify streaming round-trip. Replaces `bridge_test.rs`.
- **Acceptance:** Same pattern: launch urd, launch container, run `git status` via proxy binary, verify output. Works on Docker/nerdctl and Apple.
