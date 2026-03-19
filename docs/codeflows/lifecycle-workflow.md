# Lifecycle Workflow Engine

## Overview

The lifecycle workflow system drives tickets through an automated state machine: dispatch, implement, verify, push, review, and merge. It consists of three cooperating subsystems spawned at server boot:

1. **WorkflowEngine** -- polls `workflow_event` table and dispatches to registered handlers
2. **GithubPollerManager** -- polls GitHub for CI status and review signals
3. **LifecycleStepRouter** -- pure-function router mapping `(lifecycle_status, agent_status)` to actions

## State Machine

```
                    CLI dispatch
                         |
                         v
  Open ──────> AwaitingDispatch ──────> Implementing ──────> Verifying
                 (worker idle              ^    ^               |
                  triggers)                |    |               |
                                           |    └───────────────┘
                                           |              (hook failure,
                                           |               under limit)
                                           |
                                           |                    |
                                           |                    v (hook passes)
                                           |                 Pushing
                                           |                 |    |
                                           |    (CI failure) |    | (CI green)
                                           |  ───────────────┘    v
                                           |                   InReview
                                           |                      |
                                           |                      v
                                           |               FeedbackCreating
                                           |                /           \
                                           |     (mode=now)/             \(mode=later)
                                           |              /               \
                                           └─────────────┘               Merging
                                                                        /      \
                                                              (conflict)/        \(success)
                                                  ──────────────────────┘         v
                                                  → Implementing                Done
```

### Lifecycle States

| Status | Description |
|--------|-------------|
| `Design` | Pre-implementation planning (manual) |
| `Open` | Ready for work, not yet dispatched |
| `AwaitingDispatch` | CLI has dispatched; waiting for worker to become idle |
| `Implementing` | Worker is actively implementing the ticket |
| `Verifying` | Server runs pre-push verification hook |
| `Pushing` | Server pushes branch and creates/updates PR |
| `InReview` | PR is open, waiting for human review signal |
| `FeedbackCreating` | Worker creates feedback summary from review |
| `Merging` | Server merges PR (squash), kills worker, closes epic, dispatches children |
| `Done` | Terminal state |

### Registered Transitions (Handler Registry)

| From | To | Handler | Description |
|------|----|---------|-------------|
| Open | AwaitingDispatch | `AwaitingDispatchHandler` | No-op; acknowledges dispatch |
| AwaitingDispatch | Implementing | `DispatchImplementHandler` | Sends implement RPC to worker (with /clear) |
| Implementing | Verifying | `VerifyHandler` | Runs pre-push verification hook |
| Verifying | Pushing | `PushHandler` | Pushes branch, creates/updates PR |
| Pushing | InReview | `ReviewStartHandler` | No-op signal handler |
| Pushing | Implementing | `DispatchImplementHandler` | CI failure detected by poller |
| InReview | FeedbackCreating | `FeedbackCreateHandler` | Promotes to epic, sends feedback create RPC |
| FeedbackCreating | Merging | `MergeHandler` | Merges PR (squash), kills worker, closes epic |
| Merging | Implementing | `DispatchImplementHandler` | Merge conflict during PR merge |

Source: `crates/server/src/workflow/handlers/mod.rs` (`build_handlers()`)

## Server Boot

At startup, `ur-server` spawns both the workflow engine and GitHub poller as background tokio tasks sharing a `watch::Receiver<bool>` shutdown channel:

```
ur-server main()
├── WorkflowEngine::new(ticket_repo, worker_repo, worker_prefix, builderd_client, config, build_handlers())
│   └── engine.spawn(shutdown_rx)   // polls workflow_event table every 500ms
│
└── GithubPollerManager::new(ticket_repo, builderd_client)
    └── poller.spawn(shutdown_rx)   // scans pushing/in_review tickets every 30s
```

Both tasks run until the shutdown channel signals `true`, then exit gracefully.

Source: `crates/server/src/main.rs`

## Dispatch Flow (CLI to AwaitingDispatch)

When the user runs `ur launch --dispatch <ticket-id>`:

1. `dispatch_ticket()` validates the ticket exists and is in `open` lifecycle status
2. Transitions lifecycle_status to `awaiting_dispatch` (not `implementing`)
3. This fires the SQLite trigger, creating a `workflow_event` (Open -> AwaitingDispatch)
4. The `AwaitingDispatchHandler` processes the event as a no-op and deletes it
5. The ticket remains in `awaiting_dispatch` until a worker reports idle

