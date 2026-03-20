# Workflow Coordinator

## Overview

The workflow system drives tickets through an automated state machine: dispatch, implement, verify, push, review, and merge. It is built on three layers:

1. **WorkflowCoordinator** -- receives step-complete signals from workers via gRPC and advances tickets through the workflow
2. **WorkerdNextStepRouter** -- pure-function router mapping `(workflow_status, agent_status)` to the next action
3. **GithubPollerManager** -- polls GitHub for CI status and review signals, advancing external-wait states

The system uses two database tables (added by a prior ticket):
- **`workflow`** -- tracks the current workflow state for each ticket (status, worker assignment, attempt counts)
- **`workflow_intent`** -- records pending intents (what the coordinator plans to do next) for crash recovery

## State Machine

```
                    CLI dispatch
                         |
                         v
  Open ──────> AwaitingDispatch ──────> Implementing ──────> Verifying
                 (worker idle              ^    ^               |
                  signals)                 |    |               |
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

### Workflow States

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

## Architecture

### WorkflowCoordinator

The coordinator is the central orchestrator. Instead of polling a `workflow_event` table, it receives explicit `WorkflowStepComplete` gRPC calls from workers (via the `CoreService` RPC). On each signal:

1. Looks up the worker's assigned ticket via `worker_id` metadata
2. Loads the workflow record from the `workflow` table to get current status
3. Consults the `WorkerdNextStepRouter` for the `(status, agent_status)` pair
4. Records an intent in the `workflow_intent` table (crash recovery)
5. Executes the handler for the transition
6. On success: updates workflow status, deletes the intent
7. On failure: increments attempts; if attempts >= max, stalls the ticket

### WorkerdNextStepRouter

The step router is a pure-function mapper from `(workflow_status, agent_status)` to `StepAction`:

| Action | Behavior |
|--------|----------|
| `Advance { to }` | Transition ticket to the next workflow state |
| `AdvanceByFeedbackMode` | Route based on `feedback_mode` metadata (`now` -> Implementing, `later` -> Merging) |
| `Redispatch { reminder }` | Re-send the phase-appropriate RPC to the worker |
| `Ignore` | No action |

Key routing rules:
- **No ticket assigned**: always `Ignore`
- **Open / AwaitingDispatch**: always `Ignore` (handled by dedicated logic)
- **Stalled**: always `Ignore`
- **Working**: `Redispatch { reminder: true }` for all active states
- **Idle + Implementing**: `Advance { to: Verifying }`
- **Idle + Pushing**: `Redispatch { reminder: false }`
- **Idle + FeedbackCreating**: `AdvanceByFeedbackMode`
- **Idle + other**: `Ignore`

Source: `crates/server/src/workflow/step_router.rs`

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

## gRPC Interface

### WorkflowStepComplete RPC

Defined on `CoreService` in `proto/core.proto`:

```protobuf
rpc WorkflowStepComplete(WorkflowStepCompleteRequest) returns (WorkflowStepCompleteResponse);

message WorkflowStepCompleteRequest {
  string worker_id = 1;
}

message WorkflowStepCompleteResponse {}
```

Workers call this RPC when they finish a workflow step (e.g., implementation complete, feedback tickets created). The server-side handler (ur-a9b62) will look up the worker's ticket, consult the router, and advance the workflow.

### Proto Changes

The `lifecycle_status` and `lifecycle_managed` fields have been removed from the ticket proto messages (`Ticket`, `UpdateTicketRequest`, `ListTicketsRequest`). Workflow state is now tracked in the `workflow` table rather than as fields on the ticket itself. The `WorkerSummary` message in `core.proto` retains `lifecycle_status` for display purposes (populated from the DB ticket's lifecycle_status column).

## Server Boot

At startup, `ur-server` spawns both the workflow engine and GitHub poller as background tokio tasks sharing a `watch::Receiver<bool>` shutdown channel:

```
ur-server main()
├── WorkflowEngine::new(ticket_repo, worker_repo, worker_prefix, builderd_client, config, build_handlers())
│   └── engine.spawn(shutdown_rx)   // processes workflow transitions
│
└── GithubPollerManager::new(ticket_repo, builderd_client)
    └── poller.spawn(shutdown_rx)   // scans pushing/in_review tickets every 30s
