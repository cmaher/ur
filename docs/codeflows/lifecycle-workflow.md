# Workflow Coordinator

## Overview

The workflow system drives tickets through an automated state machine: dispatch, implement, verify, push, review, and merge. It is built on three layers:

1. **WorkflowCoordinator** -- channel-driven coordinator that receives `TransitionRequest` messages, serializes per-ticket handler execution, and processes pending requests after handler completion
2. **WorkerdNextStepRouter** -- pure-function router mapping `workflow_status` to the next action
3. **GithubPollerManager** -- polls GitHub for CI status, mergeability, and review signals on `InReview` tickets

The system uses four database tables:
- **`workflow`** -- tracks the current workflow state for each ticket (status, condition columns, timestamps)
- **`workflow_intent`** -- records pending intents (target status) for crash recovery
- **`workflow_comments`** -- tracks seen PR comments for deduplication across feedback cycles
- **`workflow_events`** -- append-only analytics log recording lifecycle transitions and condition changes

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
                                           |                 Pushing ──────> InReview
                                           |                  (push+PR,      (condition-gated)
                                           |                  init conds)     /    |     \
                                           |                                /     |      \
                                           |       (failures: CI fail   ──┘      |       \(changes
                                           |        or merge conflict)           |        requested)
                                           |                                     |         v
                                           |               (approve-only,        |   AddressingFeedback
                                           |                all 3 green)         |    /           \
                                           |                    |                | (now)/         \(later)
                                           |                    v                |    /             |
                                           |                 Merging <───────────┘                  v
                                           |                /      \                            Merging
                                           |   (conflict   /        \(success)                /      \
                                           |   or reject) /          v                       /        \
                                           └─────────────┘          Done          (conflict)/     (ok) \
                                                                                ───────────┘           v
                                                                                → Implementing        Done
