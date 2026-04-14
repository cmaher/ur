# Pool Git Operations via builderd

## Overview

The repo pool manager (`RepoPoolManager`) runs inside the ur-server container, which
has no SSH keys or git credentials. All git operations (clone, fetch, reset, clean)
are routed through builderd, which runs on the host with full credential access.

## Flow

### Acquire Slot (new clone)

1. CLI sends `ProcessLaunch` with `-p <project-key>`
2. ur-server `RepoPoolManager::acquire()`:
   a. Looks up project config (repo URL, pool limit)
   b. Scans existing slots via local filesystem (`read_dir` on container-side path)
   c. No available slot found -> `clone_slot()`
3. `clone_slot()`:
   a. Creates parent directory locally (`create_dir_all` on container-side bind mount)
   b. Calls `BuilderdClient::exec_and_check("git", ["clone", url, slot_path], parent_dir)`
4. `BuilderdClient` (shared helper):
   a. Connects to builderd gRPC at `http://host.docker.internal:12323`
   b. Sends `BuilderDaemonExecRequest { command: "git", args, working_dir }` with `%WORKSPACE%`-prefixed CWD
   c. builderd resolves `%WORKSPACE%` to its local workspace path, streams response
   d. Collects stderr, checks exit code
   e. Returns `Ok(())` or `Err(stderr + exit code)`
5. builderd spawns `git clone` on the host with SSH agent access
6. If `.gitmodules` exists, runs `git submodule update --init --recursive` via builderd
7. Slot marked in-use, `%WORKSPACE%`-prefixed path returned for Docker volume mount

### Acquire Slot (reuse existing)

1. Same scan as above, finds an existing slot not in-use
2. `reset_slot()` runs four sequential builderd commands:
   - `git fetch origin`
   - `git checkout master`
   - `git reset --hard origin/master`
   - `git clean -fdx`
3. Each command goes through `BuilderdClient::exec_and_check()`
4. If `.gitmodules` exists, runs `git submodule update --init --recursive` via builderd
5. Slot marked in-use on success

### Worker Branch Checkout

After acquiring a slot (new clone or reuse) and generating the worker ID, the gRPC
handler calls `RepoPoolManager::checkout_branch()` to create a worker-specific branch:

1. `git checkout -b <worker_id>` via builderd in the slot directory
2. Each worker gets its own branch named after its worker ID (e.g., `myproc-a1b2`)
3. This runs only for pool slots (project-key launches), not workspace mounts

### Release Slot

1. Process stops -> `RepoPoolManager::release(slot_path)`
2. Runs `reset_slot()` (same four-command sequence as above)
3. Slot marked available for reuse

## CWD Construction

The pool manager constructs CWDs using `%WORKSPACE%` template prefix instead of
resolved host paths. This decouples the server from knowing the builder's filesystem
layout:

| Context | CWD Value | Resolved By |
|---------|-----------|-------------|
| Server sends | `%WORKSPACE%/pool/myproj/0/` | -- |
| builderd receives | `%WORKSPACE%/pool/myproj/0/` | Replaces `%WORKSPACE%` with local workspace path |
| Local filesystem | `/workspace/pool/myproj/0/` | Container bind mount for `read_dir`, `create_dir_all` |

## Key Files

- `crates/server/src/pool.rs` -- `RepoPoolManager` (acquire, release, slot management)
- `crates/server/src/builderd_client.rs` -- `BuilderdClient` (shared builderd exec helper)
- `crates/server/src/grpc_hostexec.rs` -- `HostExecServiceHandler` (worker -> server -> builderd streaming)
- `crates/builderd/src/handler.rs` -- builderd exec handler (spawns processes on host)

## Error Propagation

All builderd errors propagate back to the CLI:
builderd spawn failure -> gRPC stream error -> `exec_and_check` Err -> `acquire` Err -> gRPC Status -> CLI display
