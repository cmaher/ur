// SnapshotManager: pg_dump/pg_restore via docker exec into the postgres container.

use std::process::Stdio;
use tokio::process::Command;

#[derive(Clone)]
pub struct SnapshotManager {
    container_command: String,
    container_name: String,
    db_name: String,
}

impl SnapshotManager {
    pub fn new(container_command: String, container_name: String, db_name: String) -> Self {
        Self {
            container_command,
            container_name,
            db_name,
        }
    }

    /// Create a pg_dump backup inside the container at /backup/<filename>.
    ///
    /// Runs: docker exec <container> pg_dump -Fc -f /backup/<filename> <dbname>
    pub async fn dump_to(&self, filename: &str) -> Result<(), String> {
        let backup_path = format!("/backup/{filename}");
        let output = Command::new(&self.container_command)
            .args([
                "exec",
                &self.container_name,
                "pg_dump",
                "-Fc",
                "-f",
                &backup_path,
                &self.db_name,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("failed to run {}: {e}", self.container_command))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("pg_dump failed: {stderr}"));
        }
        Ok(())
    }

    /// Restore a pg_dump backup from /backup/<filename> into the live database.
    ///
    /// Runs: docker exec <container> pg_restore --clean --if-exists -d <dbname> /backup/<filename>
    pub async fn restore_from(&self, filename: &str) -> Result<(), String> {
        let backup_path = format!("/backup/{filename}");
        let output = Command::new(&self.container_command)
            .args([
                "exec",
                &self.container_name,
                "pg_restore",
                "--clean",
                "--if-exists",
                "-d",
                &self.db_name,
                &backup_path,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("failed to run {}: {e}", self.container_command))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("pg_restore failed: {stderr}"));
        }
        Ok(())
    }
}
