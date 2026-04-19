# UI Events Pipeline

## Overview

The UI events pipeline provides real-time change notifications from the server databases to connected clients (primarily the TUI). Each Postgres database (`ur_tickets` and `ur_workflow`) has its own `ui_events` table and its own `PgEventPoller` instance. The server merges both streams into a single fan-out before delivering batched events to gRPC subscribers.

The wake mechanism is Postgres triggers with `pg_notify` for instant delivery: triggers on data mutations insert rows into the `ui_events` table and call `pg_notify('ui_events', '')`. A `PgEventPoller` (LISTEN/NOTIFY) wakes the server-side poller, and a gRPC streaming RPC delivers batched events to subscribers. A configurable fallback timeout ensures events are still delivered if the LISTEN connection drops. If no listeners are connected, events are consumed and discarded.

## Data Flow

```
ticket_db (ur_tickets)                     workflow_db (ur_workflow)
Postgres Triggers                          Postgres Triggers
(ticket, worker tables)                    (workflow tables)
│                                          │
│  INSERT INTO ui_events                   │  INSERT INTO ui_events
│  + pg_notify('ui_events', '')            │  + pg_notify('ui_events', '')
│                                          │
▼                                          ▼
┌──────────────────────┐    ┌──────────────────────┐
│  ui_events (tickets)  │    │  ui_events (workflow) │
│  (BIGSERIAL PK)       │    │  (BIGSERIAL PK)       │
└──────────┬───────────┘    └──────────┬───────────┘
           │                           │
           ▼                           ▼
┌──────────────────────┐    ┌──────────────────────┐
│  PgEventPoller        │    │  PgEventPoller        │
│  (ticket_db)          │    │  (workflow_db)        │
└──────────┬───────────┘    └──────────┬───────────┘
           │                           │
           └───────────┬───────────────┘
                       │  merged mpsc stream
                       ▼
           ┌──────────────────────┐
           │  UiEventDispatcher   │   Server-side fan-out
           │  (merged stream)     │
           └──────────┬───────────┘
                      │  Dispatch to registered listeners
                      │  via mpsc channels
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

Triggers populate the `ui_events` buffer in each database and send a NOTIFY signal. Each trigger is implemented as a PL/pgSQL function executed after INSERT or UPDATE. Ticket triggers use a recursive CTE to propagate events to ancestor tickets (parent chain).

### ticket_db triggers

| Trigger | Table | Operation | entity_type | entity_id | Ancestor propagation |
|---------|-------|-----------|-------------|-----------|---------------------|
| `ui_events_ticket_insert` | `ticket` | INSERT | `ticket` | `NEW.id` + ancestors | Yes (recursive CTE) |
| `ui_events_ticket_update` | `ticket` | UPDATE | `ticket` | `NEW.id` + ancestors | Yes (recursive CTE) |
| `ui_events_worker_insert` | `worker` | INSERT | `worker` | `NEW.worker_id` | No |
| `ui_events_worker_update` | `worker` | UPDATE | `worker` | `NEW.worker_id` | No |

### workflow_db triggers

| Trigger | Table | Operation | entity_type | entity_id | Ancestor propagation |
|---------|-------|-----------|-------------|-----------|---------------------|
| `ui_events_workflow_insert` | `workflow` | INSERT | `workflow` | `NEW.ticket_id` | No |
| `ui_events_workflow_update` | `workflow` | UPDATE | `workflow` | `NEW.ticket_id` | No |

Each trigger function ends with `PERFORM pg_notify('ui_events', '')` to wake its respective poller immediately.

Source: `crates/ticket_db/migrations/` and `crates/workflow_db/migrations/`

## ui_events Table

The same schema exists in both `ur_tickets` and `ur_workflow`:

| Column | Type | Description |
|--------|------|-------------|
| `id` | BIGSERIAL PK | Monotonically increasing event ID (per-database sequence) |
| `entity_type` | TEXT NOT NULL | One of: `ticket`, `workflow`, `worker` |
| `entity_id` | TEXT NOT NULL | ID of the changed entity |
| `created_at` | TEXT NOT NULL | Timestamp (default: `now()::TEXT`) |

The table is an ephemeral buffer, not a permanent log. Rows are deleted immediately after consumption by the poller.

The DDL is shared via the `db_events::UI_EVENTS_DDL` constant, but each crate's migration file embeds it verbatim (sqlx migrations are file-based and cannot import from other crates at migration time). If the DDL changes, update `db_events::UI_EVENTS_DDL` and add new migration files to both `ticket_db` and `workflow_db`.

## PgEventPoller

The `PgEventPoller` (from `crates/db_events`) is a generic LISTEN/NOTIFY poller. The server instantiates two — one per database pool. Each poller:

1. Holds a dedicated `PgListener` connection (separate from the pool) subscribed to the `ui_events` Postgres channel
2. On NOTIFY: queries and deletes all buffered events from its `ui_events` table
3. Sends `Vec<UiEvent>` batches via an mpsc channel to the merged dispatcher

### Wake Mechanism

When a trigger fires `pg_notify('ui_events', '')`, the listener for that database wakes immediately. If the LISTEN connection drops, the poller falls back to periodic polling at the fallback interval and attempts to reconnect.

### Poll Cycle

1. **Poll**: Query all buffered events from `ui_events` ordered by ID
2. **Delete**: Remove consumed rows (by max ID)
3. **Send**: Emit batch via mpsc channel to dispatcher
4. **Wait**: Wait for NOTIFY signal, fallback timeout, or shutdown signal

### Wake Reasons

The poller tracks four wake reasons:

| Reason | Description |
|--------|-------------|
| `Notification` | LISTEN/NOTIFY fired — instant wake |
| `Timeout` | Fallback interval elapsed |
| `Shutdown` | Shutdown signal received |
| `ListenError` | LISTEN connection broke; poller reconnects |

Source: `crates/db_events/src/lib.rs`

## Merged Dispatcher

The server merges the two poller mpsc receivers into a single fan-out (`UiEventDispatcher` or equivalent). This component:

- Reads from both poller channels concurrently (tokio::select! or stream merge)
- Dispatches each batch to all registered gRPC listener channels
- Removes dead listener channels (closed by disconnected clients) on each dispatch cycle

### Listener Registration

Each gRPC stream subscriber (`SubscribeUiEvents`) receives its own mpsc channel. The dispatcher iterates over all registered channels and sends each batch. If a send fails (channel closed), the dispatcher removes that listener.

### No Listeners Behavior

If no listeners are registered when events are polled, the events are still consumed (deleted from both tables) and discarded. The `ui_events` tables are transient buffers, not durable queues. This prevents unbounded growth when no clients are connected.

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

The RPC is a server-streaming call on the `TicketService`. The client sends an empty request and receives a continuous stream of `UiEventBatch` messages, each containing one or more `UiEvent` entries. Events from both databases are interleaved in the same batch.

### Entity Type Mapping

The string `entity_type` from the database is mapped to the `UiEventType` proto enum:

| Database value | Proto enum | Source DB |
|----------------|------------|-----------|
| `ticket` | `TICKET` | ticket_db |
| `worker` | `WORKER` | ticket_db |
| `workflow` | `WORKFLOW` | workflow_db |
| (unknown) | `UNKNOWN` | either |

Unknown entity types are mapped to `UNKNOWN` rather than causing errors. This allows adding new trigger types without breaking existing clients.

## Configuration

The fallback poll interval is configured in `ur.toml` under the `[server]` section and applies to both pollers:

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

The TUI (`urui`) subscribes to the UI events stream on startup via the gRPC channel. Incoming events are forwarded as `AppEvent::DataReady` through the `EventManager` channel, triggering page-level data refreshes. The TUI does not need to poll on a timer for data changes — the event stream provides push-based notification.

The consumption pattern:

1. TUI calls `SubscribeUiEvents` on the shared gRPC channel
2. A background tokio task reads from the stream
3. On each `UiEventBatch`, the task sends an `AppEvent::DataReady` through the `EventManager` sender
4. The main app loop receives the event and triggers a data refresh for the relevant page

Source: `crates/urui/src/event.rs` (`EventManager`, `AppEvent`)

## Error Handling

| Scenario | Behavior |
|----------|----------|
| Dead listener channel | Dispatcher removes the listener from its registry on next dispatch cycle |
| Unknown `entity_type` in database | Mapped to `UiEventType::UNKNOWN`, included in batch (not dropped) |
| gRPC stream disconnects | Server-side listener channel closes; dispatcher cleans up on next dispatch |
| No listeners connected | Events are consumed and discarded from both databases; tables stay clean |
| Database read failure | Poller logs the error and retries on the next poll cycle |
| LISTEN connection drops | Poller falls back to periodic polling and attempts reconnection |
| One poller fails permanently | The other poller continues; events from the failed DB are missed until reconnection |

## Key Files

- `crates/db_events/src/lib.rs` — `PgEventPoller`, `UiEvent`, `UI_EVENTS_CHANNEL`, `UI_EVENTS_DDL`
- `crates/ticket_db/migrations/` — `ui_events` table schema and ticket/worker trigger functions with `pg_notify`
- `crates/workflow_db/migrations/` — `ui_events` table schema and workflow trigger functions with `pg_notify`
- `crates/server/src/ui_event_poller.rs` — merged dispatcher and gRPC fan-out
- `proto/ticket.proto` — `SubscribeUiEvents` RPC, `UiEvent`, `UiEventBatch` messages
- `crates/ur_config/src/lib.rs` — `ui_event_poll_interval_ms` configuration
- `crates/urui/src/event.rs` — Client-side `EventManager` and `AppEvent` enum
