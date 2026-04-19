use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;
use uuid::Uuid;

const CI_POSTGRES_URL: &str = "postgres://ur:ur@localhost:5433/postgres";

pub struct TestDb {
    pool: PgPool,
    db_name: String,
    admin_pool: PgPool,
}

impl TestDb {
    pub async fn new() -> Self {
        let admin_pool = connect_admin_pool().await;
        let db_name = format!("ur_test_{}", Uuid::new_v4().simple());

        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{db_name}\""
        )))
        .execute(&admin_pool)
        .await
        .expect("failed to create test database");

        let db_url = format!("postgres://ur:ur@localhost:5433/{db_name}");
        let options =
            PgConnectOptions::from_str(&db_url).expect("invalid test database connection string");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .expect("failed to connect to test database");

        ticket_db::migrate(&pool)
            .await
            .expect("failed to run ticket_db migrations");
        workflow_db::migrate(&pool)
            .await
            .expect("failed to run workflow_db migrations");

        Self {
            pool,
            db_name,
            admin_pool,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn cleanup(self) {
        self.pool.close().await;

        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
            self.db_name
        )))
        .execute(&self.admin_pool)
        .await
        .expect("failed to drop test database");

        self.admin_pool.close().await;
    }
}

async fn connect_admin_pool() -> PgPool {
    let options =
        PgConnectOptions::from_str(CI_POSTGRES_URL).expect("invalid ci-postgres connection string");

    PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap_or_else(|_| {
            panic!("Cannot connect to ci-postgres on localhost:5433. Run: cargo make test:init")
        })
}
