// workflow_db: Postgres-backed workflow database crate.

pub mod model;
pub mod ui_event_repo;
pub mod worker_repo;
pub mod workflow_repo;

use sqlx::PgPool;

pub use model::{
    AgentStatus, Slot, Worker, WorkerSlot, Workflow, WorkflowEvent, WorkflowEventRow,
    WorkflowIntent,
};
pub use ui_event_repo::UiEventRepo;
pub use worker_repo::{SlotReconcileResult, WorkerRepo};
pub use workflow_repo::{PaginatedWorkflow, WorkflowRepo};

/// Run all pending workflow_db migrations against the given pool.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!().run(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;
    use uuid::Uuid;

    use ur_db_test::ci_postgres_url;

    async fn admin_pool() -> PgPool {
        let options = PgConnectOptions::from_str(&ci_postgres_url("postgres"))
            .expect("invalid ci-postgres connection string");
        PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap_or_else(|_| {
                panic!("Cannot connect to ci-postgres (port UR_CI_POSTGRES_PORT, default 5433). Run: make test-init")
            })
    }

    /// Verify that migrations apply cleanly and are idempotent (running twice does not error).
    #[tokio::test]
    async fn migration_is_idempotent() {
        let admin = admin_pool().await;
        let db_name = format!("workflow_db_test_{}", Uuid::new_v4().simple());

        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{db_name}\""
        )))
        .execute(&admin)
        .await
        .expect("failed to create test database");

        let db_url = ci_postgres_url(&db_name);
        let options =
            PgConnectOptions::from_str(&db_url).expect("invalid test database connection string");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .expect("failed to connect to test database");

        // First run: apply migrations.
        migrate(&pool).await.expect("first migration run failed");

        // Verify all expected tables are present.
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name NOT LIKE '_sqlx%' \
             ORDER BY table_name",
        )
        .fetch_all(&pool)
        .await
        .expect("should query tables");

        let names: Vec<&str> = tables.iter().map(|t| t.0.as_str()).collect();
        assert!(names.contains(&"slot"), "missing slot table");
        assert!(names.contains(&"worker"), "missing worker table");
        assert!(names.contains(&"worker_slot"), "missing worker_slot table");
        assert!(names.contains(&"workflow"), "missing workflow table");
        assert!(
            names.contains(&"workflow_event"),
            "missing workflow_event table"
        );
        assert!(
            names.contains(&"workflow_intent"),
            "missing workflow_intent table"
        );
        assert!(
            names.contains(&"workflow_comments"),
            "missing workflow_comments table"
        );
        assert!(
            names.contains(&"workflow_events"),
            "missing workflow_events table"
        );
        assert!(names.contains(&"ui_events"), "missing ui_events table");

        // Verify no node_id columns exist anywhere.
        let node_id_cols: Vec<(String, String)> = sqlx::query_as(
            "SELECT table_name, column_name FROM information_schema.columns \
             WHERE table_schema = 'public' AND column_name = 'node_id'",
        )
        .fetch_all(&pool)
        .await
        .expect("should query columns");
        assert!(
            node_id_cols.is_empty(),
            "unexpected node_id columns: {node_id_cols:?}"
        );

        // Second run: migrations must be idempotent (sqlx skips already-applied ones).
        migrate(&pool).await.expect("second migration run failed");

        pool.close().await;

        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("failed to drop test database");

        admin.close().await;
    }

    async fn create_empty_db(admin: &PgPool) -> (PgPool, String) {
        let db_name = format!("workflow_db_test_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{db_name}\""
        )))
        .execute(admin)
        .await
        .expect("failed to create test database");
        let options = PgConnectOptions::from_str(&ci_postgres_url(&db_name))
            .expect("invalid test database connection string");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .expect("failed to connect to test database");
        (pool, db_name)
    }

    async fn slot_constraints(pool: &PgPool) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT conname::TEXT FROM pg_constraint WHERE conrelid = 'slot'::regclass \
             AND contype = 'u' ORDER BY conname",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// Migration 007 on a database that already holds worker/slot rows (as
    /// a real one does): nothing is lost, rows are NULL until the backfill,
    /// the backfill fills every NULL once, and the rows then decode.
    #[tokio::test]
    async fn machine_id_migration_and_backfill_keep_existing_rows() {
        let admin = admin_pool().await;
        let (pool, db_name) = create_empty_db(&admin).await;

        sqlx::migrate!()
            .run_to(6, &pool)
            .await
            .expect("migrations 001-006 failed");
        assert_eq!(
            slot_constraints(&pool).await,
            ["slot_project_key_slot_name_key"],
            "001's auto-named constraint, which 007 drops by name"
        );
        // Rows with the pre-007 schema: a running worker owning slot 0, an
        // idle slot 1, and a stopped worker with no slot.
        sqlx::raw_sql(
            "INSERT INTO slot (id, project_key, slot_name, host_path, created_at, updated_at) VALUES
               ('s0', 'proj', '0', '/pool/proj/0', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
               ('s1', 'proj', '1', '/pool/proj/1', '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z');
             INSERT INTO worker (worker_id, process_id, project_key, container_id, worker_secret,
                 strategy, container_status, agent_status, workspace_path, created_at, updated_at,
                 agent_type, brain_path, runtime)
             VALUES
               ('w-run', 'p-run', 'proj', 'c-run', 'sec', 'code', 'running', 'idle',
                '/pool/proj/0', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'codex',
                '/brain', 'local'),
               ('w-gone', 'p-gone', 'proj', 'c-gone', 'sec', 'code', 'stopped', 'idle',
                NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'claude', NULL,
                'container');
             INSERT INTO worker_slot (worker_id, slot_id, created_at)
               VALUES ('w-run', 's0', '2026-01-01T00:00:00Z');",
        )
        .execute(&pool)
        .await
        .expect("seeding pre-007 rows failed");

        migrate(&pool).await.expect("migration 007 failed");
        assert_eq!(
            slot_constraints(&pool).await,
            ["slot_machine_project_slot_name_key"]
        );
        let nulls: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM worker WHERE machine_id IS NULL) \
                  + (SELECT COUNT(*) FROM slot WHERE machine_id IS NULL)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(nulls, 4, "existing rows kept, machine_id unset");
        let dup = sqlx::query(
            "INSERT INTO slot (id, project_key, slot_name, host_path) \
             VALUES ('s0-dup', 'proj', '0', '/x')",
        )
        .execute(&pool)
        .await;
        assert!(
            dup.is_err(),
            "un-backfilled rows keep per-project uniqueness"
        );

        let repo = WorkerRepo::new(pool.clone());
        assert_eq!(repo.backfill_machine_id("mach-a").await.unwrap(), 4);
        assert_eq!(
            repo.backfill_machine_id("mach-b").await.unwrap(),
            0,
            "idempotent"
        );

        let w = repo.get_worker("w-run").await.unwrap().unwrap();
        assert_eq!(w.machine_id, "mach-a");
        assert_eq!(
            (
                w.agent_type.as_str(),
                w.brain_path.as_deref(),
                w.container_status.as_str()
            ),
            ("codex", Some("/brain"), "running")
        );
        assert_eq!(w.runtime, model::WorkerRuntime::Local);
        assert_eq!(repo.list_all_workers().await.unwrap().len(), 2);
        assert_eq!(
            repo.get_worker_slot("w-run")
                .await
                .unwrap()
                .unwrap()
                .slot_id,
            "s0"
        );
        let slots = repo.list_slots("mach-a", "proj").await.unwrap();
        let names: Vec<_> = slots
            .iter()
            .map(|s| (s.id.as_str(), s.machine_id.as_str()))
            .collect();
        assert_eq!(names, [("s0", "mach-a"), ("s1", "mach-a")]);
        let free = repo.find_available_slots("mach-a", "proj").await.unwrap();
        assert_eq!(
            free.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["s1"]
        );

        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("failed to drop test database");
        admin.close().await;
    }
}
