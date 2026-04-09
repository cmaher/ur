use std::path::{Path, PathBuf};

use chrono::Utc;
use tracing::warn;

/// Container-side mount point for per-worker logs.
const CONTAINER_LOGS_DIR: &str = "/var/ur/logs";

/// Write a hook failure log file to the worker's log directory on the host
/// filesystem and return the container-side path for use in activity messages.
///
/// The log file is written to `<logs_dir>/workers/<worker_id>/<hook>-<timestamp>.log`
/// on the host. The returned path is the container-side equivalent:
/// `/var/ur/logs/<hook>-<timestamp>.log`.
///
/// Write errors are logged as warnings but never propagated — a failed log
/// write must not stall the workflow.
pub fn write_hook_failure_log(
    logs_dir: &Path,
    worker_id: &str,
    hook: &str,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> String {
    let now = Utc::now();
    let timestamp = now.format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let filename = format!("{hook}-{timestamp}.log");

    let host_path = logs_dir.join("workers").join(worker_id).join(&filename);

    let content = format_log_content(hook, exit_code, &now, stdout, stderr);

    if let Err(e) = write_log_file(&host_path, &content) {
        warn!(
            path = %host_path.display(),
            error = %e,
            "failed to write hook failure log"
        );
    }

    format!("{CONTAINER_LOGS_DIR}/{filename}")
}

/// Format the log file content with header fields and separated stdout/stderr.
fn format_log_content(
    hook: &str,
    exit_code: i32,
    timestamp: &chrono::DateTime<Utc>,
    stdout: &str,
    stderr: &str,
) -> String {
    format!(
        "exit_code: {exit_code}\n\
         timestamp: {}\n\
         hook: {hook}\n\
         ---\n\
         {stdout}\n\
         ---\n\
         {stderr}",
        timestamp.format("%Y-%m-%dT%H:%M:%SZ"),
    )
}

/// Write content to a file, creating parent directories as needed.
fn write_log_file(path: &PathBuf, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_hook_failure_log_creates_file_and_returns_container_path() {
        let tmp = tempfile::tempdir().unwrap();
        let logs_dir = tmp.path();
        std::fs::create_dir_all(logs_dir.join("workers").join("w-1234")).unwrap();

        let container_path =
            write_hook_failure_log(logs_dir, "w-1234", "verify", "ok stdout", "err line", 1);

        assert!(container_path.starts_with("/var/ur/logs/verify-"));
        assert!(container_path.ends_with(".log"));
        // No colons in filename (filesystem-safe timestamp).
        let filename = container_path.strip_prefix("/var/ur/logs/").unwrap();
        assert!(
            !filename.contains(':'),
            "filename must not contain colons: {filename}"
        );

        // Verify file was written on disk.
        let entries: Vec<_> = std::fs::read_dir(logs_dir.join("workers").join("w-1234"))
            .unwrap()
            .collect();
        assert_eq!(entries.len(), 1);

        let content = std::fs::read_to_string(entries[0].as_ref().unwrap().path()).unwrap();
        assert!(content.starts_with("exit_code: 1"));
        assert!(content.contains("hook: verify"));
        assert!(content.contains("ok stdout"));
        assert!(content.contains("err line"));
    }

    #[test]
    fn write_hook_failure_log_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let logs_dir = tmp.path();
        // Don't pre-create the workers dir — the helper should handle it.

        let container_path = write_hook_failure_log(logs_dir, "w-new", "push", "out", "err", 2);

        assert!(container_path.starts_with("/var/ur/logs/push-"));

        let worker_dir = logs_dir.join("workers").join("w-new");
        assert!(worker_dir.exists());
    }

    #[test]
    fn write_hook_failure_log_handles_missing_base_dir_gracefully() {
        // Point to a path that cannot be created (nested under a file).
        let tmp = tempfile::tempdir().unwrap();
        let fake_file = tmp.path().join("not-a-dir");
        std::fs::write(&fake_file, "block").unwrap();

        // Should not panic — just warn and return a path.
        let container_path = write_hook_failure_log(&fake_file, "w-x", "verify", "", "", 1);

        assert!(container_path.starts_with("/var/ur/logs/verify-"));
    }

    #[test]
    fn format_log_content_matches_spec() {
        let ts = chrono::DateTime::parse_from_rfc3339("2026-04-09T15:07:02Z")
            .unwrap()
            .with_timezone(&Utc);
        let content = format_log_content("verify", 1, &ts, "hello stdout", "hello stderr");
        let expected = "\
exit_code: 1
timestamp: 2026-04-09T15:07:02Z
hook: verify
---
hello stdout
---
hello stderr";
        assert_eq!(content, expected);
    }
}
