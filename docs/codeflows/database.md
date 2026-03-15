# Database Lifecycle

How the SQLite database is initialized, migrated, queried, backed up, and shut down.

## Overview

The ticket system uses a single SQLite file (`ur.db`) managed by sqlx with async access, automatic migrations, and optional periodic backups via `VACUUM INTO`.

## DB Path Resolution

The database file lives alongside the config file at `$UR_CONFIG/ur.db` (default `~/.ur/ur.db`).

```
Config::load()
  → cfg.config_dir          (e.g., ~/.ur/)
    → cfg.config_dir.join("ur.db")
      → DatabaseManager::open(&db_path_str)
```

`DatabaseManager::open()` passes `create_if_missing(true)` to sqlx, so the file is created on first run. Foreign keys are enabled at connection time.

## Initialization and Migration

On startup (`crates/server/src/main.rs`):

1. `DatabaseManager::open(path)` creates an sqlx `SqlitePool` (max 5 connections)
2. `sqlx::migrate!().run(&pool)` applies pending migrations from `crates/ur_db/migrations/`
3. Migrations are embedded at compile time via the `sqlx::migrate!()` macro

Current migrations:
- `001_initial.sql` — creates `ticket`, `edge`, `meta`, `activity` tables plus indexes

Adding a new migration: create `migrations/NNN_<name>.sql`. The migrate macro picks it up automatically at next compile.

## Schema

```
ticket (id PK, type, status, priority, parent_id FK→ticket, title, body, created_at, updated_at)
edge (source_id FK, target_id FK, kind) — PK: (source_id, target_id, kind)
meta (entity_id, entity_type, key, value) — PK: (entity_id, entity_type, key)
activity (id PK, ticket_id FK, timestamp, author, message)

Indexes: idx_edge_target(target_id, kind), idx_activity_ticket_id(ticket_id), idx_meta_lookup(entity_type, key, value)
```

## Component Interactions

```
DatabaseManager
  ├── owns SqlitePool (max 5 connections, WAL mode)
  ├── runs migrations on open
  │
  ├── pool() → SqlitePool (shared via clone)
  │     │
  │     ├── GraphManager(pool)
  │     │     └── builds petgraph DiGraph from edge table
  │     │         (transitive_blockers, transitive_dependents, would_create_cycle)
  │     │
  │     ├── TicketRepo(pool, GraphManager)
  │     │     └── CRUD: tickets, activities, metadata, edges
  │     │         dispatchable_tickets (uses GraphManager for blocker check)
  │     │
  │     └── SnapshotManager(pool)
  │           └── vacuum_into(path) — VACUUM INTO for consistent backup
  │               restore(source, target) — copy + reopen with migrations
  │
  └── (pool is shared, not owned exclusively by any manager)
```

### Wiring in main.rs

```
let db = DatabaseManager::open(&db_path_str).await?;

// Backup subsystem
let snapshot_manager = SnapshotManager::new(db.pool().clone());
let backup_task_manager = BackupTaskManager::new(snapshot_manager, cfg.backup.clone());
backup_task_manager.spawn(shutdown_rx)?;

// Ticket subsystem (behind "ticket" feature flag)
let graph_manager = GraphManager::new(db.pool().clone());
let ticket_repo = TicketRepo::new(db.pool().clone(), graph_manager);
// → injected into TicketServiceHandler for gRPC
```

All managers receive the pool via constructor injection. `DatabaseManager` is not passed around — only its pool is cloned and distributed.

## TicketRepo Queries

`TicketRepo` is the primary data access layer, exposed via `TicketServiceHandler` over gRPC.

| Method | Operation |
|--------|-----------|
| `create_ticket` | INSERT into ticket |
| `get_ticket` | SELECT by id |
| `update_ticket` | Fetch-then-UPDATE (partial update via TicketUpdate) |
| `list_tickets` | Dynamic WHERE from TicketFilter (status, type, parent_id) |
| `set_meta` / `get_meta` / `delete_meta` | UPSERT/SELECT/DELETE on meta table |
| `add_edge` / `remove_edge` / `edges_for` | INSERT OR IGNORE/DELETE/SELECT on edge table |
| `add_activity` / `get_activities` | INSERT/SELECT on activity table |
| `tickets_by_metadata` / `tickets_with_metadata_key` | JOIN ticket+meta for metadata queries |
| `dispatchable_tickets` | Open children of epic with no open transitive blockers |
| `epic_all_children_closed` | COUNT non-closed children |

## Periodic Backup Flow

Configured via `[backup]` section in `ur.toml`:

```toml
[backup]
path = "/path/to/backup/dir"    # omit to disable
interval_minutes = 30            # default: 30
```

### Startup

1. `BackupTaskManager::new(snapshot_manager, backup_config)` — stores config and snapshot manager
2. `backup_task_manager.spawn(shutdown_rx)` — validates path, spawns tokio task
   - Returns `None` if `path` is not configured (backup disabled)
   - Returns `Err` if path doesn't exist, isn't a directory, or isn't writable

### Backup Loop

```
loop {
    tokio::select! {
        sleep(interval) => {
            1. Generate timestamped filename: ur-backup-YYYYMMDDTHHMMSSz.db
            2. SnapshotManager::vacuum_into(backup_dir/filename)
               → SQLite VACUUM INTO (consistent, no WAL dependency)
            3. On success: clean_old_backups() removes older ur-backup-*.db files
               (keeps only the latest)
            4. On failure: log error, continue loop
        }
        shutdown_rx.changed() => {
            if shutdown signaled: return (task exits)
        }
    }
}
```

### Restore

`SnapshotManager::restore(source_path, target_path)`:
1. Validates source exists and target does not
2. Copies snapshot file to target path
3. Opens the copy with `DatabaseManager::open()` (re-runs migrations to verify schema integrity)
4. Returns the new `DatabaseManager`

## Shutdown

In `main.rs`, after the gRPC server exits:

```
let _ = shutdown_tx.send(true);       // signal backup task to stop
if let Some(handle) = backup_handle {
    let _ = handle.await;             // wait for clean exit
}
```

The sqlx `SqlitePool` is dropped when `DatabaseManager` goes out of scope, closing all connections.

## Key Files

- Database manager: `crates/ur_db/src/database.rs`
- Ticket repo: `crates/ur_db/src/ticket_repo.rs`
- Graph manager: `crates/ur_db/src/graph.rs`
- Snapshot manager: `crates/ur_db/src/snapshot.rs`
- Data models: `crates/ur_db/src/model.rs`
- Migration: `crates/ur_db/migrations/001_initial.sql`
- Backup task: `crates/server/src/backup.rs`
- Backup config: `crates/ur_config/src/lib.rs` (`BackupConfig`, `RawBackupConfig`)
- Server wiring: `crates/server/src/main.rs`
