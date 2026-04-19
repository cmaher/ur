# db_events

Shared Postgres event infrastructure consumed by both `ticket_db` and `workflow_db`. Contains no domain logic — purely infrastructure for LISTEN/NOTIFY-based UI event polling.

## Responsibilities

- `UI_EVENTS_CHANNEL` constant: the Postgres LISTEN/NOTIFY channel name (`"ui_events"`).
- `UI_EVENTS_DDL` constant: the `ui_events` table DDL as a `&'static str`, exported as the canonical definition. Both `ticket_db` and `workflow_db` embed this verbatim in their own migration files.
- `UiEvent`: the raw database row type for events read from the `ui_events` table.
- `PgEventPoller`: generic LISTEN/NOTIFY poller. The server instantiates one per database pool (one for `ticket_db`, one for `workflow_db`). Yields `Vec<UiEvent>` batches via an mpsc channel, which the server merges into a single fan-out for gRPC subscribers.

## Two-Poller Architecture

The server runs **two `PgEventPoller` instances**:

1. One connected to `ticket_db` (`ur_tickets`) — wakes on ticket and worker mutations
2. One connected to `workflow_db` (`ur_workflow`) — wakes on workflow mutations

The server merges both mpsc receivers into a unified event stream that is dispatched to all registered gRPC subscribers. Clients receive a single `SubscribeUiEvents` stream containing events from both databases.

## ui_events DDL Sharing Strategy

The `ui_events` table DDL must be identical in both `ticket_db/migrations/001_initial.sql` and `workflow_db/migrations/001_initial.sql`. The canonical definition is `db_events::UI_EVENTS_DDL`, but sqlx migrations are file-based and cannot import code from other crates at migration time. Therefore, both migration files embed the DDL **verbatim (copy-paste)**.

**This is intentional.** If the DDL changes, update all three locations:
1. `db_events::UI_EVENTS_DDL` (this crate)
2. `ticket_db/migrations/` (new migration file — never modify existing migrations)
3. `workflow_db/migrations/` (same)

## Conventions

- No domain logic — purely infrastructure.
- Channel name and DDL constants are defined here and imported by `ticket_db` and `workflow_db`.
- `PgEventPoller` is generic over any database that has the `ui_events` table.

## Testing

Tests connect to ci-postgres on `localhost:5433` (same as `ur_db_test`). Run:

```bash
cargo test -p db_events
```

Requires the test postgres to be running (`cargo make test:init`).
