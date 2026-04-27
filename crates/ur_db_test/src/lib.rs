use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;
use uuid::Uuid;

const CI_POSTGRES_URL: &str = "postgres://ur:ur@localhost:5433/postgres";

pub struct TestDb {
    ticket_pool: PgPool,
    workflow_pool: PgPool,
    ticket_db_name: String,
    workflow_db_name: String,
}

impl TestDb {
    pub async fn new() -> Self {
        let admin_pool = connect_admin_pool().await;
        let suffix = Uuid::new_v4().simple().to_string();
        let ticket_db_name = format!("ur_test_tickets_{suffix}");
        let workflow_db_name = format!("ur_test_workflow_{suffix}");

        for db_name in [&ticket_db_name, &workflow_db_name] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "CREATE DATABASE \"{db_name}\""
            )))
            .execute(&admin_pool)
            .await
            .unwrap_or_else(|e| panic!("failed to create test database {db_name}: {e}"));
        }

        let ticket_url = format!("postgres://ur:ur@localhost:5433/{ticket_db_name}");
        let workflow_url = format!("postgres://ur:ur@localhost:5433/{workflow_db_name}");

        let ticket_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(
                PgConnectOptions::from_str(&ticket_url)
                    .expect("invalid ticket db connection string"),
            )
            .await
            .expect("failed to connect to ticket test database");

        let workflow_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(
                PgConnectOptions::from_str(&workflow_url)
                    .expect("invalid workflow db connection string"),
            )
            .await
            .expect("failed to connect to workflow test database");

        ticket_db::migrate(&ticket_pool)
            .await
            .expect("failed to run ticket_db migrations");
        workflow_db::migrate(&workflow_pool)
            .await
            .expect("failed to run workflow_db migrations");

        Self {
            ticket_pool,
            workflow_pool,
            ticket_db_name,
            workflow_db_name,
        }
    }

    pub fn ticket_pool(&self) -> &PgPool {
        &self.ticket_pool
    }

    pub fn workflow_pool(&self) -> &PgPool {
        &self.workflow_pool
    }
}

/// Drop databases on scope exit so panicking tests don't leak.
///
/// Spawns a fresh OS thread with its own current-thread tokio runtime because
/// `Drop` is sync and the calling runtime may already be unwinding. `WITH
/// (FORCE)` evicts any still-open connections from the test pools.
impl Drop for TestDb {
    fn drop(&mut self) {
        let ticket_db_name = std::mem::take(&mut self.ticket_db_name);
        let workflow_db_name = std::mem::take(&mut self.workflow_db_name);
        if ticket_db_name.is_empty() && workflow_db_name.is_empty() {
            return;
        }

        let join = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build TestDb cleanup runtime");
            rt.block_on(async {
                let admin = match PgPoolOptions::new()
                    .max_connections(1)
                    .connect(CI_POSTGRES_URL)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("TestDb cleanup: failed to connect to ci-postgres: {e}");
                        return;
                    }
                };
                for db_name in [&ticket_db_name, &workflow_db_name] {
                    if let Err(e) = sqlx::query(sqlx::AssertSqlSafe(format!(
                        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
                    )))
                    .execute(&admin)
                    .await
                    {
                        eprintln!("TestDb cleanup: failed to drop {db_name}: {e}");
                    }
                }
                admin.close().await;
            });
        });
        let _ = join.join();
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
