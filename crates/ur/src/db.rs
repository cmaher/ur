use std::path::Path;

use anyhow::{Context, Result, bail};
use tracing::{debug, info};

use crate::output::{BackupCreated, BackupEntry, BackupList, OutputManager};

/// Run an on-demand database backup using VACUUM INTO.
///
/// Connects directly to the SQLite database file and creates a consistent
/// snapshot in the configured backup directory.
pub async fn backup(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    let backup_path = config
        .backup
        .path
        .as_ref()
        .context("no backup path configured — set [backup] path in ur.toml")?;

    if !backup_path.exists() {
        bail!(
            "backup directory does not exist: {} — run 'ur init' first",
            backup_path.display()
        );
    }

    let db_path = config.config_dir.join("ur.db");
    if !db_path.exists() {
        bail!("database not found at {}", db_path.display());
    }

    info!(db = %db_path.display(), backup_dir = %backup_path.display(), "creating on-demand backup");

    let db_path_str = db_path.to_string_lossy().to_string();
    let db = ur_db::DatabaseManager::open(&db_path_str)
        .await
        .context("failed to open database")?;

    let snapshot_manager = ur_db::SnapshotManager::new(db.pool().clone());
    let filename = backup_filename();
    let target = backup_path.join(&filename);
    let target_str = target.to_string_lossy();

    snapshot_manager
        .vacuum_into(&target_str)
        .await
        .context("backup failed")?;

    clean_old_backups(backup_path, &filename, config.backup.retain_count);

    info!(path = %target.display(), "backup completed");
    if output.is_json() {
        output.print_success(&BackupCreated {
            path: target.display().to_string(),
        });
    } else {
        println!("Backup created: {}", target.display());
    }
    Ok(())
}

/// Restore a database from a backup file.
///
/// Copies the backup file to a new location (the active database path with a
/// `.restored` suffix) so the user can inspect it before replacing the live
/// database. Prints instructions for completing the swap.
pub async fn restore(config: &ur_config::Config, source: &Path, output: &OutputManager) -> Result<()> {
    if !source.exists() {
        bail!("backup file not found: {}", source.display());
    }

    let db_path = config.config_dir.join("ur.db");
    let restore_target = config.config_dir.join("ur.db.restored");

    if restore_target.exists() {
        bail!(
            "restore target already exists: {} — remove it first",
            restore_target.display()
        );
    }

    info!(
        source = %source.display(),
        target = %restore_target.display(),
        "restoring database from backup"
    );

    let source_str = source.to_string_lossy().to_string();
    let target_str = restore_target.to_string_lossy().to_string();

    // Use SnapshotManager::restore to copy and verify schema integrity
    let _restored_db = ur_db::SnapshotManager::restore(&source_str, &target_str)
        .await
        .context("restore failed — backup may be corrupt or incompatible")?;

    if output.is_json() {
        output.print_text(&format!(
            "Restored database verified: {}",
            restore_target.display()
        ));
    } else {
        println!("Restored database verified: {}", restore_target.display());
        println!();
        println!("To complete the restore:");
        println!("  1. Stop the server: ur stop");
        println!(
            "  2. Replace the database: mv {} {}",
            restore_target.display(),
            db_path.display()
        );
        println!("  3. Start the server: ur start");
    }

    Ok(())
}

/// List available backup files.
pub fn list(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    let backup_path = match &config.backup.path {
        Some(p) => p,
        None => {
            output.print_text("No backup path configured — set [backup] path in ur.toml");
            return Ok(());
        }
    };

    if !backup_path.exists() {
        output.print_text(&format!(
            "Backup directory does not exist: {}",
            backup_path.display()
        ));
        return Ok(());
    }

    let mut entries: Vec<(String, u64)> = std::fs::read_dir(backup_path)
        .context("failed to read backup directory")?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let name_str = name.to_string_lossy().to_string();
            if name_str.starts_with("ur-backup-") && name_str.ends_with(".db") {
                let size = e.metadata().ok()?.len();
                Some((name_str, size))
            } else {
                None
            }
        })
        .collect();

    if entries.is_empty() {
        output.print_text(&format!("No backups found in {}", backup_path.display()));
        return Ok(());
    }

    // Sort descending (newest first)
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    debug!(count = entries.len(), "listing backup files");

    if output.is_json() {
        let backup_entries: Vec<BackupEntry> = entries
            .iter()
            .map(|(name, size)| {
                let timestamp = name
                    .strip_prefix("ur-backup-")
                    .and_then(|s| s.strip_suffix(".db"))
                    .unwrap_or("?")
                    .to_string();
                BackupEntry {
                    name: name.clone(),
                    timestamp,
                    size_bytes: *size,
                }
            })
            .collect();
        output.print_success(&BackupList {
            directory: backup_path.display().to_string(),
            retain_count: config.backup.retain_count,
            backups: backup_entries,
        });
    } else {
        println!(
            "Backups in {} (retain_count: {}):",
            backup_path.display(),
            config.backup.retain_count
        );
        for (name, size) in &entries {
            let size_display = format_size(*size);
            // Extract timestamp from filename: ur-backup-YYYYMMDDTHHMMSSZ.db
            let timestamp = name
                .strip_prefix("ur-backup-")
                .and_then(|s| s.strip_suffix(".db"))
                .unwrap_or("?");
            println!("  {timestamp}  {size_display:>8}  {name}");
        }
        println!("{} backup(s) total", entries.len());
    }

    Ok(())
}

/// Generate a timestamped backup filename.
fn backup_filename() -> String {
    let now = chrono::Utc::now();
    format!("ur-backup-{}.db", now.format("%Y%m%dT%H%M%SZ"))
}

/// Remove backup files that exceed the retain count.
fn clean_old_backups(backup_dir: &Path, current_filename: &str, retain_count: u64) {
    let entries = match std::fs::read_dir(backup_dir) {
        Ok(e) => e,
        Err(_) => return,
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

    backup_files.sort_by(|a, b| b.cmp(a));

    for name in backup_files.iter().skip(retain_count as usize) {
        if name == current_filename {
            continue;
        }
        let _ = std::fs::remove_file(backup_dir.join(name));
    }
}

/// Format a byte count as a human-readable size string.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
