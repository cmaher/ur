# ur_db

Async SQLite-backed ticket database, replacing the CozoDB-based ur_db crate.

## Architecture

- `DatabaseManager` — primary entry point. Holds an sqlx `SqlitePool`. Runs migrations on startup via `sqlx::migrate!()`. Injected into all other managers.
- `TicketRepo` — CRUD operations for tickets, activities, and metadata. Accepts `DatabaseManager` via constructor.
- `GraphManager` — dependency graph operations using petgraph. Loads edges from SQLite into a petgraph `DiGraph` for traversal, cycle detection, and topological sorting. Accepts `DatabaseManager` via constructor.
- `SnapshotManager` — point-in-time snapshots of ticket state for history/undo. Accepts `DatabaseManager` via constructor.
- `model` — shared data structs (Ticket, Activity, Edge, Meta, etc.) used across managers.

## Database Location

SQLite file: `$UR_CONFIG/ur.db` (alongside `ur.toml`, default `~/.ur/ur.db`).

## Schema

Single migration (`migrations/001_initial.sql`) creates:
- `ticket` — primary entity table
- `activity` — timestamped updates on tickets
- `meta` — unified metadata table with `(entity_id, entity_type, key)` PK
- `edge` — unified edge table with `kind` discriminator (blocks, relates_to)

## Testing

```bash
cargo test -p ur_db
```

Tests use file-backed SQLite databases with unique names per test, cleaned up explicitly. No mocks.

## Conventions

- All database access is async via sqlx.
- Graph operations load edges into petgraph in-memory graphs; the database is the source of truth.
- Managers follow the project-wide pattern: implement `Clone`, accept dependencies via constructor (dependency injection).
- The `meta` table uses `entity_type` to distinguish ticket metadata from activity metadata, avoiding separate tables.
- The `edge` table stores `relates_to` edges once (not duplicated) and queries with `WHERE source_id = ? OR target_id = ?`.
