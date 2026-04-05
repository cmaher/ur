# Database Lifecycle

How the Postgres database is initialized, migrated, queried, backed up, and shut down.

## Overview

The ticket system uses a Postgres database managed by sqlx with async access via `PgPool`, automatic migrations, and optional periodic backups via `pg_dump` into the `ur-postgres` container.

## Connection URL Resolution

The server connects to Postgres via a connection URL (e.g., `postgres://ur:ur@ur-postgres:5432/ur`).

```
init_database(cfg)
  → std::env::var("DATABASE_URL")
    → falls back to cfg.db.database_url()
      → DatabaseManager::open(&url)
```

`DatabaseConfig` in `ur_config` holds host, port, user, password, and name fields with a `database_url()` method that constructs the full Postgres URL. Defaults: host=`ur-postgres`, port=`5432`, user=`ur`, password=`ur`, name=`ur`.

## Initialization and Migration

On startup (`crates/server/src/main.rs`):

1. `DatabaseManager::open(url)` creates an sqlx `PgPool`
2. `sqlx::migrate!().run(&pool)` applies pending migrations from `crates/ur_db/migrations/`
3. Migrations are embedded at compile time via the `sqlx::migrate!()` macro

Current migrations:
- `001_initial.sql` — consolidated schema: creates all tables (`ticket`, `edge`, `meta`, `activity`, `slot`, `worker`, `worker_slot`, `workflow`, `workflow_event`, `workflow_intent`, `workflow_comments`, `workflow_events`, `ui_events`, `ticket_comments`), indexes, triggers (lifecycle change, UI event with ancestor propagation, `pg_notify`)

Adding a new migration: create `migrations/NNN_<name>.sql`. The migrate macro picks it up automatically at next compile. **Never modify existing migration files** — sqlx checksums applied migrations and will fail if they change.

## Schema

```
ticket (id PK, type, status, priority, parent_id FK→ticket, title, body, created_at, updated_at, project, lifecycle_status, branch, lifecycle_managed)
edge (source_id FK, target_id FK, kind) — PK: (source_id, target_id, kind)
meta (entity_id, entity_type, key, value) — PK: (entity_id, entity_type, key)
activity (id PK, ticket_id FK, timestamp, author, message)
slot (id PK, project_key, slot_name, host_path, created_at, updated_at)
worker (worker_id PK, process_id, project_key, container_id, worker_secret, strategy, container_status, agent_status, workspace_path, created_at, updated_at, idle_redispatch_count)
worker_slot (worker_id FK, slot_id FK) — PK: (worker_id, slot_id)
workflow (id PK, ticket_id FK, status, created_at, stalled, stall_reason, implement_cycles, worker_id, noverify, feedback_mode, ci_status, mergeable, review_status)
workflow_event (id PK, ticket_id FK, old_lifecycle_status, new_lifecycle_status, attempts, created_at)
workflow_intent (id PK, ticket_id FK, target_status, created_at)
workflow_comments (ticket_id FK, comment_id) — PK: (ticket_id, comment_id)
workflow_events (id PK, workflow_id FK, event, created_at)
ui_events (id BIGSERIAL PK, entity_type, entity_id, created_at)
ticket_comments (comment_id, ticket_id FK, pr_number, gh_repo, reply_posted, created_at) — PK: (comment_id, ticket_id)

Indexes: idx_ticket_parent_id, idx_ticket_project_priority, idx_ticket_status, idx_edge_target, idx_meta_lookup, idx_activity_ticket_id, idx_slot_project, idx_worker_container_status, idx_worker_process_id, idx_workflow_ticket_id, idx_workflow_status, idx_workflow_event_ticket_id, idx_workflow_event_created_at, idx_workflow_intent_ticket_id, idx_workflow_intent_created_at, idx_workflow_comments_ticket_id, idx_workflow_events_workflow_created, idx_ticket_comments_pending (partial)
```

## Component Interactions