```

### Key change from previous design

The `Pushing` state no longer polls CI or loops back to `Implementing` on CI failure. Instead, `PushHandler` directly transitions to `InReview` after a successful push and PR creation. All three conditions (CI, mergeability, review) are polled in parallel during `InReview` by the `GithubPollerManager`.

### Workflow States

| Status | Description |
|--------|-------------|
| `Design` | Pre-implementation planning (manual) |
| `Open` | Ready for work, not yet dispatched |
| `AwaitingDispatch` | CLI has dispatched; waiting for worker to become idle |
| `Implementing` | Worker is actively implementing the ticket |
| `Verifying` | Server runs pre-push verification hook |
| `Pushing` | Server pushes branch, creates/updates PR, initializes conditions, then advances directly to InReview |
| `InReview` | PR is open; poller evaluates three conditions (CI, mergeability, review) each scan cycle |
| `AddressingFeedback` | Worker creates feedback summary from review |
| `Merging` | Server verifies all three conditions, merges PR (squash), kills worker, closes ticket |
| `Done` | Terminal state |

## Architecture

### WorkflowCoordinator

The coordinator is a channel-driven task that serializes handler execution per ticket. It receives `TransitionRequest` messages via an mpsc channel from two sources:

1. **WorkerCoreServiceHandler** -- worker gRPC calls (`UpdateAgentStatus`, `WorkflowStepComplete`) trigger transitions
2. **GithubPollerManager** -- background poller detects CI/review signals and triggers transitions
3. **Handlers themselves** -- some handlers auto-advance (e.g., VerifyHandler → Pushing on hook pass, PushHandler → InReview on success)

On each request:

1. Writes an intent to `workflow_intent` (crash recovery)
2. If a handler is already running for this ticket, queues as pending (latest wins)
3. If no handler is running, spawns one
4. When a handler completes, sends a completion signal back to the coordinator
5. Coordinator removes the ticket from in-flight and processes any pending request
6. On success: updates workflow status, emits a lifecycle workflow event, deletes the intent
7. On failure: increments attempts; if attempts >= max, stalls the ticket

The coordinator emits a lifecycle workflow event (via `insert_workflow_event`) on every successful status transition, recording the new status as the event type. This provides an append-only audit trail in the `workflow_events` table.

The completion channel is critical: without it, pending requests for a ticket would never be dequeued after the first handler completes.

Source: `crates/server/src/workflow/coordinator.rs`

### WorkerdNextStepRouter

The step router is a pure function mapping `workflow_status` → `NextStepResult`:

| Current Status | Result |
|----------------|--------|
| `Implementing` | `Advance { to: Verifying }` |
| `AddressingFeedback` | `AdvanceByFeedbackMode` |
| All others | `Ignore` |

`AdvanceByFeedbackMode` routes based on `feedback_mode` ticket metadata:
- `now` (changes requested) → `Implementing`
- `later` (approved) → `Merging`

The router only handles workerd-driven transitions. Poller-driven transitions (InReview → AddressingFeedback/Merging/Implementing) and handler-driven transitions (Verifying → Pushing, Pushing → InReview) bypass the router entirely.

Source: `crates/server/src/workflow/step_router.rs`

### Registered Handlers

Handlers are keyed by **target status**, not by transition. Each handler runs when a ticket enters that status, regardless of which status it came from.

| Target Status | Handler | Description |
|---------------|---------|-------------|
| `AwaitingDispatch` | `AwaitingDispatchHandler` | No-op; acknowledges dispatch |
| `Implementing` | `ImplementHandler` | Sends Implement RPC to workerd (with /clear) |
| `Verifying` | `VerifyHandler` | Runs pre-push verification hook via builderd |
| `Pushing` | `PushHandler` | Pushes branch, creates/updates PR, initializes conditions, advances to InReview |
| `InReview` | `ReviewStartHandler` | No-op signal handler |
| `AddressingFeedback` | `FeedbackAddressHandler` | Queries pending comments, sends feedback create RPC to worker |
| `Merging` | `MergeHandler` | Pre-merge gate checks all 3 conditions, merges PR (squash), kills worker, closes ticket |

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
| Pushing → InReview | PushHandler (push success + PR created/updated, conditions initialized) |
| InReview → Implementing | GithubPollerManager (CI failure or merge conflict, no review feedback) |
| InReview → AddressingFeedback | GithubPollerManager (changes requested, or approval with other unseen comments, or autoapprove) |
| InReview → Merging | GithubPollerManager (approve-only: all 3 conditions green, approve is the only unseen comment) |
| AddressingFeedback → Implementing | Worker step complete + `feedback_mode=now` |
| AddressingFeedback → Merging | Worker step complete + `feedback_mode=later` |
| Merging → Implementing | MergeHandler (merge conflict or merge rejection, child ticket created via TicketClient) |

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

**`StepComplete()`** -- Called by `workertools status step-complete` when agent finishes work.
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
│   └── poller.spawn(shutdown_rx)        // scans in_review every 30s
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
| `ci_status` | TEXT | CI condition: `pending`, `succeeded`, `failed` (default: `pending`) |
| `mergeable` | TEXT | Merge condition: `unknown`, `mergeable`, `conflict` (default: `unknown`) |
| `review_status` | TEXT | Review condition: `pending`, `approved`, `changes_requested` (default: `pending`) |
| `created_at` | TEXT | Workflow creation timestamp |
| `updated_at` | TEXT | Last status change timestamp |

The three condition columns (`ci_status`, `mergeable`, `review_status`) are initialized when a ticket enters `InReview` via `PushHandler`. They are updated independently by the `GithubPollerManager` each scan cycle. The `MergeHandler` checks all three before attempting a merge (pre-merge gate). Condition values are defined as constants in `ur_rpc::workflow_condition`.

Source: `crates/ur_db/migrations/014_workflow_conditions.sql`

### workflow_events

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PK | Event ID |
| `workflow_id` | TEXT | References workflow.id (FK) |
| `event` | TEXT | Event type constant (e.g., `implementing`, `ci_succeeded`) |
| `created_at` | TEXT | Event timestamp |

Indexed on `(workflow_id, created_at)` for efficient per-workflow queries.

The events table is an append-only analytics log. Two categories of events are recorded:

1. **Lifecycle events** -- emitted by the coordinator on every status transition. Event names mirror lifecycle statuses (e.g., `implementing`, `in_review`, `merging`, `done`). Defined in `ur_rpc::workflow_event`.
2. **Condition events** -- emitted by the poller when external state changes: `pr_created`, `ci_succeeded`, `ci_failed`, `review_approved`, `review_changes_requested`, `merge_conflict_detected`, `stalled`. CI events use the actual `completed_at` timestamp from GitHub check runs (via `insert_workflow_event_at`) for accurate timing.

Source: `crates/ur_db/migrations/014_workflow_conditions.sql`

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

## Workflow Condition Constants

Condition values are defined as `&'static str` constants in `ur_rpc::workflow_condition`, organized by submodule:

| Module | Constants | Used for |
|--------|-----------|----------|
| `ci_status` | `PENDING`, `SUCCEEDED`, `FAILED` | `workflow.ci_status` column |
| `mergeable` | `UNKNOWN`, `MERGEABLE`, `CONFLICT` | `workflow.mergeable` column |
| `review_status` | `PENDING`, `APPROVED`, `CHANGES_REQUESTED` | `workflow.review_status` column |

Workflow event type constants are defined in `ur_rpc::workflow_event`:

