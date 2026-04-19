# workflow_db

Postgres-backed workflow database crate. Owns the workflow lifecycle schema: workflow state, events, intents, comments, and the workflow-side `ui_events` trigger infrastructure.

## Responsibilities

- Migrations for the workflow domain (`workflow`, `workflow_event`, `workflow_intent`, `workflow_comments`, `workflow_events`, `ui_events` tables and their triggers).
- `DatabaseManager` — opens a `PgPool` for `ur_workflow`, runs migrations on startup.
- `WorkflowRepo` — CRUD operations for workflow state, events, intents, and comments.

## What Does NOT Live Here

- Ticket data (`ticket`, `activity`, `meta`, `edge`, `slot`, `worker`) — those live in `ticket_db`.
- Shared event infrastructure (`PgEventPoller`, `UI_EVENTS_CHANNEL`, `UI_EVENTS_DDL`) — those live in `db_events`.
- Cross-DB foreign keys — this crate references `ticket_id` values from `ticket_db` as plain TEXT soft references; no DB-level FK exists.

## Cross-DB References

The `workflow` table stores a `ticket_id` that corresponds to a row in `ticket_db.ticket`. Because the two crates connect to different Postgres databases (`ur_workflow` vs `ur_tickets`), there is no database-level foreign key enforcing this relationship. The application layer (server) creates workflow records only after the ticket exists in `ticket_db`. No cascade delete exists; deleting a ticket in `ticket_db` leaves its workflow record as an orphan here.

## Database

- Database name: `ur_workflow` (default)
- Config section: `[workflow_db]` in `ur.toml`
- Password override: `UR_WORKFLOW_DB_PASSWORD` environment variable

## ui_events DDL

The `ui_events` table DDL used in this crate's migrations must stay identical to the copy in `ticket_db/migrations/` and the canonical `db_events::UI_EVENTS_DDL` constant. If the DDL changes, update all three locations: add a new migration here, add a new migration in `ticket_db`, and update `db_events::UI_EVENTS_DDL`.

## Conventions

- All database access is async via sqlx with a `PgPool`.
- Managers implement `Clone` and accept dependencies via constructor (dependency injection).
- Never modify existing migration files — always add new ones for schema changes.
- Migration files are embedded at compile time via `sqlx::migrate!()`.
