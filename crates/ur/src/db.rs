use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tokio::process::Command;
use tracing::{debug, info};

/// Manages pg_dump/pg_restore via docker exec into the postgres container.
struct SnapshotManager {
    container_command: String,
    container_name: String,
    db_name: String,
}

impl SnapshotManager {
    fn new(container_command: String, container_name: String, db_name: String) -> Self {
        Self {
            container_command,
            container_name,
            db_name,
        }
    }

    async fn dump_to(&self, filename: &str) -> Result<(), String> {
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

    async fn restore_from(&self, filename: &str) -> Result<(), String> {
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

use crate::output::{BackupCreated, BackupEntry, BackupList, OutputManager};

/// Run an on-demand database backup via pg_dump into the postgres container.
pub async fn backup(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    let backup_path = config
        .db
        .backup
        .path
        .as_ref()
        .context("no backup path configured — set [db.backup] path in ur.toml")?;

    if !backup_path.exists() {
        bail!(
            "backup directory does not exist: {} — run 'ur init' first",
            backup_path.display()
        );
    }

    info!(backup_dir = %backup_path.display(), "creating on-demand backup via pg_dump");

    let snapshot_manager = SnapshotManager::new(
        config.server.container_command.clone(),
        ur_config::DEFAULT_DB_HOST.to_string(),
        config.db.name.clone(),
    );
    let filename = backup_filename();
    let target = backup_path.join(&filename);

    snapshot_manager
        .dump_to(&filename)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    clean_old_backups(backup_path, &filename, config.db.backup.retain_count);

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

/// Restore a database from a .pgdump backup file via pg_restore.
///
/// Restores directly into the live Postgres database using pg_restore
/// with --clean --if-exists. No server restart is needed.
pub async fn restore(
    config: &ur_config::Config,
    source: &Path,
    output: &OutputManager,
) -> Result<()> {
    if !source.exists() {
        bail!("backup file not found: {}", source.display());
    }

    let filename = source
        .file_name()
        .context("invalid backup file path")?
        .to_string_lossy();

    info!(
        source = %source.display(),
        "restoring database from backup via pg_restore"
    );

    let snapshot_manager = SnapshotManager::new(
        config.server.container_command.clone(),
        ur_config::DEFAULT_DB_HOST.to_string(),
        config.db.name.clone(),
    );

    snapshot_manager
        .restore_from(&filename)
        .await
        .map_err(|e| anyhow::anyhow!("restore failed: {e}"))?;

    if output.is_json() {
        output.print_text(&format!("Database restored from {}", source.display()));
    } else {
        println!("Database restored from {}", source.display());
    }

    Ok(())
}

/// List available backup files.
pub fn list(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    let backup_path = match &config.db.backup.path {
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
            if is_backup_file(&name_str) {
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
                let timestamp = backup_timestamp(name).to_string();
                BackupEntry {
                    name: name.clone(),
                    timestamp,
                    size_bytes: *size,
                }
            })
            .collect();
        output.print_success(&BackupList {
            directory: backup_path.display().to_string(),
            retain_count: config.db.backup.retain_count,
            backups: backup_entries,
        });
    } else {
        println!(
            "Backups in {} (retain_count: {}):",
            backup_path.display(),
            config.db.backup.retain_count
        );
        for (name, size) in &entries {
            let size_display = format_size(*size);
            let timestamp = backup_timestamp(name);
            let label = if name.starts_with("manual-") {
                " [manual]"
            } else {
                ""
            };
            println!("  {timestamp}  {size_display:>8}  {name}{label}");
        }
        println!("{} backup(s) total", entries.len());
    }

    Ok(())
}

/// Generate a timestamped manual backup filename.
fn backup_filename() -> String {
    let now = chrono::Utc::now();
    format!("manual-ur-backup-{}.pgdump", now.format("%Y%m%dT%H%M%SZ"))
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
            if name_str.starts_with("ur-backup-") && name_str.ends_with(".pgdump") {
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

/// Check whether a filename is any kind of backup file (automatic or manual).
fn is_backup_file(name: &str) -> bool {
    name.ends_with(".pgdump")
        && (name.starts_with("ur-backup-") || name.starts_with("manual-ur-backup-"))
}

/// Extract the timestamp portion from a backup filename.
fn backup_timestamp(name: &str) -> &str {
    name.strip_prefix("manual-ur-backup-")
        .or_else(|| name.strip_prefix("ur-backup-"))
        .and_then(|s| s.strip_suffix(".pgdump"))
        .unwrap_or("?")
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
