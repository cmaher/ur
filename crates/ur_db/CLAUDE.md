# ur_db

CozoDB-based ticket database for the unified ticket data model.

## Architecture

- `SchemaManager` — owns the CozoDB `DbInstance`, defines relations, provides raw query access
- `QueryManager` — structured Datalog queries (dispatch, DAG traversal, rollup, cycle detection, metadata)
- `BackupManager` — backup/restore via CozoDB's `backup_db()`/`restore_backup()` API

## CozoDB Backup Approach

- **SQLite backend**: `DbInstance::new("sqlite", path, "")` — single file on disk, auto-created
- **`backup_db()`**: Logical copy via read transaction snapshot. Always produces an SQLite file. Safe during concurrent writes (captures consistent snapshot, doesn't block writers). Target path must be empty — delete old backup before writing new one.
- **`restore_backup()`**: Only works on empty/fresh DB (store_id == 0). For disaster recovery, create a new instance, restore into it, then swap.
- **Recommended config** in `ur.toml`:
  ```toml
  [database]
  path = "~/.ur/db/tickets.db"
  backup_path = "~/.ur/backups/tickets.db"
  backup_interval_secs = 300
  ```
- **Rotation**: Delete-and-recreate (not append). No incremental/WAL backup support.
- **Gotchas**: One process per DB file (SQLite write lock). Full logical copy each time (fine for <10MB ticket DBs). Restore requires fresh instance.

## Datalog Patterns

- **Negation**: CozoDB requires a separate named rule for the set to negate against (`not rule_name[var]`)
- **Transitive closure**: Recursive rules with base case (direct edge) and recursive case (edge + recursive call)
- **Variable binding**: Output head variables must be bound in the body; use `id = ticket_id` for cross-relation joins

## Testing

```bash
cargo test -p ur_db
```

All queries use raw Datalog strings via `format!()`. Parameters are interpolated into the query text (safe for internal use with known ID formats).
