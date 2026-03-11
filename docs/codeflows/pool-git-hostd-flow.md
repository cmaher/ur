# Pool Git Operations via ur-hostd

## Overview

The repo pool manager (`RepoPoolManager`) runs inside the ur-server container, which
has no SSH keys or git credentials. All git operations (clone, fetch, reset, clean)
are routed through ur-hostd, which runs on the host with full credential access.

## Flow

### Acquire Slot (new clone)

1. CLI sends `ProcessLaunch` with `-p <project-key>`
2. ur-server `RepoPoolManager::acquire()`:
   a. Looks up project config (repo URL, pool limit)
   b. Scans existing slots via local filesystem (`read_dir` on container-side path)
   c. No available slot found → `clone_slot()`
3. `clone_slot()`:
   a. Creates parent directory locally (`create_dir_all` on container-side bind mount)
   b. Calls `HostdClient::exec_and_check("git", ["clone", url, slot_path], parent_dir)`
4. `HostdClient` (shared helper):
   a. Connects to ur-hostd gRPC at `http://host.docker.internal:42070`
   b. Sends `HostDaemonExecRequest { command: "git", args, working_dir }`
   c. Streams response, collects stderr, checks exit code
   d. Returns `Ok(())` or `Err(stderr + exit code)`
5. ur-hostd spawns `git clone` on the host with SSH agent access
6. Slot marked in-use, host-side path returned for Docker volume mount

### Acquire Slot (reuse existing)

1. Same scan as above, finds an existing slot not in-use
2. `reset_slot()` runs four sequential hostd commands:
   - `git fetch origin`
   - `git checkout master`
   - `git reset --hard origin/master`
   - `git clean -fd`
3. Each command goes through `HostdClient::exec_and_check()`
4. Slot marked in-use on success

### Release Slot

1. Process stops → `RepoPoolManager::release(slot_path)`
2. Runs `reset_slot()` (same four-command sequence as above)
3. Slot marked available for reuse

## Dual Path Namespaces

The pool manager tracks two path namespaces for each slot:

| Namespace | Example | Used For |
|-----------|---------|----------|
| Local (container) | `/workspace/pool/myproj/0/` | `read_dir`, `create_dir_all` (filesystem ops) |
| Host | `~/.ur/workspace/pool/myproj/0/` | Docker volume mounts, hostd `working_dir` |

Both paths point to the same physical directory via the bind mount between
host `~/.ur/workspace/` and container `/workspace/`.

## Key Files

- `crates/server/src/pool.rs` — `RepoPoolManager` (acquire, release, slot management)
- `crates/server/src/hostd_client.rs` — `HostdClient` (shared hostd exec helper)
- `crates/server/src/grpc_hostexec.rs` — `HostExecServiceHandler` (worker → server → hostd streaming)
- `crates/hostd/src/handler.rs` — ur-hostd exec handler (spawns processes on host)

## Error Propagation

All hostd errors propagate back to the CLI:
hostd spawn failure → gRPC stream error → `exec_and_check` Err → `acquire` Err → gRPC Status → CLI display