```
DatabaseManager
  ├── owns PgPool
  ├── runs migrations on open
  │
  ├── pool() → PgPool (shared via clone)
  │     │
  │     ├── GraphManager(pool)
  │     │     └── builds petgraph DiGraph from edge table
  │     │         (transitive_blockers, transitive_dependents, would_create_cycle)
  │     │
  │     ├── TicketRepo(pool, GraphManager)
  │     │     └── CRUD: tickets, activities, metadata, edges
  │     │         dispatchable_tickets (uses GraphManager for blocker check)
  │     │
  │     ├── WorkflowRepo(pool)
  │     │     └── workflow lifecycle, events, intents, comments
  │     │
  │     ├── WorkerRepo(pool)
  │     │     └── worker and slot management
  │     │
  │     ├── UiEventRepo(pool)
  │     │     └── poll and delete ui_events rows
  │     │
  │     └── SnapshotManager(container_command, container_name, db_name)
  │           └── dump_to(filename) — pg_dump -Fc inside ur-postgres container
  │               restore_from(filename) — pg_restore --clean inside ur-postgres container
  │
  └── (pool is shared, not owned exclusively by any manager)
```

### Wiring in main.rs

```
let db = DatabaseManager::open(&database_url).await?;

// Backup subsystem
let snapshot_manager = SnapshotManager::new(container_command, container_name, db_name);
let backup_task_manager = BackupTaskManager::new(snapshot_manager, cfg.db.backup.clone());
backup_task_manager.spawn(shutdown_rx)?;

// Ticket subsystem
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
| `add_edge` / `remove_edge` / `edges_for` | INSERT ON CONFLICT DO NOTHING/DELETE/SELECT on edge table |
| `add_activity` / `get_activities` | INSERT/SELECT on activity table |
| `tickets_by_metadata` / `tickets_with_metadata_key` | JOIN ticket+meta for metadata queries |
| `dispatchable_tickets` | Open children of epic with no open transitive blockers |
| `epic_all_children_closed` | COUNT non-closed children |

## Periodic Backup Flow

Configured via `[db.backup]` section in `ur.toml` (legacy `[backup]` also supported):

```toml
[db.backup]
path = "/path/to/backup/dir"    # omit to disable
interval_minutes = 30            # default: 30
retain_count = 3                 # default: 3
```

The backup path on the host is mounted at `/backup` in the `ur-postgres` container via Docker Compose.

### Startup

1. `BackupTaskManager::new(snapshot_manager, backup_config)` — stores config and snapshot manager
2. `backup_task_manager.spawn(shutdown_rx)` — validates path, spawns tokio task
   - Returns `None` if `path` is not configured (backup disabled)
   - Returns `None` if `enabled = false`
   - Returns `None` (with warning) if path doesn't exist, isn't a directory, or isn't writable

### Backup Loop

```
loop {
    tokio::select! {
        sleep(interval) => {
            1. Generate timestamped filename: ur-backup-YYYYMMDDTHHMMSSz.pgdump
            2. SnapshotManager::dump_to(filename)
               → docker exec ur-postgres pg_dump -Fc -f /backup/<filename> <dbname>
            3. On success: clean_old_backups() removes older ur-backup-*.pgdump files
               (keeps retain_count most recent)
            4. On failure: log error, continue loop
        }
        shutdown_rx.changed() => {
            if shutdown signaled: run final backup, then return
        }
    }
}
```

### Restore

`SnapshotManager::restore_from(filename)`:
1. Runs `docker exec ur-postgres pg_restore --clean --if-exists -d <dbname> /backup/<filename>`
2. Restores directly into the live database, replacing existing data

### Manual Backup

`BackupTaskManager::run_once()` creates a manual backup with `manual-` prefix (`manual-ur-backup-*.pgdump`). Manual backups are excluded from automatic retention cleanup.

## Shutdown

In `main.rs`, after the gRPC server exits:

```
let _ = shutdown_tx.send(true);       // signal backup task to stop
if let Some(handle) = backup_handle {
    let _ = handle.await;             // wait for clean exit (includes final backup)
}
```

The `PgPool` is dropped when `DatabaseManager` goes out of scope, closing all connections.

## Key Files

- Database manager: `crates/ur_db/src/database.rs`
- Ticket repo: `crates/ur_db/src/ticket_repo.rs`
- Graph manager: `crates/ur_db/src/graph.rs`
- Snapshot manager: `crates/ur_db/src/snapshot.rs`
- Data models: `crates/ur_db/src/model.rs`
- Migration: `crates/ur_db/migrations/001_initial.sql`
- Backup task: `crates/server/src/backup.rs`
- Database config: `crates/ur_config/src/lib.rs` (`DatabaseConfig`, `BackupConfig`)
- Server wiring: `crates/server/src/main.rs`
