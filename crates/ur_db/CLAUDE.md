# ur_db

Async Postgres-backed ticket database, replacing the CozoDB-based ur_db crate.

## Architecture

- `DatabaseManager` — primary entry point. Holds an sqlx `PgPool`. Runs migrations on startup via `sqlx::migrate!()`. Injected into all other managers.
- `TicketRepo` — CRUD operations for tickets, activities, and metadata. Accepts `DatabaseManager` via constructor.
- `WorkflowRepo` — workflow lifecycle state, events, intents, and comments. Manages the workflow tables that track ticket workflow progression. Accepts `DatabaseManager` via constructor.
- `GraphManager` — dependency graph operations using petgraph. Loads edges from Postgres into a petgraph `DiGraph` for traversal, cycle detection, and topological sorting. Accepts `DatabaseManager` via constructor.
- `SnapshotManager` — point-in-time snapshots of ticket state for history/undo. Accepts `DatabaseManager` via constructor.
- `model` — shared data structs (Ticket, Activity, Edge, Meta, etc.) used across managers.

## Database Location

Postgres database, connected via URL (e.g., `postgres://user:pass@host:port/dbname`).

## Schema

Migrations (`migrations/`):
- `001_initial.sql` — creates `ticket`, `activity`, `meta`, `edge` tables
- `002_agent_slot.sql` — creates `slot` and `worker` tables
- `003_ticket_project.sql` — adds `project` column to `ticket`, backfills from ID prefix

**NEVER modify existing migration files.** The database is live — `sqlx::migrate!()` checksums applied migrations and will fail if they change. Always add new migrations for schema changes.

## Testing

```bash
cargo test -p ur_db
```

Tests connect to a Postgres database via `DATABASE_URL` env var (defaults to `postgres://ur:ur@localhost:5432/ur_test`). No mocks. CI uses a dedicated `ci-postgres` service on port 5433 to avoid conflicts with the development database. The `ur_db_test` crate provides a `TestDb` helper that creates isolated test databases per test run.

## Conventions

- All database access is async via sqlx.
- Graph operations load edges into petgraph in-memory graphs; the database is the source of truth.
- Managers follow the project-wide pattern: implement `Clone`, accept dependencies via constructor (dependency injection).
- The `meta` table uses `entity_type` to distinguish ticket metadata from activity metadata, avoiding separate tables.
- The `edge` table stores `relates_to` edges once (not duplicated) and queries with `WHERE source_id = ? OR target_id = ?`.
