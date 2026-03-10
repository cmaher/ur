use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_FILE: &str = "lifecycle.log";

/// Append-only lifecycle log for `ur start`/`ur stop` operations.
///
/// Writes timestamped lines to `~/.ur/lifecycle.log`. All errors are silently
/// ignored — logging must never break the primary operation.
pub struct LifecycleLog {
    path: PathBuf,
}

impl LifecycleLog {
    pub fn open(config_dir: &Path) -> Self {
        Self {
            path: config_dir.join(LOG_FILE),
        }
    }

    pub fn info(&self, msg: &str) {
        self.write("INFO", msg);
    }

    pub fn error(&self, msg: &str) {
        self.write("ERROR", msg);
    }

    fn write(&self, level: &str, msg: &str) {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let line = format!("{secs} [{level}] {msg}\n");
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| f.write_all(line.as_bytes()));
    }
}
