# db_events

Shared Postgres event infrastructure consumed by both `ticket_db` and `workflow_db`.

## Responsibilities

- pg_notify poller for database-driven UI events.
- `ui_events` schema snippet and channel name constants.
- Provides a common `PgListener`-based event loop so both DB crates share the same notification infrastructure.

## Conventions

- No domain logic — purely infrastructure.
- Channel name constants are defined here and imported by `ticket_db` and `workflow_db`.
