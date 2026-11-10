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
- **Launch pending (migration 006, `worker.launch_pending`).** Set with the `provisioning` refresh just before the
  Launch RPC (`record_provisioning_worker`), cleared by finalize, or by the launch path once its Launch RPC resolved
  and its own stop was attempted. **While a worker's own Launch RPC is unresolved, only the launch path
  (`abandon_launch` / `finish_launch`, via `finish_launch_stop`) or the server startup pass may finish its stop.**
  Other stoppers and the reconciler still mark it `stopping` and send `Stop(worker_id)`, but `finish_stop` refuses
  (the slot stays linked): their Stop may have reached a builderd that then restarted and lost its tombstone, so its
  Ok does not cover a Launch not yet delivered. Startup clears every marker first (a dead server can no longer deliver
  a Launch), then its pass finishes those stops.

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
current row-based stop path until they stop (`StopWorkerRequest.legacy_container_id`, env-checked by builderd).

Known limits:

- **`--workspace` changed while containers run.** `ur.instance` is builderd's canonical workspace path, so a
  symlink or another spelling is the same instance, but an actual path change makes the old instance's containers
  invisible to List and Stop: they keep running until removed by hand.
- **Upgrade strays.** If the server restarted without `ur stop` across the upgrade, a pre-change `stopped` row whose
  unlabeled container is still alive is never found (not listed, row inactive): the container runs on a freed slot
  until removed by hand. A pre-change `provisioning` row is moved to `stopping` at startup and stopped only through
  its recorded legacy name, after builderd checks the container's `UR_WORKER_ID` env.
- **builderd dies inside `docker run -d`.** The docker CLI can outlive builderd and create the (labeled) container
  after the server's Launch RPC already failed and its Stop on the restarted builderd returned Ok, so the slot is
  free under that container until the next reconcile pass lists it with no active row and stops it (≤ one interval).
- **Legacy `running` rows are skipped.** A pre-change container is never listed, so the reconciler leaves its
  `running` row alone; it runs until stopped through its row (`ur worker stop`, workflow completion).

## Non-goals

New server RPCs; Codex/AGY specifics; durable tombstones; changing the local runtime beyond Launch-not-retried.
