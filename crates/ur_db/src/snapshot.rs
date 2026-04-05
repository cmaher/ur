// SnapshotManager: point-in-time snapshots of ticket state.

use crate::database::DatabaseManager;
use sqlx::PgPool;
use std::fs;
use std::path::Path;

#[derive(Clone)]
pub struct SnapshotManager {
    pool: PgPool,
}

impl SnapshotManager {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a consistent snapshot of the database at the given path using
    /// SQLite's VACUUM INTO. This produces a self-contained copy without
    /// requiring the WAL or shared-memory files.
    pub async fn vacuum_into(&self, path: &str) -> Result<(), sqlx::Error> {
        let path = path.to_owned();
        let pool = self.pool.clone();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "VACUUM INTO '{}'",
            path.replace('\'', "''")
        )))
        .execute(&pool)
        .await?;
        Ok(())
    }

    /// Restore a snapshot into a new database file. Copies the source file to
    /// the target path, then opens it with migrations to verify schema
    /// integrity. Fails if the target path already exists.
    pub async fn restore(
        source_path: &str,
        target_path: &str,
    ) -> Result<DatabaseManager, sqlx::Error> {
        let target = Path::new(target_path);
        if target.exists() {
            return Err(sqlx::Error::Protocol(format!(
                "restore target already exists: {target_path}"
            )));
        }

        let source = Path::new(source_path);
        if !source.exists() {
            return Err(sqlx::Error::Protocol(format!(
                "snapshot source does not exist: {source_path}"
            )));
        }

        fs::copy(source_path, target_path)
            .map_err(|e| sqlx::Error::Protocol(format!("failed to copy snapshot: {e}")))?;

        // Open the restored database and run migrations to verify integrity.
        DatabaseManager::open(target_path).await
    }
}
