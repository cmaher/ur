# ticket_db

Postgres-backed ticket database crate. Owns the ticket lifecycle schema: tickets, activities, metadata, edges, dependency graph, workers, slots, and the ticket-side `ui_events` trigger infrastructure.

## Responsibilities

- Migrations for the ticket domain (`ticket`, `activity`, `meta`, `edge`, `slot`, `worker`, `worker_slot`, `ticket_comments`, `ui_events` tables and their triggers).
- `DatabaseManager` — opens a `PgPool` for `ur_tickets`, runs migrations on startup.
- `TicketRepo` — CRUD operations for tickets, activities, and metadata.
- `GraphManager` — dependency graph operations using petgraph, loaded from Postgres.
- `WorkerRepo` — worker and slot management.

## What Does NOT Live Here

- Workflow state (`workflow`, `workflow_intent`, `workflow_comments`, `workflow_events`) — those live in `workflow_db`.
- Shared event infrastructure (`PgEventPoller`, `UI_EVENTS_CHANNEL`, `UI_EVENTS_DDL`) — those live in `db_events`.
- Cross-DB foreign keys — the `workflow` table references `ticket_id` as a plain TEXT soft reference; no DB-level FK exists.

## Cross-DB References

Workflow tables in `workflow_db` store `ticket_id` values that correspond to rows in this database. The application layer (server) is responsible for creating workflow records only after the ticket exists here. There is no cascade delete; if a ticket is deleted, its workflow record in `workflow_db` becomes an orphan.

## Database

- Database name: `ur_tickets` (default)
- Config section: `[ticket_db]` in `ur.toml`
- Password override: `UR_TICKET_DB_PASSWORD` environment variable

## ui_events DDL

The `ui_events` table DDL used in this crate's migrations must stay identical to the copy in `workflow_db/migrations/` and the canonical `db_events::UI_EVENTS_DDL` constant. If the DDL changes, update all three locations: add a new migration here, add a new migration in `workflow_db`, and update `db_events::UI_EVENTS_DDL`.

## Conventions

- All database access is async via sqlx with a `PgPool`.
- Managers implement `Clone` and accept dependencies via constructor (dependency injection).
- Never modify existing migration files — always add new ones for schema changes.
- Migration files are embedded at compile time via `sqlx::migrate!()`.