Source: `crates/ur/src/main.rs` (`dispatch_ticket()`)

## Worker Readiness Flow (AwaitingDispatch to Implementing)

When a worker container starts and its agent becomes idle for the first time:

1. Worker calls `update_agent_status` RPC with `status: "idle"`
2. The gRPC handler on the **worker server** validates the agent status string by parsing it into `AgentStatus` enum (Starting, Idle, Working, Stalled)
3. Updates the worker's `agent_status` in the database
4. Looks up the worker's assigned ticket via `worker_id` metadata (set during `WorkerLaunch` RPC)
5. If the ticket's lifecycle_status is `AwaitingDispatch` and agent_status is `"idle"`:
   - Transitions lifecycle_status to `Implementing`
   - This fires the SQLite trigger, creating a `workflow_event` (AwaitingDispatch -> Implementing)
   - The `DispatchImplementHandler` sends /clear then the implement RPC to the worker via workerd
6. If the ticket is in any other lifecycle state, the `LifecycleStepRouter` is consulted

Source: `crates/server/src/grpc.rs` (`handle_agent_status_routed()`)

## Dispatch Implement Handler

The `DispatchImplementHandler` is the single entry point for all transitions into `Implementing`. It covers:

- **Initial dispatch**: AwaitingDispatch -> Implementing
- **CI failure re-dispatch**: Pushing -> Implementing (via GitHub poller)
- **Merge conflict re-dispatch**: Merging -> Implementing
- **Verification failure re-dispatch**: Verifying -> Implementing (via VerifyHandler internal transition)

The handler ensures the ticket has a branch (generating one from the ticket ID if needed), resolves the assigned worker via `worker_id` metadata, and sends the `Implement(ticket_id)` RPC to the worker's workerd daemon. The workerd handler sends `/clear` before `/implement` to ensure a fresh agent context on each dispatch.

Source: `crates/server/src/workflow/handlers/dispatch_implement.rs`

## Worker ID Metadata

During `WorkerLaunch` RPC, the server sets `worker_id` as ticket metadata:

```
ticket_repo.set_meta(&ticket_id, "ticket", "worker_id", &worker_id_str)
```

This binding is used later by `handle_agent_status_routed()` to look up which ticket a worker is assigned to when it reports status changes.

Source: `crates/server/src/grpc.rs` (WorkerLaunch handler)

## AgentStatus Enum

Agent status is validated via the `AgentStatus` enum rather than raw strings:

| Variant | Wire value | Description |
|---------|-----------|-------------|
| `Starting` | `"starting"` | Worker process initializing (default) |
| `Idle` | `"idle"` | Agent is idle, ready for work |
| `Working` | `"working"` | Agent is actively executing |
| `Stalled` | `"stalled"` | Agent has stalled (no progress) |

The `update_agent_status` gRPC handler parses the string into `AgentStatus` via `FromStr`, rejecting unknown values with `Status::invalid_argument`.

Source: `crates/ur_db/src/model.rs`

## LifecycleStepRouter

The step router is a pure-function mapper from `(lifecycle_status, agent_status)` to `StepAction`:

| Action | Behavior |
|--------|----------|
| `Advance { to }` | Transition ticket to the next lifecycle state |
| `AdvanceByFeedbackMode` | Route based on `feedback_mode` metadata (`now` -> Implementing, `later` -> Merging) |
| `Redispatch { reminder }` | Re-send the phase-appropriate RPC to the worker |
| `Ignore` | No action |

Key routing rules:
- **No ticket assigned**: always `Ignore`
- **Open / AwaitingDispatch**: always `Ignore` (handled by dedicated logic in grpc.rs)
- **Stalled**: always `Ignore`
- **Working**: `Redispatch { reminder: true }` for all active states
- **Idle + Implementing**: `Advance { to: Verifying }`
- **Idle + Pushing**: `Redispatch { reminder: false }`
- **Idle + FeedbackCreating**: `AdvanceByFeedbackMode`
- **Idle + other**: `Ignore`

Source: `crates/server/src/workflow/step_router.rs`

## Verification and Fix Attempt Budget

