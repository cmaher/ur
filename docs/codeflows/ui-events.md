# UI Events Pipeline

## Overview

The UI events pipeline provides real-time change notifications from the server database to connected clients (primarily the TUI). It uses Postgres triggers with `pg_notify` for instant wake-up: triggers on data mutations insert rows into the `ui_events` table and call `pg_notify('ui_events', '')`, a `PgListener` (LISTEN/NOTIFY) wakes the server-side poller, and a gRPC streaming RPC delivers batched events to subscribers. A configurable fallback timeout ensures events are still delivered if the LISTEN connection drops. If no listeners are connected, events are consumed and discarded.

## Data Flow

```
Postgres Triggers (ticket, workflow, worker tables)
│
│  INSERT INTO ui_events (entity_type, entity_id)
│  + PERFORM pg_notify('ui_events', '')
│
▼
┌──────────────────────┐
│  ui_events table      │   Ephemeral buffer (BIGSERIAL PK)
│  (autoincrement id)   │
└──────────┬───────────┘
           │
           │  PgListener wakes on NOTIFY
           │  (fallback: poll every fallback_interval, default 5s)
           │
           ▼
┌──────────────────────┐
│  UiEventPoller        │   Server-side tokio task
│  (consume + delete)   │
└──────────┬───────────┘
           │
           │  Dispatch to registered listeners
           │  via mpsc channels
           │
           ▼
┌──────────────────────┐
│  gRPC stream          │   SubscribeUiEvents RPC
│  (UiEventBatch)       │   (server-streaming)
└──────────┬───────────┘
           │
           ▼
┌──────────────────────┐
│  Client (urui TUI)    │   Triggers DataReady refresh
└──────────────────────┘
```

## Postgres Triggers

Triggers on three tables populate the `ui_events` buffer and send a NOTIFY signal. Each trigger is implemented as a PL/pgSQL function executed after INSERT or UPDATE. Ticket triggers use a recursive CTE to propagate events to ancestor tickets (parent chain).

| Trigger | Table | Operation | entity_type | entity_id | Ancestor propagation |
|---------|-------|-----------|-------------|-----------|---------------------|
| `ui_events_ticket_insert` | `ticket` | INSERT | `ticket` | `NEW.id` + ancestors | Yes (recursive CTE) |
| `ui_events_ticket_update` | `ticket` | UPDATE | `ticket` | `NEW.id` + ancestors | Yes (recursive CTE) |
| `ui_events_workflow_insert` | `workflow` | INSERT | `workflow` | `NEW.ticket_id` | No |
| `ui_events_workflow_update` | `workflow` | UPDATE | `workflow` | `NEW.ticket_id` | No |
| `ui_events_worker_insert` | `worker` | INSERT | `worker` | `NEW.worker_id` | No |
| `ui_events_worker_update` | `worker` | UPDATE | `worker` | `NEW.worker_id` | No |

Each trigger function ends with `PERFORM pg_notify('ui_events', '')` to wake the poller immediately.

Source: `crates/ur_db/migrations/001_initial.sql`

## ui_events Table

| Column | Type | Description |
|--------|------|-------------|
| `id` | BIGSERIAL PK | Monotonically increasing event ID |
| `entity_type` | TEXT NOT NULL | One of: `ticket`, `workflow`, `worker` |
| `entity_id` | TEXT NOT NULL | ID of the changed entity |
| `created_at` | TEXT NOT NULL | Timestamp (default: `now()::TEXT`) |

The table is an ephemeral buffer, not a permanent log. Rows are deleted immediately after consumption by the poller.

## UiEventPoller

The `UiEventPoller` is a server-side tokio task that uses Postgres LISTEN/NOTIFY for instant wake-up, with a fallback timeout for resilience.

### Wake Mechanism

The poller holds a dedicated `PgListener` connection (separate from the pool) that subscribes to the `ui_events` channel. When a trigger fires `pg_notify('ui_events', '')`, the listener wakes immediately. If the LISTEN connection drops, the poller falls back to periodic polling at the fallback interval and attempts to reconnect.

### Poll Cycle

1. **Poll**: Query all buffered events from `ui_events` ordered by ID
2. **Delete**: Remove consumed rows (by max ID)
3. **Dispatch**: Send the batch to all registered listeners via mpsc channels
4. **Wait**: Wait for NOTIFY signal, fallback timeout, or shutdown signal

### Listener Registration

Listeners register with the poller and receive events through mpsc channels. Each gRPC stream subscriber gets its own channel. The poller iterates over all registered listener channels and sends the batch to each.

### Dead Channel Cleanup

