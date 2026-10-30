//! Backup file naming and container layout shared by the server's periodic
//! backups and the CLI's `ur db` commands.
//!
//! Each database dumps into its own container directory
//! (`/backup/<db_name>`), and every filename carries the database name so
//! dumps of different databases stay distinguishable even when their host
//! directories are the same:
//!
//! - automatic: `ur-backup-<db_name>-<timestamp>.pgdump`
//! - manual:    `manual-ur-backup-<db_name>-<timestamp>.pgdump`
//!
//! Files written before the database name was added (`ur-backup-<timestamp>.pgdump`)
//! are still recognized as backups, but carry no database name and are never
//! touched by retention cleanup.

use std::path::Path;

use crate::BACKUP_CONTAINER_PATH;

/// Filename prefix for automatic (periodic) backups.
pub const AUTOMATIC_BACKUP_PREFIX: &str = "ur-backup-";

/// Filename prefix for manual (on-demand) backups. Manual backups are excluded
/// from retention cleanup.
pub const MANUAL_BACKUP_PREFIX: &str = "manual-ur-backup-";

/// File extension for pg_dump custom-format backups.
pub const BACKUP_EXTENSION: &str = ".pgdump";

/// `chrono` format string for the timestamp embedded in backup filenames.
pub const BACKUP_TIMESTAMP_FORMAT: &str = "%Y%m%dT%H%M%SZ";

/// A backup filename split into its components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupFileName<'a> {
    /// Database the dump belongs to. `None` for legacy filenames that predate
    /// per-database naming.
    pub db_name: Option<&'a str>,
    /// Timestamp portion (formatted with [`BACKUP_TIMESTAMP_FORMAT`]).
    pub timestamp: &'a str,
    /// Whether this is a manual (on-demand) backup.
    pub manual: bool,
}

/// Container-side directory a database's backups are written to.
pub fn backup_container_dir(db_name: &str) -> String {
    format!("{BACKUP_CONTAINER_PATH}/{db_name}")
}

/// Filename for an automatic backup of `db_name` taken at `timestamp`.
pub fn automatic_backup_filename(db_name: &str, timestamp: &str) -> String {
    format!("{AUTOMATIC_BACKUP_PREFIX}{db_name}-{timestamp}{BACKUP_EXTENSION}")
}

/// Filename for a manual backup of `db_name` taken at `timestamp`.
pub fn manual_backup_filename(db_name: &str, timestamp: &str) -> String {
    format!("{MANUAL_BACKUP_PREFIX}{db_name}-{timestamp}{BACKUP_EXTENSION}")
}

/// Parse a backup filename. Returns `None` if `name` is not a backup file.
pub fn parse_backup_filename(name: &str) -> Option<BackupFileName<'_>> {
    let stem = name.strip_suffix(BACKUP_EXTENSION)?;
    let (rest, manual) = match stem.strip_prefix(MANUAL_BACKUP_PREFIX) {
        Some(rest) => (rest, true),
        None => (stem.strip_prefix(AUTOMATIC_BACKUP_PREFIX)?, false),
    };
    if let Some((db_name, timestamp)) = rest.rsplit_once('-')
        && !db_name.is_empty()
        && is_backup_timestamp(timestamp)
    {
        return Some(BackupFileName {
            db_name: Some(db_name),
            timestamp,
            manual,
        });
    }
    if is_backup_timestamp(rest) {
        return Some(BackupFileName {
            db_name: None,
            timestamp: rest,
            manual,
        });
    }
    None
}

/// Check whether a filename is any kind of backup file (automatic or manual,
/// current or legacy naming).
pub fn is_backup_file(name: &str) -> bool {
    parse_backup_filename(name).is_some()
}

/// Remove automatic backups of `db_name` in `backup_dir` that exceed `retain_count`.
///
/// Keeps the `retain_count` most recent automatic backups of `db_name` (by
/// filename, which contains a timestamp). Manual backups, legacy unnamed
/// backups, and other databases' backups are never removed. The
/// `current_filename` is always preserved regardless of retain count.
///
/// Returns a description of every failure; the caller decides how to surface them.
pub fn clean_old_backups(
    backup_dir: &Path,
    db_name: &str,
    current_filename: &str,
    retain_count: u64,
) -> Vec<String> {
    let entries = match std::fs::read_dir(backup_dir) {
        Ok(e) => e,
        Err(e) => {
            return vec![format!(
                "failed to read backup directory {}: {e}",
                backup_dir.display()
            )];
        }
    };

    let mut backup_files: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let parsed = parse_backup_filename(&name)?;
            (!parsed.manual && parsed.db_name == Some(db_name)).then_some(name)
        })
        .collect();

    // Sort descending (newest first) — filenames contain ISO timestamps
    backup_files.sort_by(|a, b| b.cmp(a));

    let mut failures = Vec::new();
    for name in backup_files.iter().skip(retain_count as usize) {
        if name == current_filename {
            continue;
        }
        let path = backup_dir.join(name);
        if let Err(e) = std::fs::remove_file(&path) {
            failures.push(format!(
                "failed to remove old backup {}: {e}",
                path.display()
            ));
        }
    }
    failures
}

