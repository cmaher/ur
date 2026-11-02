use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::snapshot::SnapshotManager;
use tokio::sync::watch;
use tracing::{error, info, warn};
use ur_config::BackupConfig;
use ur_config::backup::{
    BACKUP_TIMESTAMP_FORMAT, automatic_backup_filename, clean_old_backups, validate_backup_dir,
};

/// Manages periodic database backups as a background tokio task.
///
/// Reads backup configuration from `ur.toml` and spawns a background task
/// that periodically calls pg_dump via [`SnapshotManager`]. The task
/// gracefully stops when the shutdown signal is received, performing a final
/// backup before exit.
///
/// `[<db>.backup] path` is a *host* path: it is bind-mounted at
/// `<backup_root>/<db_name>` (`/backup/<db_name>`) into both `ur-postgres`
/// (where pg_dump writes) and `ur-server` (where this manager validates the
/// directory and applies retention). The server never touches the host path
/// directly — it is not visible inside the server container.
#[derive(Clone)]
pub struct BackupTaskManager {
    snapshot_manager: SnapshotManager,
    config: BackupConfig,
    backup_root: PathBuf,
}

impl BackupTaskManager {
    /// `backup_root` is the server-side directory under which each database's
    /// backup dir is mounted — [`ur_config::BACKUP_CONTAINER_PATH`] in production.
    pub fn new(
        snapshot_manager: SnapshotManager,
        config: BackupConfig,
        backup_root: PathBuf,
    ) -> Self {
        Self {
            snapshot_manager,
            config,
            backup_root,
        }
    }

    /// Server-side directory holding this database's backups.
    fn backup_dir(&self) -> PathBuf {
        self.backup_root.join(self.snapshot_manager.db_name())
    }

    /// Spawn the periodic backup task.
    ///
    /// Returns `None` if backup is disabled (no path configured, or `enabled = false`).
    /// Returns an error if the mounted backup directory is missing, not a
    /// directory, or not writable — a misconfigured backup must stop startup,
    /// not silently disable backups.
    ///
    /// The returned [`tokio::task::JoinHandle`] represents the background task.
    /// Send `true` on the `shutdown_tx` channel to stop it gracefully (triggers
    /// a final backup before exit).
    pub fn spawn(
        &self,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<Option<tokio::task::JoinHandle<()>>, String> {
        let db = self.snapshot_manager.db_name();
        let Some(host_path) = &self.config.path else {
            info!(
                db,
                "backup disabled (no backup path configured for this database)"
            );
            return Ok(None);
        };

        if !self.config.enabled {
            info!(
                db,
                "backup disabled (enabled = false in this database's backup config)"
            );
            return Ok(None);
        }

        let backup_dir = self.backup_dir();
        validate_backup_dir(&backup_dir).map_err(|e| {
            format!(
                "{e} (mount of host backup path {} into the server container)",
                host_path.display()
            )
        })?;

        let interval = Duration::from_secs(self.config.interval_minutes * 60);
        let manager = self.snapshot_manager.clone();
        let retain_count = self.config.retain_count;

        info!(
            db,
            host_path = %host_path.display(),
            path = %backup_dir.display(),
            interval_minutes = self.config.interval_minutes,
            retain_count = retain_count,
            "periodic backup task starting"
        );

        let handle = tokio::spawn(backup_loop(
            manager,
            backup_dir,
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
    fn spawn_returns_none_when_no_path() {
        let sm = test_snapshot_manager();
        let config = BackupConfig {
            path: None,
            interval_minutes: 30,
            enabled: true,
            retain_count: 3,
        };
        let mgr = BackupTaskManager::new(sm, config, PathBuf::from("/nonexistent/backup/root"));
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
        let mgr = BackupTaskManager::new(sm, config, PathBuf::from("/nonexistent/backup/root"));
        let (_tx, rx) = watch::channel(false);
        let result = mgr.spawn(rx).expect("should not error");
        assert!(result.is_none());
    }

    fn enabled_config(host_path: &Path) -> BackupConfig {
        BackupConfig {
            path: Some(host_path.to_path_buf()),
            interval_minutes: 30,
            enabled: true,
            retain_count: 3,
        }
    }

    #[test]
    fn spawn_errors_when_container_dir_missing_even_if_host_path_exists() {
        // The host path is visible here but the server-side mount is not:
        // the old code validated the host path and would have passed (or, in
        // the real container, silently disabled backups).
        let host = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let mgr = BackupTaskManager::new(
            test_snapshot_manager(),
            enabled_config(host.path()),
            root.path().to_path_buf(),
        );
        let (_tx, rx) = watch::channel(false);
        let err = mgr.spawn(rx).expect_err("missing mount must be an error");
        assert!(err.contains("does not exist"), "{err}");
        assert!(
            err.contains(&root.path().join("ur").display().to_string()),
            "{err}"
        );
        assert!(err.contains(&host.path().display().to_string()), "{err}");
    }

    #[tokio::test]
    async fn spawn_validates_container_dir_not_host_path() {
        let root = TempDir::new().unwrap();
        std::fs::create_dir(root.path().join("ur")).unwrap();
        let mgr = BackupTaskManager::new(
            test_snapshot_manager(),
            enabled_config(Path::new("/host/only/path/not/visible/here")),
            root.path().to_path_buf(),
        );
        let (_tx, rx) = watch::channel(false);
        let handle = mgr
            .spawn(rx)
            .expect("mounted dir is valid")
            .expect("backup task should start");
        handle.abort();
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
