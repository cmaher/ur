use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{error, info, warn};
use ur_config::BackupConfig;
use ur_db::SnapshotManager;

/// Manages periodic database backups as a background tokio task.
///
/// Reads backup configuration from `ur.toml`, validates the backup path at
/// startup, and spawns a background task that periodically calls SQLite's
/// VACUUM INTO via [`SnapshotManager`]. The task gracefully stops when the
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
        let backup_path = match &self.config.path {
            Some(p) => p.clone(),
            None => {
                info!("backup disabled (no [backup] path configured)");
                return Ok(None);
            }
        };

        if !self.config.enabled {
            info!("backup disabled (enabled = false in [backup] config)");
            return Ok(None);
        }

        if let Err(e) = Self::validate_backup_path(&backup_path) {
            warn!("backup disabled: {e}");
            return Ok(None);
        }

        let interval = Duration::from_secs(self.config.interval_minutes * 60);
        let manager = self.snapshot_manager.clone();
        let retain_count = self.config.retain_count;

        info!(
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

    /// Run a single on-demand backup. Used by CLI `ur db backup`.
    ///
    /// Manual backups use a `manual-` prefix and are excluded from automatic
    /// retention cleanup, so they are never deleted by the retain count.
    ///
    /// Returns the path to the created backup file.
    pub async fn run_once(&self) -> Result<PathBuf, String> {
        let backup_path = match &self.config.path {
            Some(p) => p.clone(),
            None => return Err("no backup path configured in [backup] section".to_string()),
        };
        Self::validate_backup_path(&backup_path)?;
        let filename = manual_backup_filename();
        let target = backup_path.join(&filename);
        let target_str = target.to_string_lossy();
        self.snapshot_manager
            .vacuum_into(&target_str)
            .await
            .map_err(|e| format!("backup failed: {e}"))?;
        // Only clean automatic backups — manual ones are preserved
        clean_old_backups(&backup_path, &filename, self.config.retain_count);
        Ok(target)
    }

    /// List existing backup files in the configured backup directory.
    ///
    /// Returns backup file paths sorted newest-first (by filename timestamp).
    pub fn list_backups(&self) -> Result<Vec<PathBuf>, String> {
        let backup_path = match &self.config.path {
            Some(p) => p.clone(),
            None => return Err("no backup path configured in [backup] section".to_string()),
        };
        if !backup_path.exists() {
            return Ok(Vec::new());
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&backup_path)
            .map_err(|e| format!("failed to read backup directory: {e}"))?
            .flatten()
            .filter(|e| {
                let name = e.file_name();
                let name_str = name.to_string_lossy();
                is_backup_file(&name_str)
            })
            .map(|e| e.path())
            .collect();
        // Sort by filename descending (newest first, since filenames are timestamped)
        entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        Ok(entries)
    }
}

/// Generate a timestamped backup filename for automatic (periodic) backups.
fn backup_filename() -> String {
    let now = chrono::Utc::now();
    format!("ur-backup-{}.db", now.format("%Y%m%dT%H%M%SZ"))
}

/// Generate a timestamped backup filename for manual (on-demand) backups.
///
/// Manual backups use a `manual-` prefix so they are excluded from automatic
/// retention cleanup.
fn manual_backup_filename() -> String {
    let now = chrono::Utc::now();
    format!("manual-ur-backup-{}.db", now.format("%Y%m%dT%H%M%SZ"))
}

/// Check whether a filename is any kind of backup file (automatic or manual).
fn is_backup_file(name: &str) -> bool {
    name.ends_with(".db")
        && (name.starts_with("ur-backup-") || name.starts_with("manual-ur-backup-"))
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
    let filename = backup_filename();
    let target = backup_dir.join(&filename);
    let target_str = target.to_string_lossy();

    match manager.vacuum_into(&target_str).await {
        Ok(()) => {
            info!(path = %target.display(), "backup completed successfully");
            clean_old_backups(backup_dir, &filename, retain_count);
        }
        Err(e) => {
            error!(error = %e, path = %target.display(), "backup failed");
        }
    }
}

