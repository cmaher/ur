use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;
use ur_db::DatabaseManager;
use uuid::Uuid;

const CI_POSTGRES_URL: &str = "postgres://ur:ur@localhost:5433/postgres";

pub struct TestDb {
    db: DatabaseManager,
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
        let db = DatabaseManager::open(&db_url)
            .await
            .expect("failed to open test database");

        Self {
            db,
            db_name,
            admin_pool,
        }
    }

    pub fn db(&self) -> &DatabaseManager {
        &self.db
    }

    pub async fn cleanup(self) {
        self.db.pool().close().await;

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