/// Whether `s` has the shape produced by [`BACKUP_TIMESTAMP_FORMAT`]
/// (`YYYYMMDDTHHMMSSZ`).
fn is_backup_timestamp(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 16
        && bytes[8] == b'T'
        && bytes[15] == b'Z'
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[9..15].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const TS: &str = "20260101T000000Z";

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), name).unwrap();
    }

    #[test]
    fn filenames_include_db_name() {
        assert_eq!(
            automatic_backup_filename("ur_tickets", TS),
            "ur-backup-ur_tickets-20260101T000000Z.pgdump"
        );
        assert_eq!(
            manual_backup_filename("ur_workflow", TS),
            "manual-ur-backup-ur_workflow-20260101T000000Z.pgdump"
        );
    }

    #[test]
    fn filenames_for_different_dbs_never_collide() {
        assert_ne!(
            automatic_backup_filename("ur_tickets", TS),
            automatic_backup_filename("ur_workflow", TS)
        );
    }

    #[test]
    fn container_dir_is_per_db() {
        assert_eq!(backup_container_dir("ur_tickets"), "/backup/ur_tickets");
    }

    #[test]
    fn parse_round_trips_new_names() {
        let auto = automatic_backup_filename("ur_tickets", TS);
        assert_eq!(
            parse_backup_filename(&auto),
            Some(BackupFileName {
                db_name: Some("ur_tickets"),
                timestamp: TS,
                manual: false,
            })
        );
        let manual = manual_backup_filename("my-db", TS);
        assert_eq!(
            parse_backup_filename(&manual),
            Some(BackupFileName {
                db_name: Some("my-db"),
                timestamp: TS,
                manual: true,
            })
        );
    }

    #[test]
    fn parse_accepts_legacy_names() {
        assert_eq!(
            parse_backup_filename("ur-backup-20260101T000000Z.pgdump"),
            Some(BackupFileName {
                db_name: None,
                timestamp: TS,
                manual: false,
            })
        );
        assert_eq!(
            parse_backup_filename("manual-ur-backup-20260101T000000Z.pgdump"),
            Some(BackupFileName {
                db_name: None,
                timestamp: TS,
                manual: true,
            })
        );
    }

    #[test]
    fn parse_rejects_non_backups() {
        assert!(!is_backup_file("ur-backup-20260101T000000Z.db"));
        assert!(!is_backup_file("other.pgdump"));
        assert!(!is_backup_file("other.txt"));
        assert!(!is_backup_file("ur-backup-.pgdump"));
        assert!(!is_backup_file("ur-backup-ur_tickets-notatimestamp.pgdump"));
    }

    #[test]
    fn clean_respects_retain_count_per_db() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        for day in 1..=3 {
            let ts = format!("2026010{day}T000000Z");
            touch(dir, &automatic_backup_filename("ur_tickets", &ts));
            touch(dir, &automatic_backup_filename("ur_workflow", &ts));
        }
        touch(dir, "other.txt");
        let current = automatic_backup_filename("ur_tickets", "20260313T120000Z");
        touch(dir, &current);

        let failures = clean_old_backups(dir, "ur_tickets", &current, 2);
        assert!(failures.is_empty(), "{failures:?}");

        assert!(dir.join(&current).exists(), "current must be kept");
        assert!(
            dir.join(automatic_backup_filename("ur_tickets", "20260103T000000Z"))
                .exists(),
            "second newest must be kept"
        );
        assert!(
            !dir.join(automatic_backup_filename("ur_tickets", "20260102T000000Z"))
                .exists()
        );
        assert!(
            !dir.join(automatic_backup_filename("ur_tickets", "20260101T000000Z"))
                .exists()
        );
        // The other database's dumps are a separate retention budget.
        for day in 1..=3 {
            let ts = format!("2026010{day}T000000Z");
            assert!(
                dir.join(automatic_backup_filename("ur_workflow", &ts))
                    .exists(),
                "workflow dump {ts} must survive ticket cleanup"
            );
        }
        assert!(dir.join("other.txt").exists(), "non-backup preserved");
    }

    #[test]
    fn clean_preserves_manual_and_legacy_backups() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        touch(dir, &automatic_backup_filename("ur_tickets", TS));
        touch(dir, &manual_backup_filename("ur_tickets", TS));
        touch(dir, "ur-backup-20260101T000000Z.pgdump");
        touch(dir, "manual-ur-backup-20260101T000000Z.pgdump");
        let current = automatic_backup_filename("ur_tickets", "20260103T000000Z");
        touch(dir, &current);

        let failures = clean_old_backups(dir, "ur_tickets", &current, 1);
        assert!(failures.is_empty(), "{failures:?}");

        assert!(dir.join(&current).exists());
        assert!(
            !dir.join(automatic_backup_filename("ur_tickets", TS))
                .exists()
        );
        assert!(dir.join(manual_backup_filename("ur_tickets", TS)).exists());
        assert!(dir.join("ur-backup-20260101T000000Z.pgdump").exists());
        assert!(
            dir.join("manual-ur-backup-20260101T000000Z.pgdump")
                .exists()
        );
    }

    #[test]
    fn clean_keeps_all_when_under_retain() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let old = automatic_backup_filename("ur_tickets", TS);
        touch(dir, &old);
        let current = automatic_backup_filename("ur_tickets", "20260102T000000Z");
        touch(dir, &current);

        clean_old_backups(dir, "ur_tickets", &current, 5);

        assert!(dir.join(&old).exists());
        assert!(dir.join(&current).exists());
    }

    #[test]
    fn clean_reports_unreadable_dir() {
        let failures = clean_old_backups(
            Path::new("/nonexistent/backup/dir"),
            "ur_tickets",
            "x.pgdump",
            1,
        );
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("failed to read backup directory"));
    }
}
