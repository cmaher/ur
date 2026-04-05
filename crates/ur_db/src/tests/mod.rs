mod database_tests;
mod graph_tests;
mod snapshot_tests;
mod ticket_repo_tests;
mod ui_event_tests;
mod worker_repo_tests;
mod workflow_repo_tests;

use crate::database::DatabaseManager;

pub struct TestDb {
    db: DatabaseManager,
}

impl TestDb {
    pub async fn new() -> Self {
        let db_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://ur:ur@localhost:5432/ur_test".to_string());
        let db = DatabaseManager::open(&db_url)
            .await
            .expect("failed to open test database");
        Self { db }
    }

    pub fn db(&self) -> &DatabaseManager {
        &self.db
    }

    pub async fn cleanup(self) {
        self.db.pool().close().await;
    }
}
