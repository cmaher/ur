use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{PgPool, Row, SqlitePool};

const DEFAULT_SQLITE_PATH: &str = "ur.db";
const DEFAULT_PG_URL: &str = "postgres://ur:ur@localhost:5432/ur";

/// Ordered list of tables for migration. Foreign-key dependencies are respected:
/// ticket first (topo-sorted by parent_id), then dependent tables.
const TABLE_ORDER: &[&str] = &[
    "ticket",
    "edge",
    "meta",
    "activity",
    "workflow",
    "workflow_intent",
    "workflow_event",
    "workflow_events",
    "workflow_comments",
    "worker",
    "slot",
    "worker_slot",
    "ticket_comments",
];

struct Args {
    source: String,
    target: String,
    dry_run: bool,
    verify: bool,
}

fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    let mut source = default_sqlite_path();
    let mut target = DEFAULT_PG_URL.to_string();
    let mut dry_run = false;
    let mut verify = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => {
                i += 1;
                source = args[i].clone();
            }
            "--target" => {
                i += 1;
                target = args[i].clone();
            }
            "--dry-run" => dry_run = true,
            "--verify" => verify = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                eprintln!("Unknown argument: {other}");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    Args {
        source,
        target,
        dry_run,
        verify,
    }
}

fn default_sqlite_path() -> String {
    if let Ok(config) = std::env::var("UR_CONFIG") {
        format!("{config}/ur.db")
    } else if let Ok(home) = std::env::var("HOME") {
        format!("{home}/.ur/ur.db")
    } else {
        DEFAULT_SQLITE_PATH.to_string()
    }
}

fn print_help() {
    println!("sqlite-to-pg: migrate ur data from SQLite to Postgres");
    println!();
    println!("Usage: sqlite-to-pg [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --source <PATH>  SQLite database path (default: $UR_CONFIG/ur.db or ~/.ur/ur.db)");
    println!("  --target <URL>   Postgres connection URL (default: {DEFAULT_PG_URL})");
    println!("  --dry-run        Connect to both databases and report row counts without writing");
    println!("  --verify         Compare row counts and spot-check data between databases");
    println!("  -h, --help       Show this help");
}

async fn connect_sqlite(path: &str) -> Result<SqlitePool, String> {
    let url = format!("sqlite:{path}?mode=ro");
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .map_err(|e| format!("Failed to connect to SQLite at {path}: {e}"))
}

async fn connect_pg(url: &str) -> Result<PgPool, String> {
    PgPool::connect(url)
        .await
        .map_err(|e| format!("Failed to connect to Postgres at {url}: {e}"))
}

async fn count_rows(sqlite: &SqlitePool, table: &str) -> Result<i64, String> {
    let query = format!("SELECT COUNT(*) as cnt FROM {table}");
    let row: (i64,) = sqlx::query_as(&query)
        .fetch_one(sqlite)
        .await
        .map_err(|e| format!("Failed to count {table}: {e}"))?;
    Ok(row.0)
}

async fn count_rows_pg(pg: &PgPool, table: &str) -> Result<i64, String> {
    let query = format!("SELECT COUNT(*) as cnt FROM {table}");
    let row: (i64,) = sqlx::query_as(&query)
        .fetch_one(pg)
        .await
        .map_err(|e| format!("Failed to count {table} in Postgres: {e}"))?;
    Ok(row.0)
}

async fn run_dry_run(sqlite: &SqlitePool, pg: &PgPool) -> Result<(), String> {
    println!("\n[DRY RUN] Row counts per table:");
    println!("{:<25} {:>10} {:>10}", "Table", "SQLite", "Postgres");
    println!("{:-<47}", "");

    for table in TABLE_ORDER {
        let sqlite_count = match count_rows(sqlite, table).await {
            Ok(c) => c,
            Err(_) => {
                println!("{:<25} {:>10} {:>10}", table, "(missing)", "-");
                continue;
            }
        };
        let pg_count = count_rows_pg(pg, table).await.unwrap_or(-1);
        let pg_str = if pg_count < 0 {
            "(missing)".to_string()
        } else {
            pg_count.to_string()
        };
        println!("{:<25} {:>10} {:>10}", table, sqlite_count, pg_str);
    }
    Ok(())
}

