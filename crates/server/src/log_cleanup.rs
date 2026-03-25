use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use tokio::sync::watch;
use tracing::{error, info, warn};

/// Manages periodic cleanup of old worker log directories.
///
/// Worker log directories accumulate under `<logs_dir>/workers/` because each
/// worker gets a unique ID. Standard log rotation does not help because the
/// directories themselves are never removed. This manager scans the workers
/// subdirectory on a configurable interval and deletes directories whose most
/// recent file mtime is older than `max_age`.
#[derive(Clone)]
pub struct LogCleanupManager {
    logs_dir: PathBuf,
    interval: Duration,
    max_age: Duration,
}

impl LogCleanupManager {
    pub fn new(logs_dir: PathBuf, interval: Duration, max_age: Duration) -> Self {
        Self {
            logs_dir,
            interval,
            max_age,
        }
    }

    /// Spawn the periodic log cleanup task.
    ///
    /// Returns a [`tokio::task::JoinHandle`] for the background task.
    /// Send `true` on the shutdown channel to stop it gracefully.
    pub fn spawn(&self, shutdown_rx: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        let workers_dir = self.logs_dir.join("workers");
        let interval = self.interval;
        let max_age = self.max_age;

        info!(
            workers_dir = %workers_dir.display(),
            interval_secs = interval.as_secs(),
            max_age_secs = max_age.as_secs(),
            "log cleanup task starting"
        );

        tokio::spawn(cleanup_loop(workers_dir, interval, max_age, shutdown_rx))
    }
}

/// Run the periodic cleanup loop until shutdown is signaled.
async fn cleanup_loop(
    workers_dir: PathBuf,
    interval: Duration,
    max_age: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                run_cleanup(&workers_dir, max_age);
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("log cleanup task shutting down");
                    return;
                }
            }
        }
    }
}

/// Scan `workers_dir` and remove subdirectories whose newest file is older than `max_age`.
fn run_cleanup(workers_dir: &std::path::Path, max_age: Duration) {
    let entries = match std::fs::read_dir(workers_dir) {
        Ok(e) => e,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                // Workers dir doesn't exist yet — nothing to clean.
                return;
            }
            warn!(error = %e, dir = %workers_dir.display(), "failed to read workers log directory");
            return;
        }
    };

    let now = SystemTime::now();
    let mut removed = 0u64;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let should_remove = match newest_mtime_in_dir(&path) {
            Some(mtime) => now.duration_since(mtime).unwrap_or(Duration::ZERO) > max_age,
            None => true, // Empty directory — treat as stale.
        };

        if should_remove {
            removed += try_remove_dir(&path);
        }
    }

    if removed > 0 {
        info!(removed, "cleaned up old worker log directories");
    }
}

/// Attempt to remove a directory. Returns 1 on success, 0 on failure (logged).
fn try_remove_dir(path: &std::path::Path) -> u64 {
    match std::fs::remove_dir_all(path) {
        Ok(()) => 1,
        Err(e) => {
            error!(error = %e, dir = %path.display(), "failed to remove worker log directory");
            0
        }
    }
}

/// Return the most recent mtime of any file (recursively) inside `dir`.
///
/// Returns `None` if the directory is empty or unreadable.
fn newest_mtime_in_dir(dir: &std::path::Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;

    let walker = match std::fs::read_dir(dir) {
        Ok(w) => w,
        Err(_) => return None,
    };

    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(sub_newest) = newest_mtime_in_dir(&path) {
                newest = Some(match newest {
                    Some(current) => current.max(sub_newest),
                    None => sub_newest,
                });
            }
        } else if let Ok(mtime) = path.metadata().and_then(|m| m.modified()) {
            newest = Some(match newest {
                Some(current) => current.max(mtime),
                None => mtime,
            });
        }
    }

    newest
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_file_with_age(path: &std::path::Path, age: Duration) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "log data").unwrap();
        let mtime = SystemTime::now() - age;
        let times = std::fs::FileTimes::new().set_modified(mtime);
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_times(times).unwrap();
    }

    #[test]
    fn newest_mtime_returns_none_for_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let empty = tmp.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(newest_mtime_in_dir(&empty).is_none());
    }

    #[test]
    fn newest_mtime_finds_most_recent_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("worker-abc");
        let old_age = Duration::from_secs(86400 * 10);
        let new_age = Duration::from_secs(60);

        create_file_with_age(&dir.join("old.log"), old_age);
        create_file_with_age(&dir.join("sub/new.log"), new_age);

        let newest = newest_mtime_in_dir(&dir).unwrap();
        let age = SystemTime::now().duration_since(newest).unwrap();
        // The newest file is ~60s old, so age should be less than 120s
        assert!(age < Duration::from_secs(120), "age was {:?}", age);
    }

    #[test]
    fn run_cleanup_removes_old_directories() {
        let tmp = TempDir::new().unwrap();
        let workers_dir = tmp.path().join("workers");
        fs::create_dir_all(&workers_dir).unwrap();

        let max_age = Duration::from_secs(86400 * 7);

        // Old worker — 10 days old
        let old_worker = workers_dir.join("worker-old");
        create_file_with_age(
            &old_worker.join("workerd.log"),
            Duration::from_secs(86400 * 10),
        );

        // Recent worker — 1 day old
        let recent_worker = workers_dir.join("worker-recent");
        create_file_with_age(
            &recent_worker.join("workerd.log"),
            Duration::from_secs(86400),
        );

        run_cleanup(&workers_dir, max_age);

        assert!(!old_worker.exists(), "old worker dir should be removed");
        assert!(recent_worker.exists(), "recent worker dir should be kept");
    }

    #[test]
    fn run_cleanup_removes_empty_directories() {
        let tmp = TempDir::new().unwrap();
        let workers_dir = tmp.path().join("workers");
        let empty_worker = workers_dir.join("worker-empty");
        fs::create_dir_all(&empty_worker).unwrap();

        let max_age = Duration::from_secs(86400 * 7);
        run_cleanup(&workers_dir, max_age);

        assert!(!empty_worker.exists(), "empty worker dir should be removed");
    }

    #[test]
    fn run_cleanup_ignores_missing_workers_dir() {
        let tmp = TempDir::new().unwrap();
        let workers_dir = tmp.path().join("nonexistent");
        // Should not panic
        run_cleanup(&workers_dir, Duration::from_secs(86400 * 7));
    }

    #[test]
    fn run_cleanup_skips_files_in_workers_dir() {
        let tmp = TempDir::new().unwrap();
        let workers_dir = tmp.path().join("workers");
        fs::create_dir_all(&workers_dir).unwrap();

        // A file directly in workers/ should not be touched
        let stray_file = workers_dir.join("stray.txt");
        fs::write(&stray_file, "data").unwrap();

        run_cleanup(&workers_dir, Duration::from_secs(86400 * 7));

        assert!(
            stray_file.exists(),
            "files in workers dir should not be touched"
        );
    }

    #[tokio::test]
    async fn spawn_and_shutdown() {
        let tmp = TempDir::new().unwrap();
        let logs_dir = tmp.path().to_path_buf();
        let manager = LogCleanupManager::new(
            logs_dir,
            Duration::from_secs(3600),
            Duration::from_secs(86400 * 7),
        );
        let (tx, rx) = watch::channel(false);

        let handle = manager.spawn(rx);
        assert!(!handle.is_finished());

        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("task should finish within timeout")
            .expect("task should not panic");
    }
}
