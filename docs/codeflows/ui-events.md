# UI Events Pipeline

## Overview

The UI events pipeline provides real-time change notifications from the server database to connected clients (primarily the TUI). It is an ephemeral, poll-and-consume system: SQLite triggers capture data mutations, a server-side poller reads and deletes buffered rows, and a gRPC streaming RPC delivers batched events to subscribers. If no listeners are connected, events are consumed and discarded.

## Data Flow

```
SQLite Triggers (ticket, workflow, worker tables)
│
│  INSERT INTO ui_events (entity_type, entity_id)
│
▼
┌──────────────────────┐
│  ui_events table      │   Ephemeral buffer
│  (autoincrement id)   │
└──────────┬───────────┘
           │
           │  Poll every ui_event_poll_interval_ms (default: 200ms)
           │
           ▼
┌──────────────────────┐
│  UiEventPoller        │   Server-side tokio task
│  (consume + delete)   │
└──────────┬───────────┘
           │
           │  Dispatch to registered listeners
           │  via broadcast/mpsc channels
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

## SQLite Triggers

Six triggers on three tables populate the `ui_events` buffer. Each trigger fires on INSERT or UPDATE, writing the entity type and ID:

| Trigger | Table | Operation | entity_type | entity_id |
|---------|-------|-----------|-------------|-----------|
| `ui_events_ticket_insert` | `ticket` | INSERT | `ticket` | `NEW.id` |
| `ui_events_ticket_update` | `ticket` | UPDATE | `ticket` | `NEW.id` |
| `ui_events_workflow_insert` | `workflow` | INSERT | `workflow` | `NEW.ticket_id` |
| `ui_events_workflow_update` | `workflow` | UPDATE | `workflow` | `NEW.ticket_id` |
| `ui_events_worker_insert` | `worker` | INSERT | `worker` | `NEW.worker_id` |
| `ui_events_worker_update` | `worker` | UPDATE | `worker` | `NEW.worker_id` |

Source: `crates/ur_db/migrations/015_ui_events.sql`

## ui_events Table

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PK AUTOINCREMENT | Monotonically increasing event ID |
| `entity_type` | TEXT NOT NULL | One of: `ticket`, `workflow`, `worker` |
| `entity_id` | TEXT NOT NULL | ID of the changed entity |
| `created_at` | TEXT NOT NULL | Timestamp (default: `datetime('now')`) |

The table is an ephemeral buffer, not a permanent log. Rows are deleted immediately after consumption by the poller.

## UiEventPoller

The `UiEventPoller` is a server-side tokio task that runs a poll-consume-dispatch loop:

1. **Poll**: Query `SELECT * FROM ui_events ORDER BY id` to read all buffered events
2. **Delete**: Remove consumed rows from the table (by ID range or batch delete)
3. **Dispatch**: Send the batch to all registered listeners via channels
4. **Sleep**: Wait `ui_event_poll_interval_ms` before the next cycle

### Listener Registration

Listeners register with the poller and receive events through channels. Each gRPC stream subscriber gets its own channel. The poller iterates over all registered listener channels and sends the batch to each.

### Dead Channel Cleanup

When a listener disconnects (channel closed), the send fails. The poller detects closed channels and removes dead listeners from its registry. This prevents unbounded memory growth from abandoned subscriptions.

### No Listeners Behavior

If no listeners are registered when events are polled, the events are still consumed (deleted from the table) and discarded. The `ui_events` table is a transient buffer, not a durable queue. This prevents unbounded table growth when no clients are connected.

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

The poll interval is configurable in `ur.toml` under the `[server]` section:

```toml
[server]
ui_event_poll_interval_ms = 200  # default
```

| Setting | Default | Description |
|---------|---------|-------------|
| `ui_event_poll_interval_ms` | 200 | Milliseconds between poll cycles |

Lower values increase responsiveness but add database load. The default of 200ms provides near-real-time updates without excessive polling.

Source: `crates/ur_config/src/lib.rs` (`ServerConfig`, `DEFAULT_UI_EVENT_POLL_INTERVAL_MS`)

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

## Key Files

- `crates/ur_db/migrations/015_ui_events.sql` -- Table schema and SQLite triggers
- `proto/ticket.proto` -- `SubscribeUiEvents` RPC, `UiEvent`, `UiEventBatch` messages
- `crates/ur_config/src/lib.rs` -- `ui_event_poll_interval_ms` configuration
- `crates/urui/src/event.rs` -- Client-side `EventManager` and `AppEvent` enum
