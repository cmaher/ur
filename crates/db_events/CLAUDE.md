# db_events

Shared Postgres event infrastructure consumed by both `ticket_db` and `workflow_db`.

## Responsibilities

- `UI_EVENTS_CHANNEL` constant: the Postgres LISTEN/NOTIFY channel name (`"ui_events"`).
- `UI_EVENTS_DDL` constant: the `ui_events` table DDL as a `&'static str`, exported for documentation and reference.
- `UiEvent`: the raw database row type for events read from the `ui_events` table.
- `PgEventPoller`: generic LISTEN/NOTIFY poller. Each DB crate instantiates one against its own pool. Yields `Vec<UiEvent>` batches via `mpsc::Receiver`.

## ui_events DDL Sharing Strategy

The `ui_events` table DDL must be identical in both `ticket_db/migrations/001_initial.sql` and `workflow_db/migrations/001_initial.sql`. The canonical definition is `db_events::UI_EVENTS_DDL`, but sqlx migrations are file-based and cannot import code from other crates at migration time. Therefore, both migration files embed the DDL **verbatim (copy-paste)**.

**This is intentional.** If the DDL changes, update all three locations:
1. `db_events::UI_EVENTS_DDL` (this crate)
2. `ticket_db/migrations/` (new migration file — never modify existing migrations)
3. `workflow_db/migrations/` (same)

## Conventions

- No domain logic — purely infrastructure.
- Channel name constants are defined here and imported by `ticket_db` and `workflow_db`.
- `PgEventPoller` mirrors the `UiEventPoller` in `crates/server/src/ui_event_poller.rs` but is generic over any database with the `ui_events` table.

## Testing

Tests connect to ci-postgres on `localhost:5433` (same as `ur_db_test`). Run:

```bash
cargo test -p db_events
```

Requires the test postgres to be running (`cargo make test:init`).
