# Workflow Coordinator

## Overview

The workflow system drives tickets through an automated state machine: dispatch, implement, verify, push, review, and merge. It is built on three layers:

1. **WorkflowCoordinator** -- channel-driven coordinator that receives `TransitionRequest` messages, serializes per-ticket handler execution, and processes pending requests after handler completion
2. **WorkerdNextStepRouter** -- pure-function router mapping `workflow_status` to the next action
3. **GithubPollerManager** -- polls GitHub for CI status and review signals, advancing external-wait states

The system uses three database tables:
- **`workflow`** -- tracks the current workflow state for each ticket (status, timestamps)
- **`workflow_intent`** -- records pending intents (target status) for crash recovery
- **`workflow_comments`** -- tracks seen PR comments for deduplication across feedback cycles

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
| `Merging` | Server merges PR (squash), kills worker, closes ticket, dispatches follow-up children |
| `Done` | Terminal state |

## Architecture

### WorkflowCoordinator

The coordinator is a channel-driven task that serializes handler execution per ticket. It receives `TransitionRequest` messages via an mpsc channel from two sources:

1. **WorkerCoreServiceHandler** -- worker gRPC calls (`UpdateAgentStatus`, `WorkflowStepComplete`) trigger transitions
2. **GithubPollerManager** -- background poller detects CI/review signals and triggers transitions
3. **Handlers themselves** -- some handlers auto-advance (e.g., VerifyHandler → Pushing on hook pass)

On each request:

1. Writes an intent to `workflow_intent` (crash recovery)
2. If a handler is already running for this ticket, queues as pending (latest wins)
3. If no handler is running, spawns one
4. When a handler completes, sends a completion signal back to the coordinator
5. Coordinator removes the ticket from in-flight and processes any pending request
6. On success: updates workflow status, deletes the intent
7. On failure: increments attempts; if attempts >= max, stalls the ticket

The completion channel is critical: without it, pending requests for a ticket would never be dequeued after the first handler completes.

Source: `crates/server/src/workflow/coordinator.rs`

### WorkerdNextStepRouter

The step router is a pure function mapping `workflow_status` → `NextStepResult`:

| Current Status | Result |
|----------------|--------|
| `Implementing` | `Advance { to: Verifying }` |
| `FeedbackCreating` | `AdvanceByFeedbackMode` |
| All others | `Ignore` |

`AdvanceByFeedbackMode` routes based on `feedback_mode` ticket metadata:
- `now` (changes requested) → `Implementing`
- `later` (approved) → `Merging`

The router only handles workerd-driven transitions. Poller-driven transitions (Pushing → InReview, InReview → FeedbackCreating) and handler-driven transitions (Verifying → Pushing) bypass the router entirely.

Source: `crates/server/src/workflow/step_router.rs`

### Registered Handlers

Handlers are keyed by **target status**, not by transition. Each handler runs when a ticket enters that status, regardless of which status it came from.

| Target Status | Handler | Description |
|---------------|---------|-------------|
| `AwaitingDispatch` | `AwaitingDispatchHandler` | No-op; acknowledges dispatch |
| `Implementing` | `ImplementHandler` | Sends Implement RPC to workerd (with /clear) |
| `Verifying` | `VerifyHandler` | Runs pre-push verification hook via builderd |
| `Pushing` | `PushHandler` | Pushes branch, creates/updates PR |
| `InReview` | `ReviewStartHandler` | No-op signal handler |
| `FeedbackCreating` | `FeedbackCreateHandler` | Queries pending comments, sends feedback create RPC to worker |
| `Merging` | `MergeHandler` | Merges PR (squash), kills worker, closes ticket, dispatches follow-up children |

Source: `crates/server/src/workflow/handlers/mod.rs` (`build_handlers()`)

## Transition Triggers

Different lifecycle transitions are triggered by different sources:

| Transition | Trigger |
|------------|---------|
| Open → AwaitingDispatch | CLI dispatch (`ur worker launch --dispatch`) |
| AwaitingDispatch → Implementing | Worker idle signal (`UpdateAgentStatus` RPC with `idle`) |
| Implementing → Verifying | Worker step complete (`WorkflowStepComplete` RPC) |
| Verifying → Pushing | VerifyHandler (hook passes or not configured) |
| Verifying → Implementing | VerifyHandler (hook fails, under fix limit) |
| Pushing → InReview | GithubPollerManager (CI green or no checks) |
| Pushing → Implementing | GithubPollerManager (CI failure) |
| InReview → FeedbackCreating | GithubPollerManager (review signal or autoapprove) |
| FeedbackCreating → Implementing | Worker step complete + `feedback_mode=now` |
| FeedbackCreating → Merging | Worker step complete + `feedback_mode=later` |
| Merging → Implementing | MergeHandler (merge conflict) |

## gRPC Interface

### Worker-Facing RPCs (WorkerCoreServiceHandler)

Served on the worker gRPC server (`0.0.0.0:{worker_port}`), protected by auth interceptor.

