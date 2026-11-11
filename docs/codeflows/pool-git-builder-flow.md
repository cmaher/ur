# Pool Git Operations via builderd

## Overview

The repo pool manager (`RepoPoolManager`) runs inside the ur-server container and
performs only DB orchestration: querying slot availability, inserting new slot rows,
and linking workers to slots. All filesystem and git operations are delegated to
builderd via `BuilderPoolClient`, which calls coarse-grained RPCs on the
`BuilderPoolService`. Builderd runs on the host with full credential and filesystem access.

**Local projects never reach the pool.** A project declared `local = true` in
`ur.toml` has no `repo` to clone, so `RepoPoolManager::resolve_pool_project()`
refuses it at the top of `acquire_slot`, `acquire_shared_slot`, and
`prepare_shared_slot` — before any DB query or builderd RPC, so no half-prepared
slot directory is left behind. `ur worker launch` and the server's
`LaunchManager` reject such launches even earlier. See
[config.md](config.md#local-projects).

## Flow

### Acquire Slot (new clone)

1. CLI sends `ProcessLaunch` with `-p <project-key>`
2. `RepoPoolManager::acquire_slot()`:
   a. `resolve_pool_project()` looks up project config via `ProjectRegistry` and
      resolves the repo URL, erroring for unknown keys and for local projects
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
4. Server inserts new slot row in DB (`project_key`, `slot_name`, `host_path`) and claims it
   (see "Slot claim" below)
5. Returns `(host_path, claimed worker row)`

### Acquire Slot (reuse existing)

1. Same DB query as above, finds candidate slots with no linked active worker
2. Claims the first candidate it can (see "Slot claim"); a lost claim moves on to the next
3. `RepoPoolManager::acquire_slot()` calls
   `BuilderPoolClient::recycle_slot(project_key, slot_name, repo_url)` under the claim;
   a failed recycle deletes the claim row
4. `BuilderPoolHandler::recycle_slot()` on builderd:
   a. Attempts `reset_slot()`: `git fetch --no-tags origin`, `git checkout master`,
      `git reset --hard origin/master`, `git clean -fdx`, submodule update
   b. On reset failure, falls back to `reclone_slot()`: removes directory, re-clones
      (retries `rm -rf` up to 3 times to handle macOS Spotlight locks)
   c. Applies local overlay files from `<config_dir>/projects/<project>/local/`
   d. Returns the absolute host path
5. Returns `(host_path, claimed worker row)` — existing slot row already in DB

### Slot claim (ur-j5puq)

The database is the only source of slot ownership. `acquire_slot(project_key, owner)` builds
the launch's `provisioning` worker row for each candidate slot (`owner(slot_path)`: a Manual
process ID is named after the slot) and `WorkerRepo::claim_slot` inserts that row and its
`worker_slot` link in one transaction. `UNIQUE (slot_id)` on `worker_slot` (migration 005)
makes the insert the arbiter: a unique violation means another launch got the slot. A link
left by a `stopped` worker owns nothing and is dropped inside the same transaction. There is no
in-memory claim; the launch RPC runs on its own task, so a dropped client cannot leave a claim
half-done. Every later transition is a compare-and-set: finalize `provisioning → running`,
stop `→ stopping`, `WorkerRepo::finish_stop` `stopping → stopped` plus deleting the link.
Nothing else moves a row to `running`, and nothing else unlinks a slot (ur-ezhdb): the
reconciler converges rows on builderd's lists only through these transitions.

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

1. The stop is confirmed (builderd `StopWorker(worker_id)` / `StopLocalWorker(worker_id)`
   returned Ok: nothing with that worker id runs in builderd, now or later) ->
   `WorkerRepo::finish_stop(worker_id)` marks the row `stopped` and deletes only this worker's
   worker-slot row, in one transaction
2. Slot is now available for the next `find_available_slot` query

Release never touches the slot's contents (ur-j5puq): the next `acquire_slot` recycles it
(`RecycleSlot`: fetch + reset) under the new owner's claim. A stale or concurrent release can
therefore never reset a slot another worker already holds.

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
- `crates/server/src/builder_pool_client.rs` — `BuilderPoolClient` (typed wrappers for 5 RPCs)
- `crates/builderd/src/pool_handler.rs` — `BuilderPoolHandler` (all filesystem/git operations)
- `proto/builder_pool.proto` — `BuilderPoolService` proto definition
- `crates/server/src/grpc_hostexec.rs` — `HostExecServiceHandler` (separate path: worker -> server -> builderd)

## Error Propagation

All builderd errors propagate back to the CLI:
builderd handler error -> gRPC Status -> `BuilderPoolClient` Err(String) ->
`RepoPoolManager` Err(String) -> gRPC Status -> CLI display
