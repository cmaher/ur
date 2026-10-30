use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::snapshot::SnapshotManager;
use tokio::sync::watch;
use tracing::{error, info, warn};
use ur_config::BackupConfig;
use ur_config::backup::{BACKUP_TIMESTAMP_FORMAT, automatic_backup_filename, clean_old_backups};

/// Manages periodic database backups as a background tokio task.
///
/// Reads backup configuration from `ur.toml`, validates the backup path at
/// startup, and spawns a background task that periodically calls pg_dump
/// via [`SnapshotManager`]. The task gracefully stops when the
/// shutdown signal is received, performing a final backup before exit.
#[derive(Clone)]
pub struct BackupTaskManager {
    snapshot_manager: SnapshotManager,
    config: BackupConfig,
}

impl BackupTaskManager {
    pub fn new(snapshot_manager: SnapshotManager, config: BackupConfig) -> Self {
        Self {
            snapshot_manager,
            config,
        }
    }

    /// Validate that the backup path exists and is writable.
    ///
    /// Returns an error if the path does not exist, is not a directory, or
    /// is not writable. Call this at startup before spawning the background task.
    pub fn validate_backup_path(path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Err(format!("backup path does not exist: {}", path.display()));
        }
        if !path.is_dir() {
            return Err(format!(
                "backup path is not a directory: {}",
                path.display()
            ));
        }
        // Check writability by attempting to create and remove a temp file
        let probe = path.join(".ur-backup-probe");
        std::fs::write(&probe, b"probe")
            .map_err(|e| format!("backup path is not writable: {} ({})", path.display(), e))?;
        std::fs::remove_file(&probe).ok();
        Ok(())
    }

    /// Spawn the periodic backup task.
    ///
    /// Returns `None` if backup is disabled (no path configured, or `enabled = false`).
    /// Returns an error if the backup path is invalid.
    ///
    /// The returned [`tokio::task::JoinHandle`] represents the background task.
    /// Send `true` on the `shutdown_tx` channel to stop it gracefully (triggers
    /// a final backup before exit).
    pub fn spawn(
        &self,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<Option<tokio::task::JoinHandle<()>>, String> {
        let db = self.snapshot_manager.db_name();
        let backup_path = match &self.config.path {
            Some(p) => p.clone(),
            None => {
                info!(
                    db,
                    "backup disabled (no backup path configured for this database)"
                );
                return Ok(None);
            }
        };

        if !self.config.enabled {
            info!(
                db,
                "backup disabled (enabled = false in this database's backup config)"
            );
            return Ok(None);
        }

        if let Err(e) = Self::validate_backup_path(&backup_path) {
            warn!(db, "backup disabled: {e}");
            return Ok(None);
        }

        let interval = Duration::from_secs(self.config.interval_minutes * 60);
        let manager = self.snapshot_manager.clone();
        let retain_count = self.config.retain_count;

        info!(
            db,
            path = %backup_path.display(),
            interval_minutes = self.config.interval_minutes,
            retain_count = retain_count,
            "periodic backup task starting"
        );

        let handle = tokio::spawn(backup_loop(
            manager,
            backup_path,
            interval,
            retain_count,
            shutdown_rx,
        ));
        Ok(Some(handle))
    }
}

/// Current UTC time formatted for embedding in a backup filename.
fn timestamp_now() -> String {
    chrono::Utc::now()
        .format(BACKUP_TIMESTAMP_FORMAT)
        .to_string()
}

/// Surface retention-cleanup failures in the server log.
fn log_cleanup_failures(failures: Vec<String>) {
    for failure in failures {
        warn!(error = %failure, "backup retention cleanup failed");
    }
}

/// Run the periodic backup loop until shutdown is signaled.
///
/// On shutdown, performs one final backup before returning.
async fn backup_loop(
    manager: SnapshotManager,
    backup_dir: PathBuf,
    interval: Duration,
    retain_count: u64,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                run_backup(&manager, &backup_dir, retain_count).await;
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("backup task shutting down — running final backup");
                    run_backup(&manager, &backup_dir, retain_count).await;
                    return;
                }
            }
        }
    }
}

/// Execute a single backup, cleaning up old files based on retain count.
async fn run_backup(manager: &SnapshotManager, backup_dir: &Path, retain_count: u64) {
    let db_name = manager.db_name();
    let filename = automatic_backup_filename(db_name, &timestamp_now());
    let target = backup_dir.join(&filename);

    match manager.dump_to(&filename).await {
        Ok(()) => {
            info!(path = %target.display(), "backup completed successfully");
            log_cleanup_failures(clean_old_backups(
                backup_dir,
                db_name,
                &filename,
                retain_count,
            ));
        }
        Err(e) => {
            error!(error = %e, path = %target.display(), "backup failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use ur_config::backup::parse_backup_filename;

    fn test_snapshot_manager() -> SnapshotManager {
        SnapshotManager::new(
            "docker".to_string(),
            "ur-postgres".to_string(),
            "ur".to_string(),
        )
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
        let err = BackupTaskManager::validate_backup_path(&file).expect_err("should fail for file");
        assert!(err.contains("not a directory"), "{err}");
    }

    #[test]
    fn validate_backup_path_accepts_writable_dir() {
        let tmp = TempDir::new().unwrap();
        BackupTaskManager::validate_backup_path(tmp.path()).expect("should succeed");
    }

    #[test]
    fn spawn_returns_none_when_no_path() {
        let sm = test_snapshot_manager();
        let config = BackupConfig {
            path: None,
            interval_minutes: 30,
            enabled: true,
            retain_count: 3,
        };
        let mgr = BackupTaskManager::new(sm, config);
        let (_tx, rx) = watch::channel(false);
        let result = mgr.spawn(rx).expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn spawn_returns_none_when_disabled() {
        let backup_tmp = TempDir::new().unwrap();
        let sm = test_snapshot_manager();
        let config = BackupConfig {
            path: Some(backup_tmp.path().to_path_buf()),
            interval_minutes: 30,
            enabled: false,
            retain_count: 3,
        };
        let mgr = BackupTaskManager::new(sm, config);
        let (_tx, rx) = watch::channel(false);
        let result = mgr.spawn(rx).expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn spawn_returns_none_on_invalid_path() {
        let sm = test_snapshot_manager();
        let config = BackupConfig {
            path: Some(PathBuf::from("/nonexistent/backup/path")),
            interval_minutes: 30,
            enabled: true,
            retain_count: 3,
        };
        let mgr = BackupTaskManager::new(sm, config);
        let (_tx, rx) = watch::channel(false);
        // Invalid paths are gracefully handled — spawn returns Ok(None) with a warning
        let result = mgr.spawn(rx).expect("should not error");
        assert!(result.is_none(), "invalid path should disable backup");
    }

    #[test]
    fn timestamp_now_matches_filename_format() {
        let name = automatic_backup_filename("ur_tickets", &timestamp_now());
        assert_eq!(
            parse_backup_filename(&name).and_then(|p| p.db_name),
            Some("ur_tickets")
        );
    }
}