The `VerifyHandler` runs a configurable pre-push hook against the worker's code. On hook failure:

1. Increments `fix_attempt_count` metadata on the ticket
2. If `fix_attempt_count` < `max_fix_attempts` (default: 10, configurable per project): transitions to `Implementing` for another attempt
3. If `fix_attempt_count` >= `max_fix_attempts`: sets the worker's `agent_status` to `stalled`, halting the cycle

On successful push (handled by `PushHandler`), `fix_attempt_count` is reset to 0. This means the budget resets each time code is successfully pushed, giving the worker a fresh set of attempts for any subsequent verification cycles.

Source: `crates/server/src/workflow/handlers/verify.rs`, `crates/server/src/workflow/handlers/push.rs`

## Epic Promotion and Feedback Flow

When a ticket reaches `FeedbackCreating`, the `FeedbackCreateHandler`:

1. **Promotes the ticket to an epic** (if not already) so child feedback tickets can be parented under it
2. Sends the `CreateFeedbackTickets(ticket_id, pr_number)` RPC to the worker
3. The worker creates child tickets from PR review comments and goes idle
4. The step router detects Idle + FeedbackCreating and routes via `AdvanceByFeedbackMode`:
   - `feedback_mode=now` (changes requested): transitions to `Implementing` to address feedback
   - `feedback_mode=later` (approved/merged): transitions to `Merging` to complete the PR

Source: `crates/server/src/workflow/handlers/feedback_create.rs`, `crates/server/src/workflow/step_router.rs`

## Merge Flow

The `MergeHandler` (FeedbackCreating -> Merging) performs:

1. Kills the worker and releases its slot
2. Merges the PR via `GhBackend::merge_pr` (squash strategy)
3. Closes the original ticket (lifecycle_status -> Done)
4. Finds the follow-up epic via `follow_up` edge and closes it
5. Dispatches follow-up epic children as independent work (lifecycle -> Open, branch cleared)

If the merge fails due to a conflict, the handler transitions back to `Implementing` with an activity recording the failure. The `DispatchImplementHandler` then re-dispatches the worker.

Source: `crates/server/src/workflow/handlers/merge.rs`

## WorkflowEngine Internals

The engine polls `workflow_event` every 500ms and processes one event per cycle:

1. **Poll**: `ticket_repo.poll_workflow_event()` returns the oldest unprocessed event
2. **Idempotency check**: Verify the ticket still has the expected lifecycle_status (skip stale events)
3. **Lifecycle-managed check**: Skip events for tickets without `lifecycle_managed = true`
4. **Handler lookup**: Find the registered handler for the `(from, to)` transition key
5. **Execute**: Call `handler.handle(ctx, ticket_id, transition)`
6. **On success**: Delete the event
7. **On failure**: Increment attempts; if attempts >= 3, revert ticket to `Open`

Source: `crates/server/src/workflow/engine.rs`

## GithubPollerManager

Scans every 30s for tickets in `pushing` or `in_review` lifecycle states:

### Pushing tickets
- Queries GitHub CI status via `GhBackend::check_runs()`
- **All green / No checks**: Transition to `InReview`
- **Failed**: Record failing checks as activity, transition to `Implementing`
- **Pending**: No action (re-check next scan)

### InReview tickets
- Check for `autoapprove` metadata -- if set, auto-advance to `FeedbackCreating` with `feedback_mode=later`
- Otherwise, check latest PR comment for emoji signals:
  - Approval (checkmark/rocket/ship/:shipit:): Transition to `FeedbackCreating` with `feedback_mode=later`
  - Changes requested (construction): Transition to `FeedbackCreating` with `feedback_mode=now`
- Check PR state: merged -> `FeedbackCreating`, closed -> revert to `Open`
- Only the latest comment counts, and only if no commits were pushed after it

Source: `crates/server/src/workflow/github_poller.rs`

## Error Handling

- **Handler failures**: Retried up to 3 times (MAX_ATTEMPTS). After exhausting retries, the ticket is stalled: it stays in its current lifecycle state and `stall_reason` metadata is set with the error message.
- **Stale events**: If a ticket's lifecycle_status has moved past the event's target, the event is deleted without processing.
- **No handler**: Events with no registered handler are deleted with a warning log.
- **Non-lifecycle tickets**: Events for tickets without `lifecycle_managed = true` are deleted.
