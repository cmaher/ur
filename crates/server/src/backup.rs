use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{error, info, warn};
use ur_config::BackupConfig;
use ur_db::BackupManager;

/// Manages periodic database backups as a background tokio task.
///
/// Reads backup configuration from `ur.toml`, validates the backup path at
/// startup, and spawns a background task that periodically calls CozoDB's
/// backup API via [`BackupManager`]. The task gracefully stops when the
/// shutdown signal is received.
#[derive(Clone)]
pub struct BackupTaskManager {
    backup_manager: BackupManager,
    config: BackupConfig,
}

impl BackupTaskManager {
    pub fn new(backup_manager: BackupManager, config: BackupConfig) -> Self {
        Self {
            backup_manager,
            config,
        }
    }

    /// Validate that the backup path exists and is writable.
    ///
    /// Returns an error if the path does not exist, is not a directory, or
    /// is not writable. Call this at startup before spawning the background task.
    pub fn validate_backup_path(path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Err(format!(
                "backup path does not exist: {}",
                path.display()
            ));
        }
        if !path.is_dir() {
            return Err(format!(
                "backup path is not a directory: {}",
                path.display()
            ));
        }
        // Check writability by attempting to create and remove a temp file
        let probe = path.join(".ur-backup-probe");
        std::fs::write(&probe, b"probe").map_err(|e| {
            format!(
                "backup path is not writable: {} ({})",
                path.display(),
                e
            )
        })?;
        std::fs::remove_file(&probe).ok();
        Ok(())
    }

    /// Spawn the periodic backup task.
    ///
    /// Returns `None` if backup is disabled (no path configured).
    /// Returns an error if the backup path is invalid.
    ///
    /// The returned [`tokio::task::JoinHandle`] represents the background task.
    /// Send `true` on the `shutdown_tx` channel to stop it gracefully.
    pub fn spawn(
        &self,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<Option<tokio::task::JoinHandle<()>>, String> {
        let backup_path = match &self.config.path {
            Some(p) => p.clone(),
            None => {
                info!("backup disabled (no [backup] path configured)");
                return Ok(None);
            }
        };

        Self::validate_backup_path(&backup_path)?;

        let interval = Duration::from_secs(self.config.interval_minutes * 60);
        let manager = self.backup_manager.clone();

        info!(
            path = %backup_path.display(),
            interval_minutes = self.config.interval_minutes,
            "periodic backup task starting"
        );

        let handle = tokio::spawn(backup_loop(manager, backup_path, interval, shutdown_rx));
        Ok(Some(handle))
    }
}

/// Generate a timestamped backup filename.
fn backup_filename() -> String {
    let now = chrono::Utc::now();
    format!("ur-backup-{}.db", now.format("%Y%m%dT%H%M%SZ"))
}

/// Run the periodic backup loop until shutdown is signaled.
async fn backup_loop(
    manager: BackupManager,
    backup_dir: PathBuf,
    interval: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                run_backup(&manager, &backup_dir);
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("backup task shutting down");
                    return;
                }
            }
        }
    }
}

/// Execute a single backup, rotating the previous file if needed.
fn run_backup(manager: &BackupManager, backup_dir: &Path) {
    let filename = backup_filename();
    let target = backup_dir.join(&filename);

    match manager.backup(&target) {
        Ok(()) => {
            info!(path = %target.display(), "backup completed successfully");
            // Clean up older backups — keep only the latest
            clean_old_backups(backup_dir, &filename);
        }
        Err(e) => {
            error!(error = %e, path = %target.display(), "backup failed");
        }
    }
}