| Constant | Category | Description |
|----------|----------|-------------|
| `AWAITING_DISPATCH` | lifecycle | Entered awaiting_dispatch |
| `IMPLEMENTING` | lifecycle | Entered implementing |
| `VERIFYING` | lifecycle | Entered verifying |
| `PUSHING` | lifecycle | Entered pushing |
| `IN_REVIEW` | lifecycle | Entered in_review |
| `ADDRESSING_FEEDBACK` | lifecycle | Entered addressing_feedback |
| `MERGING` | lifecycle | Entered merging |
| `DONE` | lifecycle | Entered done |
| `CANCELLED` | lifecycle | Workflow cancelled |
| `PR_CREATED` | condition | PR created after push |
| `CI_SUCCEEDED` | condition | CI checks passed |
| `CI_FAILED` | condition | CI checks failed |
| `REVIEW_APPROVED` | condition | Review approved (`ur approve`) |
| `REVIEW_CHANGES_REQUESTED` | condition | Changes requested (`ur respond`) |
| `MERGE_CONFLICT_DETECTED` | condition | PR has merge conflicts |
| `STALLED` | condition | Workflow stalled |

Source: `crates/ur_rpc/src/workflow_condition.rs`, `crates/ur_rpc/src/workflow_event.rs`

## Server-Side Ticket Creation (TicketClient)

The `TicketClient` enables workflow handlers and the poller to create child tickets for operational failures without going through the network gRPC stack. It wraps `TicketServiceHandler` and calls it in-process via the `TicketService` trait.

### Issue Types

Three issue types are defined as constants in `ticket_client::issue_type`:

| Constant | Value | Created by |
|----------|-------|------------|
| `CI_FAILURE` | `"ci_failure"` | GithubPollerManager (InReview scan, CI failed) |
| `MERGE_CONFLICT` | `"merge_conflict"` | GithubPollerManager (InReview scan, conflict detected) and MergeHandler (merge attempt fails) |
| `MERGE_REJECTION` | `"merge_rejection"` | MergeHandler (merge rejected by branch protection or other rules) |

### Deduplication

Each issue ticket has a `workflow_issue:{issue_type}` metadata key set on creation (e.g., `workflow_issue:ci_failure`). Before creating a new ticket, `TicketClient` searches for an existing open child of the parent with the same metadata key. If found, the existing ticket ID is returned instead of creating a duplicate.

### Flow

1. Handler or poller calls `ticket_client.create_workflow_issue_ticket(parent_id, issue_type, title, body)`
2. `TicketClient` checks for existing open child with matching `workflow_issue:{issue_type}` metadata
3. If found, returns existing ticket ID (dedup)
4. If not found, creates a child ticket via `TicketService::create_ticket`, sets the metadata key, and returns the new ticket ID

Source: `crates/server/src/workflow/ticket_client.rs`

## Ticket Types and Tree Structure

Tickets have two types: **task** and **design**. Tasks are the default work unit; design tickets represent pre-implementation planning.

Tickets form a tree hierarchy via parent-child relationships. Any ticket can have children -- there is no separate "epic" type. The `--tree` flag on list queries performs a recursive tree walk from a root ticket, returning the root and all descendants with a `depth` field indicating hierarchy level. The `dispatchable` command walks the full tree to find open tickets with no open blockers.

The tree structure is used by the workflow system for dependency tracking (blockers, dispatchable queries).

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

## Push and InReview Flow

The `PushHandler` performs a git push and PR creation, then transitions directly to `InReview`:

1. Pushes the branch via `local_repo.push()` through builderd
2. Creates a PR if none exists (stores `pr_number`, `pr_url`, `gh_repo` metadata)
3. Initializes the three workflow conditions via `initialize_workflow_conditions`:
   - `ci_status` = `pending`
   - `mergeable` = `unknown`
   - `review_status` = `pending` (or `approved` if ticket has `autoapprove` metadata)
4. Emits a `pr_created` workflow event
5. Sends a `TransitionRequest` for `InReview` to the coordinator

This replaces the previous flow where the poller scanned `Pushing` tickets for CI status. Now all external polling happens in `InReview`.

Source: `crates/server/src/workflow/handlers/push.rs`

## Two-Phase Comment Handling and Feedback Flow

The feedback flow uses two-phase comment tracking to deduplicate PR comments across multiple feedback cycles (e.g., when a ticket goes through Implementing -> Pushing -> InReview -> AddressingFeedback multiple times).

### Phase 1: Comment Discovery (GithubPollerManager)

When scanning `InReview` tickets, the poller:

1. Fetches all seen comment IDs from `workflow_comments` for the ticket
2. Retrieves PR comments from GitHub
3. Filters out already-seen comments
4. Evaluates only unseen comments for review signals (approval/changes-requested)
5. Inserts new comment IDs into `workflow_comments` with `feedback_created = 0`

