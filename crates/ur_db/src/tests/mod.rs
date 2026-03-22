mod database_tests;
mod graph_tests;
mod snapshot_tests;
mod ticket_repo_tests;
mod worker_repo_tests;
mod workflow_repo_tests;

use crate::database::DatabaseManager;
use std::path::PathBuf;

pub struct TestDb {
    db: DatabaseManager,
    path: PathBuf,
}

impl TestDb {
    pub async fn new() -> Self {
        let file_name = format!("ur_test_{}.db", uuid::Uuid::new_v4());
        let path = std::env::temp_dir().join(file_name);
        let db = DatabaseManager::open(path.to_str().expect("temp path is valid UTF-8"))
            .await
            .expect("failed to open test database");
        Self { db, path }
    }

    pub fn db(&self) -> &DatabaseManager {
        &self.db
    }

    pub async fn cleanup(self) {
        self.db.pool().close().await;
        std::fs::remove_file(&self.path).ok();
    }
}
