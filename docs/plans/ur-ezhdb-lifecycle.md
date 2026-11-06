# ur-ezhdb: worker lifecycle — desired state in the DB, identity by worker id, one reconciler

Ticket: ur-ezhdb (folds in ur-rkufm). Parent epic: ur-wdfir.

## Principle

The database is the single source of truth for **what should exist**. builderd is the only component that touches
external objects (containers, tmux sessions); it identifies every object **by worker id** and can report **what does
exist**. One server-side reconciler compares the two and converges. RPC results are hints for speed, never proof:
correctness comes from identity + observation.

## 1. Database (desired state)

- `worker.container_status`: `provisioning → running → stopping → stopped`. Every transition is a conditional UPDATE
  (compare-and-set). Nothing moves a row back to `running` except finalize (`provisioning → running`).
- `worker_slot` link = the slot claim. Inserted with the `provisioning` row in one transaction; `UNIQUE (slot_id)`
  (migration 005) means one owner per slot; the insert decides who gets a slot.
- A slot is unlinked only by `finish_stop` (`stopping → stopped` + unlink, one transaction), and only after builderd
  `Stop(worker_id)` returned Ok.

## 2. builderd (executor, keyed only by worker id)

Every external object carries the worker id and this ur instance's marker:

| Runtime | Identity |
|---|---|
| local | tmux session `ur-<worker_id>` on socket `ur`; `UR_DRIVE` under this builderd's drive root |
| container | name `ur-worker-<worker_id>`; labels `ur.worker_id=<worker_id>`, `ur.instance=<instance id>` |

Operations (same contract for both runtimes; callers never pass container names, Docker ids or session names):

```
Launch(worker_id, spec)   // per-id lock (WorkerBarrier); refused if worker_id is tombstoned
Stop(worker_id)           // tombstone, wait for any in-flight Launch, remove everything owned by worker_id.
                          // Ok ⇒ nothing with this id runs in this builderd, now or later.
List() -> [(worker_id, runtime, alive)]   // every object owned by this instance
```

Tombstones are in builderd memory (1h TTL). A builderd restart loses them safely because Launch RPCs are never resent
(below) and List finds anything a dying builderd left behind.

### Proto changes

```proto
// builder_container.proto
rpc ListWorkerContainers(ListWorkerContainersRequest) returns (ListWorkerContainersResponse);
message ListWorkerContainersResponse { repeated WorkerContainerStatus containers = 1; }
message WorkerContainerStatus { string worker_id = 1; bool running = 2; }
// LaunchWorkerRequest.worker_id: required; container name derived from it.
// StopWorkerRequest: keyed by worker_id; container_id removed.
```

Local already has `ListLocalWorkers` / `StopLocalWorker(worker_id)` with these semantics.

## 3. Server

**Launch:** claim (row + link, one txn) → `Launch` (no automatic retry) → finalize CAS. If Launch fails or finalize
changes no row: `Stop(worker_id)` → `finish_stop` on Ok, else the row stays `stopping`.

**Stop (any caller):** `begin_stopping` CAS → `Stop(worker_id)` → `finish_stop` on Ok, else stays `stopping`.

**Reconciler** (`WorkerReconcileManager`): at startup before serving, then every 15s; one `List()` per runtime per pass;
one table for both runtimes:

| DB row | builderd List | Action |
|---|---|---|
| `running` | listed alive | none |
| `running` | absent or dead | stop path |
| `stopping` | any | `Stop(worker_id)` → `finish_stop` on Ok |
| `provisioning` | any | periodic: none · startup: → `stopping` |
| no active row (none / `stopped`) | listed | `Stop(worker_id)` |
| — | List failed | no change this pass |

## Why this closes the failure classes found in review

| Class | Closed by |
|---|---|
| stop hits another worker (reused names) | identity by worker id; worker ids are unique per launch |
| lost launch after builderd restart | Launch never resent; barrier refuses Launch after Stop; List finds leftovers |
| leaked container / session | List sweep: no active row ⇒ Stop |
| revived rows, slot free under a live worker | only finalize sets `running`; unlink only after Stop Ok |
| concurrent stoppers | per-id lock in builderd (where the objects live); CAS in the DB (where the state lives) |

## Files to change

- `proto/builder_container.proto` — ListWorkerContainers; Stop by worker_id.
- `crates/builderd/src/container_handler.rs` — name + labels from worker id; List by label (instance-scoped); Stop by
  label under the barrier; drop the `launched` map if Stop-by-label makes it redundant.
- `crates/server/src/worker_reconcile.rs` — the table above, both runtimes.
- `crates/server/src/worker.rs`, `local_launch.rs` — launch/stop as above; delete stray-container, Docker-id ownership,
  stop-by-name and lost-launch special cases.
- `crates/workflow_db/src/worker_repo.rs` — container startup reconcile by inspect is replaced by the reconciler;
  remove what becomes dead.
- `crates/server/src/workerd_address.rs` — Docker DNS name from worker id.
- `crates/ur/src/attach.rs` — `docker exec` target from worker id.
- `crates/ur_rpc` (retry config) — exempt `LaunchWorker` / `LaunchLocalWorker` from automatic retry.
- Docs: `docs/codeflows/builderd.md`, `docs/codeflows/local-workers.md`, `docs/codeflows/pool-git-builder-flow.md`,
  crate `CLAUDE.md` files.

## Upgrade

Containers launched before this change have process-id names and no labels. They are not listed; their rows keep the
current row-based stop path until they stop. Documented as a known limit.

## Non-goals

New server RPCs; Codex/AGY specifics; durable tombstones; changing the local runtime beyond Launch-not-retried.
