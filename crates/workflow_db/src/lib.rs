// workflow_db: Postgres-backed workflow database crate.

use sqlx::PgPool;

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

    const CI_POSTGRES_URL: &str = "postgres://ur:ur@localhost:5433/postgres";

    async fn admin_pool() -> PgPool {
        let options = PgConnectOptions::from_str(CI_POSTGRES_URL)
            .expect("invalid ci-postgres connection string");
        PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap_or_else(|_| {
                panic!("Cannot connect to ci-postgres on localhost:5433. Run: cargo make test:init")
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

        let db_url = format!("postgres://ur:ur@localhost:5433/{db_name}");
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
}