/// Remove backup files in the directory that are older than the current one.
///
/// Only removes files matching the `ur-backup-*.db` naming pattern.
fn clean_old_backups(backup_dir: &Path, current_filename: &str) {
    let entries = match std::fs::read_dir(backup_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "failed to read backup directory for cleanup");
            return;
        }
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("ur-backup-")
            && name_str.ends_with(".db")
            && name_str.as_ref() != current_filename
        {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                warn!(
                    error = %e,
                    file = %entry.path().display(),
                    "failed to remove old backup"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use ur_db::DatabaseManager;

    fn create_test_db() -> (DatabaseManager, BackupManager) {
        let db = DatabaseManager::create_in_memory().expect("create in-memory db");
        // Insert some data so backup is non-trivial
        db.run(
            r#"
            ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
                "ur.001", "epic", "open", 1, "",
                "Test Epic", "For backup testing.",
                "2026-03-12T10:00:00Z", "2026-03-12T10:00:00Z"
            ]]
            :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
        "#,
        )
        .expect("insert test data");
        let bm = BackupManager::new(db.clone());
        (db, bm)
    }

    #[test]
    fn validate_backup_path_rejects_nonexistent() {
        let err = BackupTaskManager::validate_backup_path(Path::new("/nonexistent/path/abc123"))
            .expect_err("should fail");
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn validate_backup_path_rejects_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("not-a-dir");
        std::fs::write(&file, "data").unwrap();
        let err =
            BackupTaskManager::validate_backup_path(&file).expect_err("should fail for file");
        assert!(err.contains("not a directory"), "{err}");
    }

    #[test]
    fn validate_backup_path_accepts_writable_dir() {
        let tmp = TempDir::new().unwrap();
        BackupTaskManager::validate_backup_path(tmp.path()).expect("should succeed");
    }

    #[test]
    fn run_backup_creates_file() {
        let tmp = TempDir::new().unwrap();
        let (_db, bm) = create_test_db();
        run_backup(&bm, tmp.path());

        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("ur-backup-")
            })
            .collect();
        assert_eq!(entries.len(), 1, "should create exactly one backup file");
        assert!(entries[0].metadata().unwrap().len() > 0);
    }

    #[test]
    fn clean_old_backups_removes_previous() {
        let tmp = TempDir::new().unwrap();
        // Create fake old backups
        std::fs::write(tmp.path().join("ur-backup-20260101T000000Z.db"), "old1").unwrap();
        std::fs::write(tmp.path().join("ur-backup-20260102T000000Z.db"), "old2").unwrap();
        // Create a non-backup file that should be preserved
        std::fs::write(tmp.path().join("other.txt"), "keep").unwrap();

        let current = "ur-backup-20260313T120000Z.db";
        std::fs::write(tmp.path().join(current), "current").unwrap();

        clean_old_backups(tmp.path(), current);

        assert!(!tmp.path().join("ur-backup-20260101T000000Z.db").exists());
        assert!(!tmp.path().join("ur-backup-20260102T000000Z.db").exists());
        assert!(tmp.path().join(current).exists());
        assert!(tmp.path().join("other.txt").exists());
    }

    #[test]
    fn spawn_returns_none_when_no_path() {
        let (_db, bm) = create_test_db();
        let config = BackupConfig {
            path: None,
            interval_minutes: 30,
        };
        let mgr = BackupTaskManager::new(bm, config);
        let (_tx, rx) = watch::channel(false);
        let result = mgr.spawn(rx).expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn spawn_errors_on_invalid_path() {
        let (_db, bm) = create_test_db();
        let config = BackupConfig {
            path: Some(PathBuf::from("/nonexistent/backup/path")),
            interval_minutes: 30,
        };
        let mgr = BackupTaskManager::new(bm, config);
        let (_tx, rx) = watch::channel(false);
        let err = mgr.spawn(rx).expect_err("should fail");
        assert!(err.contains("does not exist"), "{err}");
    }

    #[tokio::test]
    async fn spawn_and_shutdown() {
        let tmp = TempDir::new().unwrap();
        let (_db, bm) = create_test_db();
        let config = BackupConfig {
            path: Some(tmp.path().to_path_buf()),
            interval_minutes: 1, // Won't actually tick in this test
        };
        let mgr = BackupTaskManager::new(bm, config);
        let (tx, rx) = watch::channel(false);

        let handle = mgr.spawn(rx).expect("should succeed").expect("should be Some");
        assert!(!handle.is_finished());

        // Signal shutdown
        tx.send(true).unwrap();
        // Wait for task to finish
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("task should finish within timeout")
            .expect("task should not panic");
    }
}