**`UpdateAgentStatus`** -- Worker reports its agent status (idle, working, stalled).
- On `idle`: checks if the worker's ticket has workflow status `AwaitingDispatch`. If so, sends a `TransitionRequest` for `Implementing` to the coordinator.
- On `stalled` with message: records request-human activity on the ticket.

**`WorkflowStepComplete`** -- Worker signals the current step is done.
- Finds the worker's assigned ticket via `worker_id` metadata
- Looks up the workflow status
- Consults the `WorkerdNextStepRouter`
- Sends the appropriate `TransitionRequest` to the coordinator

Source: `crates/server/src/grpc.rs` (`WorkerCoreServiceHandler`)

### Workerd RPCs (in worker container)

Served by the `workerd` daemon inside each worker container on port 9120.

**`Implement(ticket_id)`** -- Server dispatches implementation work.
- Populates `DispatchBuffer` with `["/clear", "/implement {ticket_id}"]`
- Sets `lifecycle_step = "implementing"`
- Pops and sends `/clear` to tmux immediately

**`NotifyIdle()`** -- Called by Claude Code hooks when agent goes idle.
- 4-state machine:
  1. Buffer has commands → pop and send to tmux
  2. Buffer empty + step_complete → send `WorkflowStepComplete` RPC to server
  3. Buffer empty + !step_complete + lifecycle_step set → nudge agent
  4. No active dispatch → forward idle to server (`UpdateAgentStatus`)

**`StepComplete()`** -- Called by `workertools step-complete` when agent finishes work.
- Sets `step_complete = true` on the `DispatchBuffer`
- Next idle signal will trigger case 2 above

Source: `crates/workerd/src/grpc_service.rs`

## Server Boot

At startup, `ur-server` spawns the coordinator, engine, and poller as background tokio tasks sharing a `watch::Receiver<bool>` shutdown channel:

```
ur-server main()
├── WorkflowCoordinator::new(transition_rx, cancel_rx, ctx, handlers, max_attempts)
│   └── coordinator.spawn(shutdown_rx)   // channel-driven coordinator
│
├── WorkflowEngine::new(...)
│   └── engine.spawn(shutdown_rx)        // polls workflow_event table (legacy)
│
├── GithubPollerManager::new(...)
│   └── poller.spawn(shutdown_rx)        // scans pushing/in_review every 30s
│
├── serve_grpc(host_addr, ...)           // host CLI gRPC server
│
└── serve_worker_grpc(worker_addr, ..., transition_tx)
    └── WorkerCoreServiceHandler         // worker gRPC server with auth
```

The `transition_tx` channel connects the worker gRPC handler to the coordinator. Both servers share the same `TicketRepo` and `WorkerRepo`.

Source: `crates/server/src/main.rs`

## Database Tables

### workflow

| Column | Type | Description |
|--------|------|-------------|
| `ticket_id` | TEXT PK | References tickets.id |
| `status` | TEXT | Current workflow status (e.g., implementing, verifying) |
| `created_at` | TEXT | Workflow creation timestamp |
| `updated_at` | TEXT | Last status change timestamp |

### workflow_intent

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PK | Intent ID |
| `ticket_id` | TEXT | References tickets.id |
| `target_status` | TEXT | Target workflow status |
| `attempts` | INTEGER | Handler retry count |
| `created_at` | TEXT | Intent creation timestamp |

The intent table provides crash recovery: if the server crashes mid-transition, on restart the coordinator replays pending intents. Intents are deleted after successful execution.

### workflow_comments

| Column | Type | Description |
|--------|------|-------------|
| `ticket_id` | TEXT | References tickets.id (composite PK) |
| `comment_id` | TEXT | GitHub PR comment ID (composite PK) |
| `feedback_created` | INTEGER | 0 = seen but pending, 1 = feedback ticket created |
| `created_at` | TEXT | When the comment was first seen |

The `workflow_comments` table enables two-phase comment handling for deduplication across feedback cycles. Phase 1 (poller): the `GithubPollerManager` inserts unseen comment IDs with `feedback_created = 0` when it detects new review comments. Phase 2 (step complete): after the worker finishes creating feedback tickets, `mark_feedback_created` flips `feedback_created` to 1 for all pending comments. This ensures that if the worker dies mid-way, comments remain pending (`feedback_created = 0`) and will be re-processed on the next feedback cycle. Already-handled comment IDs (`feedback_created = 1`) are passed to the worker so it skips comments that already have feedback tickets.

Source: `crates/ur_db/migrations/011_workflow_comments.sql`

## Ticket Types and Tree Structure

Tickets have two types: **task** and **design**. Tasks are the default work unit; design tickets represent pre-implementation planning.

Tickets form a tree hierarchy via parent-child relationships. Any ticket can have children -- there is no separate "epic" type. The `--tree` flag on list queries performs a recursive tree walk from a root ticket, returning the root and all descendants with a `depth` field indicating hierarchy level. The `dispatchable` command walks the full tree to find open tickets with no open blockers.

The tree structure is used by the workflow system: the `MergeHandler` finds follow-up children via `follow_up` edges and dispatches them as independent work after a successful merge.

