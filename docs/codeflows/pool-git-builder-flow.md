# Pool Git Operations via builderd

## Overview

The repo pool manager (`RepoPoolManager`) runs inside the ur-server container and
performs only DB orchestration: querying slot availability, inserting new slot rows,
and linking workers to slots. All filesystem and git operations are delegated to
builderd via `BuilderPoolClient`, which calls coarse-grained RPCs on the
`BuilderPoolService`. Builderd runs on the host with full credential and filesystem access.

## Flow

### Acquire Slot (new clone)

1. CLI sends `ProcessLaunch` with `-p <project-key>`
2. `RepoPoolManager::acquire_slot()`:
   a. Looks up project config (repo URL, pool limit) via `ProjectRegistry`
   b. Queries DB for an available slot (`find_available_slot` — no linked active worker)
   c. No available slot found — calls `BuilderPoolClient::scan_slots(project_key)`
      to get existing numeric slot indices on disk
   d. If total on-disk slots >= `pool_limit`, returns error
   e. Computes next slot index (fills gaps, otherwise max+1), calls
      `BuilderPoolClient::prepare_new_slot(project_key, slot_name, repo_url)`
3. `BuilderPoolHandler::prepare_new_slot()` on builderd:
   a. Creates `<workspace>/pool/<project>/<slot_name>/` parent directory
   b. Runs `git clone --filter=blob:none --no-tags --single-branch <repo_url> <slot_name>`
   c. Initializes submodules if `.gitmodules` exists
   d. Trusts mise if `mise.toml` exists
   e. Copies local overlay files from `<config_dir>/projects/<project>/local/` into slot
   f. Returns the absolute host path to the slot directory
4. Server inserts new slot row in DB (`project_key`, `slot_name`, `host_path`)
5. Returns `(host_path, slot_id)` for Docker volume mount and worker-slot linking

### Acquire Slot (reuse existing)

1. Same DB query as above, finds a slot with no linked active worker
2. `RepoPoolManager::acquire_slot()` calls
   `BuilderPoolClient::recycle_slot(project_key, slot_name, repo_url)`
3. `BuilderPoolHandler::recycle_slot()` on builderd:
   a. Attempts `reset_slot()`: `git fetch --no-tags origin`, `git checkout master`,
      `git reset --hard origin/master`, `git clean -fdx`, submodule update
   b. On reset failure, falls back to `reclone_slot()`: removes directory, re-clones
      (retries `rm -rf` up to 3 times to handle macOS Spotlight locks)
   c. Applies local overlay files from `<config_dir>/projects/<project>/local/`
   d. Returns the absolute host path
4. Returns `(host_path, slot_id)` — existing slot row already in DB

### Worker Branch Checkout

After acquiring a slot and generating the worker ID:

1. `RepoPoolManager::checkout_branch(host_slot_path, branch_name)`:
   a. Looks up slot in DB by host path to get `project_key` and `slot_name`
   b. Calls `BuilderPoolClient::checkout_branch(project_key, slot_name, git_branch_prefix, branch_name)`
2. `BuilderPoolHandler::checkout_branch()` on builderd:
   a. Verifies slot directory exists; returns `NotFound` if missing
   b. Runs `git checkout -B <prefix><branch_name>` in the slot
3. Each exclusive worker gets its own branch named `<prefix><worker_id>`

### Shared Slot Acquire

For read-only multi-worker mounts (no pool-limit counting, no DB tracking):

1. `RepoPoolManager::acquire_shared_slot(project_key)`:
   a. Looks up project config (repo URL)
   b. Calls `BuilderPoolClient::prepare_shared_slot(project_key, repo_url)`
2. `BuilderPoolHandler::prepare_shared_slot()` on builderd:
   a. If `<workspace>/pool/<project>/shared/` does not exist: clones (same as new slot)
   b. If it exists: `git fetch --no-tags origin`, `git reset --hard origin/HEAD`,
      submodule update
   c. Returns the absolute host path

### Release Slot

1. Process stops -> `RepoPoolManager::release_slot(worker_id, slot_path)`
2. Looks up slot in DB by host path to get `project_key` and `slot_name`
3. Calls `BuilderPoolClient::clean_slot(project_key, slot_name)`:
   a. `BuilderPoolHandler::clean_slot()` on builderd runs `reset_slot()` (fetch + checkout
      master + reset --hard + clean -fdx + submodule update) without applying local overlays
   b. On failure, logs a warning but does not block the unlink
4. `worker_repo.unlink_worker_slot(worker_id)` removes the worker-slot join row
5. Slot is now available for the next `find_available_slot` query

## Slot Layout

```
<workspace>/pool/
  <project-key>/
    0/          <- exclusive slot (numeric index)
    1/          <- exclusive slot
    shared/     <- shared read-only slot (non-numeric, excluded from find_available_slot)
```

## Key Files

- `crates/server/src/pool.rs` — `RepoPoolManager` (DB orchestration: acquire, release, slot linking)
- `crates/server/src/builder_pool_client.rs` — `BuilderPoolClient` (typed wrappers for 6 RPCs)
- `crates/builderd/src/pool_handler.rs` — `BuilderPoolHandler` (all filesystem/git operations)
- `proto/builder_pool.proto` — `BuilderPoolService` proto definition
- `crates/server/src/grpc_hostexec.rs` — `HostExecServiceHandler` (separate path: worker -> server -> builderd)

## Error Propagation

All builderd errors propagate back to the CLI:
builderd handler error -> gRPC Status -> `BuilderPoolClient` Err(String) ->
`RepoPoolManager` Err(String) -> gRPC Status -> CLI display