async fn run_verify(sqlite: &SqlitePool, pg: &PgPool) -> Result<(), String> {
    println!("\n[VERIFY] Comparing SQLite and Postgres data:");
    println!(
        "{:<25} {:>10} {:>10} {:>10}",
        "Table", "SQLite", "Postgres", "Status"
    );
    println!("{:-<57}", "");

    let mut all_ok = true;

    for table in TABLE_ORDER {
        let sqlite_count = match count_rows(sqlite, table).await {
            Ok(c) => c,
            Err(_) => {
                println!(
                    "{:<25} {:>10} {:>10} {:>10}",
                    table, "(missing)", "-", "SKIP"
                );
                continue;
            }
        };
        let pg_count = match count_rows_pg(pg, table).await {
            Ok(c) => c,
            Err(e) => {
                println!(
                    "{:<25} {:>10} {:>10} {:>10}",
                    table, sqlite_count, "ERR", "FAIL"
                );
                eprintln!("  Error: {e}");
                all_ok = false;
                continue;
            }
        };

        let status = if sqlite_count == pg_count {
            "OK"
        } else {
            all_ok = false;
            "MISMATCH"
        };
        println!(
            "{:<25} {:>10} {:>10} {:>10}",
            table, sqlite_count, pg_count, status
        );
    }

    // Spot-check: compare a few ticket IDs
    spot_check_tickets(sqlite, pg).await?;

    if all_ok {
        println!("\nVerification passed: all row counts match.");
    } else {
        println!("\nVerification FAILED: row count mismatches found.");
    }

    Ok(())
}

fn print_field_diff(field: &str, sqlite_val: &str, pg_val: &str) {
    if sqlite_val != pg_val {
        println!("    {field}: {sqlite_val:?} vs {pg_val:?}");
    }
}

async fn spot_check_tickets(sqlite: &SqlitePool, pg: &PgPool) -> Result<(), String> {
    println!("\nSpot-checking ticket data...");

    let rows = sqlx::query("SELECT id, title, status, project FROM ticket LIMIT 5")
        .fetch_all(sqlite)
        .await
        .map_err(|e| format!("Failed to fetch sample tickets from SQLite: {e}"))?;

    for row in &rows {
        let id: String = row.get("id");
        let title: String = row.get("title");
        let status: String = row.get("status");
        let project: String = row.get("project");

        let pg_row = sqlx::query("SELECT title, status, project FROM ticket WHERE id = $1")
            .bind(&id)
            .fetch_optional(pg)
            .await
            .map_err(|e| format!("Failed to query Postgres for ticket {id}: {e}"))?;

        let Some(pg_row) = pg_row else {
            println!("  {id}: MISSING in Postgres");
            continue;
        };

        let pg_title: String = pg_row.get("title");
        let pg_status: String = pg_row.get("status");
        let pg_project: String = pg_row.get("project");

        if title == pg_title && status == pg_status && project == pg_project {
            println!("  {id}: OK");
        } else {
            println!("  {id}: MISMATCH");
            print_field_diff("title", &title, &pg_title);
            print_field_diff("status", &status, &pg_status);
            print_field_diff("project", &project, &pg_project);
        }
    }

    Ok(())
}

async fn migrate_all(sqlite: &SqlitePool, pg: &PgPool) -> Result<(), String> {
    println!("\nMigrating data from SQLite to Postgres...\n");

    for table in TABLE_ORDER {
        let exists = table_exists_sqlite(sqlite, table).await;
        if !exists {
            println!("  {table}: skipped (not in SQLite)");
            continue;
        }

        let count = match *table {
            "ticket" => migrate_tickets(sqlite, pg).await?,
            "edge" => migrate_edge(sqlite, pg).await?,
            "meta" => migrate_meta(sqlite, pg).await?,
            "activity" => migrate_activity(sqlite, pg).await?,
            "workflow" => migrate_workflow(sqlite, pg).await?,
            "workflow_intent" => migrate_workflow_intent(sqlite, pg).await?,
            "workflow_event" => migrate_workflow_event(sqlite, pg).await?,
            "workflow_events" => migrate_workflow_events(sqlite, pg).await?,
            "workflow_comments" => migrate_workflow_comments(sqlite, pg).await?,
            "worker" => migrate_worker(sqlite, pg).await?,
            "slot" => migrate_slot(sqlite, pg).await?,
            "worker_slot" => migrate_worker_slot(sqlite, pg).await?,
            "ticket_comments" => migrate_ticket_comments(sqlite, pg).await?,
            _ => {
                println!("  {table}: skipped (no handler)");
                continue;
            }
        };
        println!("  {table}: {count} rows migrated");
    }

    println!("\nMigration complete.");
    Ok(())
}