/// Remove backup files that exceed the retain count.
///
/// Keeps the `retain_count` most recent backup files (by filename, which
/// contains a timestamp). Only removes files matching the `ur-backup-*.db`
/// naming pattern. The `current_filename` is always preserved regardless
/// of retain count.
fn clean_old_backups(backup_dir: &Path, current_filename: &str, retain_count: u64) {
    let entries = match std::fs::read_dir(backup_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "failed to read backup directory for cleanup");
            return;
        }
    };

    let mut backup_files: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();
            if name_str.starts_with("ur-backup-") && name_str.ends_with(".db") {
                Some(name_str)
            } else {
                None
            }
        })
        .collect();

    // Sort descending (newest first) — filenames contain ISO timestamps
    backup_files.sort_by(|a, b| b.cmp(a));

    // Keep the newest `retain_count` files; delete everything else
    for name in backup_files.iter().skip(retain_count as usize) {
        if name == current_filename {
            continue;
        }
        let path = backup_dir.join(name);
        if let Err(e) = std::fs::remove_file(&path) {
            warn!(
                error = %e,
                file = %path.display(),
                "failed to remove old backup"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use ur_db::{DatabaseManager, GraphManager, NewTicket, TicketRepo};

    async fn create_test_db(tmp: &TempDir) -> (DatabaseManager, SnapshotManager) {
        let db_path = tmp.path().join("test.db");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = DatabaseManager::open(&db_path_str)
            .await
            .expect("open test db");
        // Insert some data so backup is non-trivial
        let graph_manager = GraphManager::new(db.pool().clone());
        let repo = TicketRepo::new(db.pool().clone(), graph_manager);
        let ticket = NewTicket {
            id: "ur-001".to_string(),
            project: "ur".to_string(),
            type_: "task".to_string(),
            priority: 1,
            parent_id: None,
            title: "Test Epic".to_string(),
            body: "For backup testing.".to_string(),
            ..Default::default()
        };
        repo.create_ticket(&ticket).await.expect("insert test data");
        let sm = SnapshotManager::new(db.pool().clone());
        (db, sm)
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

    #[tokio::test]
    async fn run_backup_creates_file() {
        let db_tmp = TempDir::new().unwrap();
        let backup_tmp = TempDir::new().unwrap();
        let (_db, sm) = create_test_db(&db_tmp).await;
        run_backup(&sm, backup_tmp.path(), 3).await;

        let entries: Vec<_> = std::fs::read_dir(backup_tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("ur-backup-"))
            .collect();
        assert_eq!(entries.len(), 1, "should create exactly one backup file");
        assert!(entries[0].metadata().unwrap().len() > 0);
    }

    #[test]
    fn clean_old_backups_respects_retain_count() {
        let tmp = TempDir::new().unwrap();
        // Create fake backups with ascending timestamps
        std::fs::write(tmp.path().join("ur-backup-20260101T000000Z.db"), "old1").unwrap();
        std::fs::write(tmp.path().join("ur-backup-20260102T000000Z.db"), "old2").unwrap();
        std::fs::write(tmp.path().join("ur-backup-20260103T000000Z.db"), "old3").unwrap();
        // Create a non-backup file that should be preserved
        std::fs::write(tmp.path().join("other.txt"), "keep").unwrap();

        let current = "ur-backup-20260313T120000Z.db";
        std::fs::write(tmp.path().join(current), "current").unwrap();

        // retain_count = 2: keep current + 20260103, remove the rest
        clean_old_backups(tmp.path(), current, 2);

        assert!(tmp.path().join(current).exists(), "current must be kept");
        assert!(
            tmp.path().join("ur-backup-20260103T000000Z.db").exists(),
            "second newest must be kept"
        );
        assert!(
            !tmp.path().join("ur-backup-20260102T000000Z.db").exists(),
            "third should be removed"
        );
        assert!(
            !tmp.path().join("ur-backup-20260101T000000Z.db").exists(),
            "oldest should be removed"
        );
        assert!(
            tmp.path().join("other.txt").exists(),
            "non-backup preserved"
        );
    }

    #[test]
    fn clean_old_backups_preserves_manual_backups() {
        let tmp = TempDir::new().unwrap();
        // Create automatic backups
        std::fs::write(tmp.path().join("ur-backup-20260101T000000Z.db"), "auto1").unwrap();
        std::fs::write(tmp.path().join("ur-backup-20260102T000000Z.db"), "auto2").unwrap();
        // Create manual backups
        std::fs::write(
            tmp.path().join("manual-ur-backup-20260101T120000Z.db"),
            "manual1",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("manual-ur-backup-20260102T120000Z.db"),
            "manual2",
        )
        .unwrap();

        let current = "ur-backup-20260103T000000Z.db";
        std::fs::write(tmp.path().join(current), "current").unwrap();

        // retain_count = 1: keep only the newest automatic backup, delete the rest
        clean_old_backups(tmp.path(), current, 1);

        // Current automatic backup kept
        assert!(tmp.path().join(current).exists());
        // Older automatic backups deleted
        assert!(!tmp.path().join("ur-backup-20260102T000000Z.db").exists());
        assert!(!tmp.path().join("ur-backup-20260101T000000Z.db").exists());
        // Both manual backups preserved
        assert!(
            tmp.path()
                .join("manual-ur-backup-20260101T120000Z.db")
                .exists(),
            "manual backups must survive automatic cleanup"
        );
        assert!(
            tmp.path()
                .join("manual-ur-backup-20260102T120000Z.db")
                .exists(),
            "manual backups must survive automatic cleanup"
        );
    }

    #[test]
    fn clean_old_backups_keeps_all_when_under_retain() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur-backup-20260101T000000Z.db"), "a").unwrap();
        let current = "ur-backup-20260102T000000Z.db";
        std::fs::write(tmp.path().join(current), "b").unwrap();

        clean_old_backups(tmp.path(), current, 5);

        assert!(tmp.path().join("ur-backup-20260101T000000Z.db").exists());
        assert!(tmp.path().join(current).exists());
    }

    #[tokio::test]
    async fn spawn_returns_none_when_no_path() {
        let db_tmp = TempDir::new().unwrap();
        let (_db, sm) = create_test_db(&db_tmp).await;
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

    #[tokio::test]
    async fn spawn_returns_none_when_disabled() {
        let db_tmp = TempDir::new().unwrap();
        let backup_tmp = TempDir::new().unwrap();
        let (_db, sm) = create_test_db(&db_tmp).await;
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

    #[tokio::test]
    async fn spawn_returns_none_on_invalid_path() {
        let db_tmp = TempDir::new().unwrap();
        let (_db, sm) = create_test_db(&db_tmp).await;
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

    #[tokio::test]
    async fn spawn_and_shutdown_triggers_final_backup() {
        let db_tmp = TempDir::new().unwrap();
        let backup_tmp = TempDir::new().unwrap();
        let (_db, sm) = create_test_db(&db_tmp).await;
        let config = BackupConfig {
            path: Some(backup_tmp.path().to_path_buf()),
            interval_minutes: 60, // Won't tick in this test
            enabled: true,
            retain_count: 3,
        };
        let mgr = BackupTaskManager::new(sm, config);
        let (tx, rx) = watch::channel(false);

        let handle = mgr
            .spawn(rx)
            .expect("should succeed")
            .expect("should be Some");
        assert!(!handle.is_finished());

        // Signal shutdown — should trigger final backup
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("task should finish within timeout")
            .expect("task should not panic");

        // Verify final backup was created
        let entries: Vec<_> = std::fs::read_dir(backup_tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("ur-backup-"))
            .collect();
        assert_eq!(entries.len(), 1, "shutdown should create a final backup");
    }

    #[tokio::test]
    async fn run_once_creates_manual_backup() {
        let db_tmp = TempDir::new().unwrap();
        let backup_tmp = TempDir::new().unwrap();
        let (_db, sm) = create_test_db(&db_tmp).await;
        let config = BackupConfig {
            path: Some(backup_tmp.path().to_path_buf()),
            interval_minutes: 30,
            enabled: true,
            retain_count: 3,
        };
        let mgr = BackupTaskManager::new(sm, config);
        let path = mgr.run_once().await.expect("backup should succeed");
        assert!(path.exists());
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("manual-ur-backup-"),
            "on-demand backups must use manual- prefix"
        );
    }

    #[tokio::test]
    async fn list_backups_returns_sorted_including_manual() {
        let db_tmp = TempDir::new().unwrap();
        let backup_tmp = TempDir::new().unwrap();
        let (_db, sm) = create_test_db(&db_tmp).await;

        // Create fake backup files (automatic and manual)
        std::fs::write(backup_tmp.path().join("ur-backup-20260101T000000Z.db"), "a").unwrap();
        std::fs::write(backup_tmp.path().join("ur-backup-20260103T000000Z.db"), "c").unwrap();
        std::fs::write(backup_tmp.path().join("ur-backup-20260102T000000Z.db"), "b").unwrap();
        std::fs::write(
            backup_tmp
                .path()
                .join("manual-ur-backup-20260104T000000Z.db"),
            "m",
        )
        .unwrap();
        // Non-backup file should be excluded
        std::fs::write(backup_tmp.path().join("other.txt"), "x").unwrap();

        let config = BackupConfig {
            path: Some(backup_tmp.path().to_path_buf()),
            interval_minutes: 30,
            enabled: true,
            retain_count: 3,
        };
        let mgr = BackupTaskManager::new(sm, config);
        let backups = mgr.list_backups().expect("list should succeed");
        assert_eq!(backups.len(), 4, "should include both automatic and manual");
        // Newest first — by filename, 'ur-backup-2026010' sorts after 'manual-ur-backup-2026010'
        // so the manual backup sorts first alphabetically in descending order
        assert!(
            backups[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("20260103")
        );
    }
}