This ensures that comments from previous review rounds do not re-trigger transitions.

### Phase 2: Feedback Creation (FeedbackAddressHandler)

When a ticket reaches `AddressingFeedback`, the `FeedbackAddressHandler`:

1. Queries `workflow_comments` for pending comments (`feedback_created = 0`) and handled comments (`feedback_created = 1`)
2. Sends the `AddressFeedbackTickets(ticket_id, pr_number, handled_comment_ids)` RPC to the worker, passing handled IDs so the worker skips comments that already have feedback tickets
3. The worker creates child tickets from new PR review comments and signals step complete
4. On step completion, `WorkerCoreServiceHandler` calls `mark_feedback_created` to flip all pending comments to `feedback_created = 1`
5. The step router routes via `AdvanceByFeedbackMode`:
   - `feedback_mode=now` (changes requested): transitions to `Implementing` to address feedback
   - `feedback_mode=later` (approved/merged): transitions to `Merging` to complete the PR

If the worker dies before step completion, pending comments remain at `feedback_created = 0` and will be re-processed on recovery.

Source: `crates/server/src/workflow/handlers/feedback_address.rs`, `crates/server/src/workflow/step_router.rs`, `crates/server/src/grpc.rs`

## Merge Flow

The `MergeHandler` (→ Merging) performs:

1. **Pre-merge gate**: Verifies all three conditions are met before proceeding:
   - `ci_status` = `succeeded`
   - `mergeable` = `mergeable`
   - `review_status` = `approved`
   If any condition fails, the handler returns an error (retried by the coordinator).
2. Kills the worker and releases its slot
3. Merges the PR via `GhBackend::merge_pr` (squash strategy)
4. Marks workflow as done and closes the ticket (status → Done)

If the merge fails:
- **Merge conflict**: Creates a child ticket via `TicketClient` (issue type `merge_conflict`), records activity, transitions to `Implementing`
- **Merge rejection** (branch protection, etc.): Creates a child ticket via `TicketClient` (issue type `merge_rejection`), records activity, transitions to `Implementing`

Source: `crates/server/src/workflow/handlers/merge.rs`

## GithubPollerManager

Scans every 30s for tickets in the `in_review` workflow state. Each scan cycle checks all three conditions in sequence, creates failure tickets as needed, then evaluates the combined state for transitions.

### InReview scan (per ticket)

**Step 1: Poll CI status** (`poll_ci_condition`)
- Queries GitHub CI status via `GhBackend::check_runs()`
- Updates `workflow.ci_status` if changed
- Emits `ci_succeeded` or `ci_failed` workflow event on change (using the check run's `completed_at` timestamp)

**Step 2: Poll mergeability** (`poll_mergeable_condition`)
- Queries PR mergeability via `GhBackend::check_mergeable()`
- Updates `workflow.mergeable` if changed
- Emits `merge_conflict_detected` workflow event when conflict is detected

**Step 3: Poll review signals** (`poll_review_condition`)
- Fetches seen comment IDs from `workflow_comments`
- Evaluates unseen comments for review commands (`ur approve`, `ur respond`)
- Updates `workflow.review_status` if changed
- Emits `review_approved` or `review_changes_requested` workflow event on change

**Step 4: Create failure tickets**
- If `ci_status` = `failed`: creates a child ticket via `TicketClient` (issue type `ci_failure`) with failing check details
- If `mergeable` = `conflict`: creates a child ticket via `TicketClient` (issue type `merge_conflict`)

**Step 5: Evaluate transition** (`evaluate_transition`)
- PR merged by human → `AddressingFeedback` (mode=later)
- PR closed without merge → cancel workflow, revert ticket to Open
- Changes requested → `AddressingFeedback` (mode=now), resets implement cycles
- Failures (CI or merge conflict) without review feedback → `Implementing`
- All three conditions green + approved:
  - Approve-only (1 or fewer unseen comments) → `Merging` (skip feedback)
  - Other unseen comments → `AddressingFeedback` (mode=later)
- Otherwise → stay in InReview (conditions not yet met)

Source: `crates/server/src/workflow/github_poller.rs`

## Error Handling

- **Handler failures**: Retried up to `max_transition_attempts` times. After exhausting retries, the ticket is stalled: it stays in its current workflow state, `stall_reason` metadata is set with the error message, and the intent is deleted. Use `ur flow redrive` to retry.
- **Stale intents**: On server restart, pending intents are replayed. If attempts are already at max, the intent is deleted and the ticket is stalled.
- **No handler**: Transitions with no registered handler are logged as warnings and the intent is cleaned up.
- **Crash recovery**: The intent table ensures no transitions are lost if the server crashes mid-execution.
