# ur_db

CozoDB-based ticket database for the unified ticket data model.

## Architecture

- `DatabaseManager` — primary entry point. Holds the CozoDB `DbInstance` (internally Arc'd, so Clone is cheap). Creates all six relations on startup. Exposes `run()` for raw CozoScript queries. ur-server injects this at startup and passes it to managers that need database access.
- `QueryManager` — structured Datalog queries (dispatch, DAG traversal, rollup, cycle detection, metadata). Accepts `DatabaseManager` via constructor.
- `BackupManager` — backup/restore via CozoDB's `backup_db()`/`restore_backup()` API. Accepts `DatabaseManager` via constructor.

## Database Location

SQLite backend: `$UR_CONFIG/ur.db` (alongside `ur.toml`, default `~/.ur/ur.db`).

## Schema (six relations)

- `ticket` — primary entity, keyed by `id: String`
- `ticket_meta` — flexible key-value metadata per ticket, keyed by `(ticket_id, key)`
- `blocks` — hard dependency edges forming the dispatch DAG, keyed by `(blocker_id, blocked_id)`
- `relates_to` — soft informational links, keyed by `(left_id, right_id)`
- `activity` — timestamped updates on tickets, keyed by `id: String`
- `activity_meta` — flexible key-value metadata per activity, keyed by `(activity_id, key)`

## Datalog Patterns

- **Negation**: CozoDB requires a separate named rule for the set to negate against (`not rule_name[var]`)
- **Transitive closure**: Recursive rules with base case (direct edge) and recursive case (edge + recursive call)
- **Variable binding**: Output head variables must be bound in the body; use `id = ticket_id` for cross-relation joins

## Testing

```bash
cargo test -p ur_db
```

All queries use raw Datalog strings via `format!()`. Parameters are interpolated into the query text (safe for internal use with known ID formats).