## Dispatch Flow

When the user runs `ur worker launch --dispatch <ticket-id>`:

1. CLI creates a workflow record in `awaiting_dispatch` status
2. CLI launches the worker container
3. The ticket remains in `AwaitingDispatch` until the worker reports idle
4. The coordinator runs the `AwaitingDispatchHandler` (no-op acknowledgment)

Source: `crates/ur/src/main.rs`, `crates/server/src/grpc.rs`

## Worker Readiness Flow (AwaitingDispatch to Implementing)

When a worker container starts and its agent becomes idle for the first time:

1. Workerd's `NotifyIdle` handler (case 4: no active dispatch) fires `UpdateAgentStatus(idle)` to the server
2. `WorkerCoreServiceHandler::update_agent_status` updates the worker's status in DB
3. Spawns `handle_awaiting_dispatch_readiness`:
   - Finds the worker's assigned ticket via `worker_id` metadata
   - Checks if the ticket's workflow status is `AwaitingDispatch`
   - If so, sends a `TransitionRequest` for `Implementing` to the coordinator
4. The coordinator queues `Implementing` as pending (if `AwaitingDispatch` handler is still in-flight) or spawns immediately
5. `ImplementHandler` sends the Implement RPC to the workerd daemon
6. Workerd populates the DispatchBuffer with `/clear` + `/implement {ticket_id}`

Source: `crates/server/src/grpc.rs` (`handle_awaiting_dispatch_readiness`)

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

## Two-Phase Comment Handling and Feedback Flow

The feedback flow uses two-phase comment tracking to deduplicate PR comments across multiple feedback cycles (e.g., when a ticket goes through Implementing -> Pushing -> InReview -> FeedbackCreating multiple times).

### Phase 1: Comment Discovery (GithubPollerManager)

When scanning `InReview` tickets, the poller:

1. Fetches all seen comment IDs from `workflow_comments` for the ticket
2. Retrieves PR comments from GitHub
3. Filters out already-seen comments
4. Evaluates only unseen comments for review signals (approval/changes-requested)
5. Inserts new comment IDs into `workflow_comments` with `feedback_created = 0`

This ensures that comments from previous review rounds do not re-trigger transitions.

### Phase 2: Feedback Creation (FeedbackCreateHandler)

When a ticket reaches `FeedbackCreating`, the `FeedbackCreateHandler`:

1. Queries `workflow_comments` for pending comments (`feedback_created = 0`) and handled comments (`feedback_created = 1`)
2. Sends the `CreateFeedbackTickets(ticket_id, pr_number, handled_comment_ids)` RPC to the worker, passing handled IDs so the worker skips comments that already have feedback tickets
3. The worker creates child tickets from new PR review comments and signals step complete
4. On step completion, `WorkerCoreServiceHandler` calls `mark_feedback_created` to flip all pending comments to `feedback_created = 1`
5. The step router routes via `AdvanceByFeedbackMode`:
   - `feedback_mode=now` (changes requested): transitions to `Implementing` to address feedback
   - `feedback_mode=later` (approved/merged): transitions to `Merging` to complete the PR

If the worker dies before step completion, pending comments remain at `feedback_created = 0` and will be re-processed on recovery.

Source: `crates/server/src/workflow/handlers/feedback_create.rs`, `crates/server/src/workflow/step_router.rs`, `crates/server/src/grpc.rs`

## Merge Flow

The `MergeHandler` (→ Merging) performs:

1. Kills the worker and releases its slot
2. Merges the PR via `GhBackend::merge_pr` (squash strategy)
3. Closes the original ticket (status → Done)
4. Finds the follow-up ticket via `follow_up` edge and closes it
5. Dispatches follow-up ticket's children as independent work (branch cleared)

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
- Fetch seen comment IDs from `workflow_comments` to filter out already-processed comments
- Evaluate only unseen comments for emoji signals:
  - Approval (checkmark/rocket/ship/:shipit:): Transition to `FeedbackCreating` with `feedback_mode=later`
  - Changes requested (construction): Transition to `FeedbackCreating` with `feedback_mode=now`
- Insert new comment IDs into `workflow_comments` with `feedback_created = 0` (phase 1 of two-phase comment handling)
- Check PR state: merged → `FeedbackCreating`, closed → revert to `Open`
- Only the latest unseen comment counts, and only if no commits were pushed after it

Source: `crates/server/src/workflow/github_poller.rs`

## Error Handling

- **Handler failures**: Retried up to `max_transition_attempts` times. After exhausting retries, the ticket is stalled: it stays in its current workflow state, `stall_reason` metadata is set with the error message, and the intent is deleted. Use `ur flow redrive` to retry.
- **Stale intents**: On server restart, pending intents are replayed. If attempts are already at max, the intent is deleted and the ticket is stalled.
- **No handler**: Transitions with no registered handler are logged as warnings and the intent is cleaned up.
- **Crash recovery**: The intent table ensures no transitions are lost if the server crashes mid-execution.