async fn table_exists_sqlite(sqlite: &SqlitePool, table: &str) -> bool {
    let result = sqlx::query("SELECT name FROM sqlite_master WHERE type='table' AND name=?")
        .bind(table)
        .fetch_optional(sqlite)
        .await;
    matches!(result, Ok(Some(_)))
}

async fn migrate_tickets(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query(
        "SELECT id, type, status, priority, parent_id, title, body, \
         created_at, updated_at, project, lifecycle_status, branch, \
         lifecycle_managed FROM ticket",
    )
    .fetch_all(sqlite)
    .await
    .map_err(|e| format!("Failed to fetch tickets: {e}"))?;

    // Build topo order
    let mut id_to_idx = std::collections::HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        let id: String = row.get("id");
        id_to_idx.insert(id, i);
    }

    let mut order: Vec<usize> = Vec::new();
    let mut visited = vec![false; rows.len()];

    fn visit(
        idx: usize,
        rows: &[sqlx::sqlite::SqliteRow],
        id_to_idx: &std::collections::HashMap<String, usize>,
        visited: &mut [bool],
        order: &mut Vec<usize>,
    ) {
        if visited[idx] {
            return;
        }
        visited[idx] = true;
        let parent_id: Option<String> = rows[idx].get("parent_id");
        if let Some(ref pid) = parent_id
            && let Some(&pidx) = id_to_idx.get(pid)
        {
            visit(pidx, rows, id_to_idx, visited, order);
        }
        order.push(idx);
    }

    for i in 0..rows.len() {
        visit(i, &rows, &id_to_idx, &mut visited, &mut order);
    }

    let mut count: u64 = 0;
    for &idx in &order {
        let row = &rows[idx];
        let id: String = row.get("id");
        let type_: String = row.get("type");
        let status: String = row.get("status");
        let priority: i32 = row.get("priority");
        let parent_id: Option<String> = row.get("parent_id");
        let title: String = row.get("title");
        let body: String = row.get("body");
        let created_at: String = row.get("created_at");
        let updated_at: String = row.get("updated_at");
        let project: String = row.get("project");
        let lifecycle_status: String = row.get("lifecycle_status");
        let branch: Option<String> = row.get("branch");
        let lifecycle_managed: bool = row.get("lifecycle_managed");

        sqlx::query(
            "INSERT INTO ticket (id, type, status, priority, parent_id, title, body, \
             created_at, updated_at, project, lifecycle_status, branch, lifecycle_managed) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
             ON CONFLICT (id) DO UPDATE SET \
             type = EXCLUDED.type, status = EXCLUDED.status, priority = EXCLUDED.priority, \
             parent_id = EXCLUDED.parent_id, title = EXCLUDED.title, body = EXCLUDED.body, \
             created_at = EXCLUDED.created_at, updated_at = EXCLUDED.updated_at, \
             project = EXCLUDED.project, lifecycle_status = EXCLUDED.lifecycle_status, \
             branch = EXCLUDED.branch, lifecycle_managed = EXCLUDED.lifecycle_managed",
        )
        .bind(&id)
        .bind(&type_)
        .bind(&status)
        .bind(priority)
        .bind(&parent_id)
        .bind(&title)
        .bind(&body)
        .bind(&created_at)
        .bind(&updated_at)
        .bind(&project)
        .bind(&lifecycle_status)
        .bind(&branch)
        .bind(lifecycle_managed)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert ticket {id}: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_edge(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query("SELECT source_id, target_id, kind FROM edge")
        .fetch_all(sqlite)
        .await
        .map_err(|e| format!("Failed to fetch edges: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let source_id: String = row.get("source_id");
        let target_id: String = row.get("target_id");
        let kind: String = row.get("kind");

        sqlx::query(
            "INSERT INTO edge (source_id, target_id, kind) VALUES ($1, $2, $3) \
             ON CONFLICT (source_id, target_id, kind) DO NOTHING",
        )
        .bind(&source_id)
        .bind(&target_id)
        .bind(&kind)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert edge {source_id}->{target_id}: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_meta(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query("SELECT entity_id, entity_type, key, value FROM meta")
        .fetch_all(sqlite)
        .await
        .map_err(|e| format!("Failed to fetch meta: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let entity_id: String = row.get("entity_id");
        let entity_type: String = row.get("entity_type");
        let key: String = row.get("key");
        let value: String = row.get("value");

        sqlx::query(
            "INSERT INTO meta (entity_id, entity_type, key, value) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (entity_id, entity_type, key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(&entity_id)
        .bind(&entity_type)
        .bind(&key)
        .bind(&value)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert meta: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_activity(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query("SELECT id, ticket_id, timestamp, author, message FROM activity")
        .fetch_all(sqlite)
        .await
        .map_err(|e| format!("Failed to fetch activities: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let id: String = row.get("id");
        let ticket_id: String = row.get("ticket_id");
        let timestamp: String = row.get("timestamp");
        let author: String = row.get("author");
        let message: String = row.get("message");

        sqlx::query(
            "INSERT INTO activity (id, ticket_id, \"timestamp\", author, message) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (id) DO UPDATE SET \
             ticket_id = EXCLUDED.ticket_id, \"timestamp\" = EXCLUDED.\"timestamp\", \
             author = EXCLUDED.author, message = EXCLUDED.message",
        )
        .bind(&id)
        .bind(&ticket_id)
        .bind(&timestamp)
        .bind(&author)
        .bind(&message)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert activity {id}: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_workflow(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query(
        "SELECT id, ticket_id, status, created_at, stalled, stall_reason, \
         implement_cycles, worker_id, noverify, feedback_mode, \
         ci_status, mergeable, review_status FROM workflow",
    )
    .fetch_all(sqlite)
    .await
    .map_err(|e| format!("Failed to fetch workflows: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let id: String = row.get("id");
        let ticket_id: String = row.get("ticket_id");
        let status: String = row.get("status");
        let created_at: String = row.get("created_at");
        let stalled: i32 = row.get("stalled");
        let stall_reason: String = row.get("stall_reason");
        let implement_cycles: i32 = row.get("implement_cycles");
        let worker_id: String = row.get("worker_id");
        let noverify: i32 = row.get("noverify");
        let feedback_mode: String = row.get("feedback_mode");
        let ci_status: String = row.get("ci_status");
        let mergeable: String = row.get("mergeable");
        let review_status: String = row.get("review_status");

        sqlx::query(
            "INSERT INTO workflow (id, ticket_id, status, created_at, stalled, stall_reason, \
             implement_cycles, worker_id, noverify, feedback_mode, ci_status, mergeable, \
             review_status) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
             ON CONFLICT (id) DO UPDATE SET \
             ticket_id = EXCLUDED.ticket_id, status = EXCLUDED.status, \
             created_at = EXCLUDED.created_at, stalled = EXCLUDED.stalled, \
             stall_reason = EXCLUDED.stall_reason, \
             implement_cycles = EXCLUDED.implement_cycles, worker_id = EXCLUDED.worker_id, \
             noverify = EXCLUDED.noverify, feedback_mode = EXCLUDED.feedback_mode, \
             ci_status = EXCLUDED.ci_status, mergeable = EXCLUDED.mergeable, \
             review_status = EXCLUDED.review_status",
        )
        .bind(&id)
        .bind(&ticket_id)
        .bind(&status)
        .bind(&created_at)
        .bind(stalled)
        .bind(&stall_reason)
        .bind(implement_cycles)
        .bind(&worker_id)
        .bind(noverify)
        .bind(&feedback_mode)
        .bind(&ci_status)
        .bind(&mergeable)
        .bind(&review_status)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert workflow {id}: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_workflow_intent(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query("SELECT id, ticket_id, target_status, created_at FROM workflow_intent")
        .fetch_all(sqlite)
        .await
        .map_err(|e| format!("Failed to fetch workflow_intent: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let id: String = row.get("id");
        let ticket_id: String = row.get("ticket_id");
        let target_status: String = row.get("target_status");
        let created_at: String = row.get("created_at");

        sqlx::query(
            "INSERT INTO workflow_intent (id, ticket_id, target_status, created_at) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (id) DO UPDATE SET \
             ticket_id = EXCLUDED.ticket_id, target_status = EXCLUDED.target_status, \
             created_at = EXCLUDED.created_at",
        )
        .bind(&id)
        .bind(&ticket_id)
        .bind(&target_status)
        .bind(&created_at)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert workflow_intent {id}: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_workflow_event(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query(
        "SELECT id, ticket_id, old_lifecycle_status, new_lifecycle_status, attempts, \
         created_at FROM workflow_event",
    )
    .fetch_all(sqlite)
    .await
    .map_err(|e| format!("Failed to fetch workflow_event: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let id: String = row.get("id");
        let ticket_id: String = row.get("ticket_id");
        let old_status: String = row.get("old_lifecycle_status");
        let new_status: String = row.get("new_lifecycle_status");
        let attempts: i32 = row.get("attempts");
        let created_at: String = row.get("created_at");

        sqlx::query(
            "INSERT INTO workflow_event (id, ticket_id, old_lifecycle_status, \
             new_lifecycle_status, attempts, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (id) DO UPDATE SET \
             ticket_id = EXCLUDED.ticket_id, \
             old_lifecycle_status = EXCLUDED.old_lifecycle_status, \
             new_lifecycle_status = EXCLUDED.new_lifecycle_status, \
             attempts = EXCLUDED.attempts, created_at = EXCLUDED.created_at",
        )
        .bind(&id)
        .bind(&ticket_id)
        .bind(&old_status)
        .bind(&new_status)
        .bind(attempts)
        .bind(&created_at)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert workflow_event {id}: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_workflow_events(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query("SELECT id, workflow_id, event, created_at FROM workflow_events")
        .fetch_all(sqlite)
        .await
        .map_err(|e| format!("Failed to fetch workflow_events: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let id: String = row.get("id");
        let workflow_id: String = row.get("workflow_id");
        let event: String = row.get("event");
        let created_at: String = row.get("created_at");

        sqlx::query(
            "INSERT INTO workflow_events (id, workflow_id, event, created_at) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (id) DO UPDATE SET \
             workflow_id = EXCLUDED.workflow_id, event = EXCLUDED.event, \
             created_at = EXCLUDED.created_at",
        )
        .bind(&id)
        .bind(&workflow_id)
        .bind(&event)
        .bind(&created_at)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert workflow_events {id}: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_workflow_comments(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query(
        "SELECT ticket_id, comment_id, feedback_created, created_at FROM workflow_comments",
    )
    .fetch_all(sqlite)
    .await
    .map_err(|e| format!("Failed to fetch workflow_comments: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let ticket_id: String = row.get("ticket_id");
        let comment_id: String = row.get("comment_id");
        let feedback_created: i32 = row.get("feedback_created");
        let created_at: String = row.get("created_at");

        sqlx::query(
            "INSERT INTO workflow_comments (ticket_id, comment_id, feedback_created, created_at) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (ticket_id, comment_id) DO UPDATE SET \
             feedback_created = EXCLUDED.feedback_created, created_at = EXCLUDED.created_at",
        )
        .bind(&ticket_id)
        .bind(&comment_id)
        .bind(feedback_created)
        .bind(&created_at)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert workflow_comments: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_worker(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query(
        "SELECT worker_id, process_id, project_key, container_id, worker_secret, \
         strategy, container_status, agent_status, workspace_path, \
         created_at, updated_at, idle_redispatch_count FROM worker",
    )
    .fetch_all(sqlite)
    .await
    .map_err(|e| format!("Failed to fetch workers: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let worker_id: String = row.get("worker_id");
        let process_id: String = row.get("process_id");
        let project_key: String = row.get("project_key");
        let container_id: String = row.get("container_id");
        let worker_secret: String = row.get("worker_secret");
        let strategy: String = row.get("strategy");
        let container_status: String = row.get("container_status");
        let agent_status: String = row.get("agent_status");
        let workspace_path: Option<String> = row.get("workspace_path");
        let created_at: String = row.get("created_at");
        let updated_at: String = row.get("updated_at");
        let idle_redispatch_count: i32 = row.get("idle_redispatch_count");

        sqlx::query(
            "INSERT INTO worker (worker_id, process_id, project_key, container_id, \
             worker_secret, strategy, container_status, agent_status, workspace_path, \
             created_at, updated_at, idle_redispatch_count) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             ON CONFLICT (worker_id) DO UPDATE SET \
             process_id = EXCLUDED.process_id, project_key = EXCLUDED.project_key, \
             container_id = EXCLUDED.container_id, worker_secret = EXCLUDED.worker_secret, \
             strategy = EXCLUDED.strategy, container_status = EXCLUDED.container_status, \
             agent_status = EXCLUDED.agent_status, workspace_path = EXCLUDED.workspace_path, \
             created_at = EXCLUDED.created_at, updated_at = EXCLUDED.updated_at, \
             idle_redispatch_count = EXCLUDED.idle_redispatch_count",
        )
        .bind(&worker_id)
        .bind(&process_id)
        .bind(&project_key)
        .bind(&container_id)
        .bind(&worker_secret)
        .bind(&strategy)
        .bind(&container_status)
        .bind(&agent_status)
        .bind(&workspace_path)
        .bind(&created_at)
        .bind(&updated_at)
        .bind(idle_redispatch_count)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert worker {worker_id}: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_slot(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query(
        "SELECT id, project_key, slot_name, host_path, created_at, updated_at FROM slot",
    )
    .fetch_all(sqlite)
    .await
    .map_err(|e| format!("Failed to fetch slots: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let id: String = row.get("id");
        let project_key: String = row.get("project_key");
        let slot_name: String = row.get("slot_name");
        let host_path: String = row.get("host_path");
        let created_at: String = row.get("created_at");
        let updated_at: String = row.get("updated_at");

        sqlx::query(
            "INSERT INTO slot (id, project_key, slot_name, host_path, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (id) DO UPDATE SET \
             project_key = EXCLUDED.project_key, slot_name = EXCLUDED.slot_name, \
             host_path = EXCLUDED.host_path, created_at = EXCLUDED.created_at, \
             updated_at = EXCLUDED.updated_at",
        )
        .bind(&id)
        .bind(&project_key)
        .bind(&slot_name)
        .bind(&host_path)
        .bind(&created_at)
        .bind(&updated_at)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert slot {id}: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_worker_slot(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query("SELECT worker_id, slot_id, created_at FROM worker_slot")
        .fetch_all(sqlite)
        .await
        .map_err(|e| format!("Failed to fetch worker_slot: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let worker_id: String = row.get("worker_id");
        let slot_id: String = row.get("slot_id");
        let created_at: String = row.get("created_at");

        sqlx::query(
            "INSERT INTO worker_slot (worker_id, slot_id, created_at) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (worker_id, slot_id) DO UPDATE SET \
             created_at = EXCLUDED.created_at",
        )
        .bind(&worker_id)
        .bind(&slot_id)
        .bind(&created_at)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert worker_slot: {e}"))?;
        count += 1;
    }

    Ok(count)
}

async fn migrate_ticket_comments(sqlite: &SqlitePool, pg: &PgPool) -> Result<u64, String> {
    let rows = sqlx::query(
        "SELECT comment_id, ticket_id, pr_number, gh_repo, reply_posted, \
         created_at FROM ticket_comments",
    )
    .fetch_all(sqlite)
    .await
    .map_err(|e| format!("Failed to fetch ticket_comments: {e}"))?;

    let mut count: u64 = 0;
    for row in &rows {
        let comment_id: String = row.get("comment_id");
        let ticket_id: String = row.get("ticket_id");
        let pr_number: i32 = row.get("pr_number");
        let gh_repo: String = row.get("gh_repo");
        let reply_posted: i32 = row.get("reply_posted");
        let created_at: String = row.get("created_at");

        sqlx::query(
            "INSERT INTO ticket_comments (comment_id, ticket_id, pr_number, gh_repo, \
             reply_posted, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (comment_id, ticket_id) DO UPDATE SET \
             pr_number = EXCLUDED.pr_number, gh_repo = EXCLUDED.gh_repo, \
             reply_posted = EXCLUDED.reply_posted, created_at = EXCLUDED.created_at",
        )
        .bind(&comment_id)
        .bind(&ticket_id)
        .bind(pr_number)
        .bind(&gh_repo)
        .bind(reply_posted)
        .bind(&created_at)
        .execute(pg)
        .await
        .map_err(|e| format!("Failed to insert ticket_comments: {e}"))?;
        count += 1;
    }

    Ok(count)
}

#[tokio::main]
async fn main() {
    let args = parse_args();

    println!("sqlite-to-pg: SQLite to Postgres data migration");
    println!("  Source (SQLite): {}", args.source);
    println!("  Target (Postgres): {}", args.target);
    if args.dry_run {
        println!("  Mode: DRY RUN");
    } else if args.verify {
        println!("  Mode: VERIFY");
    } else {
        println!("  Mode: MIGRATE");
    }
    println!();

    let sqlite = match connect_sqlite(&args.source).await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("Connected to SQLite.");

    let pg = match connect_pg(&args.target).await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("Connected to Postgres.");

    let result = if args.dry_run {
        run_dry_run(&sqlite, &pg).await
    } else if args.verify {
        run_verify(&sqlite, &pg).await
    } else {
        migrate_all(&sqlite, &pg).await
    };

    if let Err(e) = result {
        eprintln!("\nError: {e}");
        std::process::exit(1);
    }
}
