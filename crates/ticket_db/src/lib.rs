// ticket_db: Postgres-backed ticket database crate.

pub mod graph;
pub mod model;
pub mod ticket_repo;

pub use graph::GraphManager;
pub use model::{
    Activity, DispatchableTicket, Edge, EdgeKind, LifecycleStatus, MetadataMatchTicket, NewTicket,
    Ticket, TicketComment, TicketFilter, TicketStatus, TicketType, TicketUpdate, UiEventRow,
};
pub use ticket_repo::TicketRepo;

use sqlx::PgPool;

/// Run all pending ticket_db migrations against the given pool.
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
            .expect("Cannot connect to ci-postgres on localhost:5433. Run: cargo make test:init")
    }

    async fn create_test_pool(admin: &PgPool, db_name: &str) -> PgPool {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{db_name}\""
        )))
        .execute(admin)
        .await
        .expect("failed to create test database");

        let url = format!("postgres://ur:ur@localhost:5433/{db_name}");
        let options =
            PgConnectOptions::from_str(&url).expect("invalid test database connection string");
        PgPoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .expect("failed to connect to test database")
    }

    async fn drop_test_db(admin: &PgPool, db_name: &str) {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        )))
        .execute(admin)
        .await
        .expect("failed to drop test database");
    }

    #[tokio::test]
    async fn migration_applies_all_tables() {
        let admin = admin_pool().await;
        let db_name = format!("ticket_db_test_{}", Uuid::new_v4().simple());
        let pool = create_test_pool(&admin, &db_name).await;

        migrate(&pool).await.expect("migration should succeed");

        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name NOT LIKE '_sqlx%' \
             ORDER BY table_name",
        )
        .fetch_all(&pool)
        .await
        .expect("should query tables");

        let names: Vec<&str> = tables.iter().map(|t| t.0.as_str()).collect();
        assert!(names.contains(&"ticket"), "missing ticket table");
        assert!(names.contains(&"edge"), "missing edge table");
        assert!(names.contains(&"meta"), "missing meta table");
        assert!(names.contains(&"activity"), "missing activity table");
        assert!(
            names.contains(&"ticket_comments"),
            "missing ticket_comments table"
        );
        assert!(names.contains(&"ui_events"), "missing ui_events table");

        pool.close().await;
        drop_test_db(&admin, &db_name).await;
        admin.close().await;
    }

    #[tokio::test]
    async fn migration_is_idempotent() {
        let admin = admin_pool().await;
        let db_name = format!("ticket_db_idem_{}", Uuid::new_v4().simple());
        let pool = create_test_pool(&admin, &db_name).await;

        // Apply migrations twice — second run must succeed (sqlx skips applied migrations).
        migrate(&pool)
            .await
            .expect("first migration should succeed");
        migrate(&pool)
            .await
            .expect("second migration should be idempotent");

        pool.close().await;
        drop_test_db(&admin, &db_name).await;
        admin.close().await;
    }
}