```

Both tasks run until the shutdown channel signals `true`, then exit gracefully.

Source: `crates/server/src/main.rs`

## Database Tables

### workflow

| Column | Type | Description |
|--------|------|-------------|
| `ticket_id` | TEXT PK | References tickets.id |
| `status` | TEXT | Current workflow status (e.g., implementing, verifying) |
| `worker_id` | TEXT | Assigned worker ID |
| `attempt_count` | INTEGER | Handler retry count |
| `created_at` | TEXT | Workflow creation timestamp |
| `updated_at` | TEXT | Last status change timestamp |

### workflow_intent

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PK | Intent ID |
| `ticket_id` | TEXT | References tickets.id |
| `action` | TEXT | Planned action (e.g., advance, redispatch) |
| `target_status` | TEXT | Target workflow status |
| `created_at` | TEXT | Intent creation timestamp |

The intent table provides crash recovery: if the server crashes mid-transition, on restart it can replay pending intents. Intents are deleted after successful execution.

## Dispatch Flow

When the user runs `ur launch --dispatch <ticket-id>`:

1. `dispatch_ticket()` validates the ticket exists and is not closed
2. Sends an update via gRPC (the server-side workflow will handle the actual state transition)
3. The coordinator creates a workflow record and transitions to `AwaitingDispatch`
4. The ticket remains in `AwaitingDispatch` until a worker reports idle

Source: `crates/ur/src/main.rs` (`dispatch_ticket()`)

## Worker Readiness Flow (AwaitingDispatch to Implementing)

When a worker container starts and its agent becomes idle for the first time:

1. Worker calls `update_agent_status` RPC with `status: "idle"`
2. The gRPC handler validates the agent status via `AgentStatus` enum
3. Updates the worker's `agent_status` in the database
4. Looks up the worker's assigned ticket via `worker_id` metadata
5. If the ticket's workflow status is `AwaitingDispatch` and agent_status is `"idle"`:
   - Transitions workflow status to `Implementing`
   - The `DispatchImplementHandler` sends /clear then the implement RPC to the worker
6. If the ticket is in any other state, the `WorkerdNextStepRouter` is consulted

Source: `crates/server/src/grpc.rs` (`handle_agent_status_routed()`)

## AgentStatus Enum

Agent status is validated via the `AgentStatus` enum rather than raw strings:

| Variant | Wire value | Description |
|---------|-----------|-------------|
| `Starting` | `"starting"` | Worker process initializing (default) |
| `Idle` | `"idle"` | Agent is idle, ready for work |
| `Working` | `"working"` | Agent is actively executing |
| `Stalled` | `"stalled"` | Agent has stalled (no progress) |

Source: `crates/ur_db/src/model.rs`

## Verification and Fix Attempt Budget

The `VerifyHandler` runs a configurable pre-push hook against the worker's code. On hook failure:

1. Increments `fix_attempt_count` metadata on the ticket
2. If `fix_attempt_count` < `max_fix_attempts` (default: 10, configurable per project): transitions to `Implementing` for another attempt
3. If `fix_attempt_count` >= `max_fix_attempts`: sets the worker's `agent_status` to `stalled`, halting the cycle

On successful push (handled by `PushHandler`), `fix_attempt_count` is reset to 0.

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
3. Closes the original ticket (status -> Done)
4. Finds the follow-up epic via `follow_up` edge and closes it
5. Dispatches follow-up epic children as independent work (branch cleared)

If the merge fails due to a conflict, the handler transitions back to `Implementing` with an activity recording the failure.

Source: `crates/server/src/workflow/handlers/merge.rs`

## GithubPollerManager

Scans every 30s for tickets in `pushing` or `in_review` workflow states:

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

- **Handler failures**: Retried up to max_transition_attempts times. After exhausting retries, the ticket is stalled: it stays in its current workflow state, `stall_reason` metadata is set with the error message, and the intent is deleted. Use `ur admin redrive` to retry.
- **Stale intents**: On server restart, pending intents are replayed. If the ticket has moved past the intent's target, the intent is deleted without processing.
- **No handler**: Transitions with no registered handler are logged as warnings and skipped.
- **Crash recovery**: The intent table ensures no transitions are lost if the server crashes mid-execution.