When a listener disconnects (channel closed), the send fails. The poller detects closed channels and removes dead listeners from its registry. This prevents unbounded memory growth from abandoned subscriptions.

### No Listeners Behavior

If no listeners are registered when events are polled, the events are still consumed (deleted from the table) and discarded. The `ui_events` table is a transient buffer, not a durable queue. This prevents unbounded table growth when no clients are connected.

### LISTEN Connection Recovery

The poller tracks four wake reasons: `Notification` (LISTEN/NOTIFY fired), `Timeout` (fallback interval elapsed), `Shutdown` (shutdown signal received), and `ListenError` (LISTEN connection broke). On `ListenError`, the poller drops the broken connection and attempts to reconnect. If reconnection fails, subsequent iterations use fallback polling until a reconnect succeeds.

## gRPC Interface

### Proto Definition

```protobuf
// proto/ticket.proto

rpc SubscribeUiEvents(SubscribeUiEventsRequest) returns (stream UiEventBatch);

enum UiEventType {
  UNKNOWN = 0;
  TICKET = 1;
  WORKFLOW = 2;
  WORKER = 3;
}

message UiEvent {
  UiEventType entity_type = 1;
  string entity_id = 2;
}

message UiEventBatch {
  repeated UiEvent events = 1;
}

message SubscribeUiEventsRequest {}
```

The RPC is a server-streaming call on the `TicketService`. The client sends an empty request and receives a continuous stream of `UiEventBatch` messages, each containing one or more `UiEvent` entries.

### Entity Type Mapping

The string `entity_type` from the database is mapped to the `UiEventType` proto enum:

| Database value | Proto enum | Description |
|----------------|------------|-------------|
| `ticket` | `TICKET` | Ticket created or updated |
| `workflow` | `WORKFLOW` | Workflow state changed |
| `worker` | `WORKER` | Worker status changed |
| (unknown) | `UNKNOWN` | Unrecognized entity type (logged, not fatal) |

Unknown entity types are mapped to `UNKNOWN` rather than causing errors. This allows adding new trigger types without breaking existing clients.

## Configuration

The fallback poll interval is configured in `ur.toml` under the `[server]` section:

```toml
[server]
ui_event_poll_interval_ms = 5000  # default: 5000 (5 seconds)
```

| Setting | Default | Description |
|---------|---------|-------------|
| `ui_event_poll_interval_ms` | 5000 | Fallback interval in milliseconds when LISTEN/NOTIFY is active; primary wake-up is instant via NOTIFY |

With LISTEN/NOTIFY working, events are delivered nearly instantly regardless of this interval. The interval only governs fallback polling when the LISTEN connection is unavailable.

Source: `crates/ur_config/src/lib.rs` (`ServerConfig`)

## Client Consumption (urui TUI)

The TUI (`urui`) subscribes to the UI events stream on startup via the gRPC channel. Incoming events are forwarded as `AppEvent::DataReady` through the `EventManager` channel, triggering page-level data refreshes. The TUI does not need to poll on a timer for data changes -- the event stream provides push-based notification.

The consumption pattern:

1. TUI calls `SubscribeUiEvents` on the shared gRPC channel
2. A background tokio task reads from the stream
3. On each `UiEventBatch`, the task sends an `AppEvent::DataReady` through the `EventManager` sender
4. The main app loop receives the event and triggers a data refresh for the relevant page

Source: `crates/urui/src/event.rs` (`EventManager`, `AppEvent`)

## Error Handling

| Scenario | Behavior |
|----------|----------|
| Dead listener channel | Poller removes the listener from its registry on next dispatch cycle |
| Unknown `entity_type` in database | Mapped to `UiEventType::UNKNOWN`, included in batch (not dropped) |
| gRPC stream disconnects | Server-side listener channel closes; poller cleans up on next dispatch |
| No listeners connected | Events are consumed and discarded; table stays clean |
| Database read failure | Poller logs the error and retries on the next poll cycle |
| LISTEN connection drops | Poller falls back to periodic polling and attempts reconnection |

## Key Files

- `crates/ur_db/migrations/001_initial.sql` -- Table schema and Postgres trigger functions with `pg_notify`
- `crates/server/src/ui_event_poller.rs` -- `UiEventPoller` with `PgListener` LISTEN/NOTIFY integration
- `proto/ticket.proto` -- `SubscribeUiEvents` RPC, `UiEvent`, `UiEventBatch` messages
- `crates/ur_config/src/lib.rs` -- `ui_event_poll_interval_ms` configuration
- `crates/urui/src/event.rs` -- Client-side `EventManager` and `AppEvent` enum
