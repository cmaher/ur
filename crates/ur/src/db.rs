use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tokio::process::Command;
use tracing::{debug, info, warn};
use ur_config::BackupConfig;
use ur_config::backup::{
    BACKUP_TIMESTAMP_FORMAT, MANUAL_BACKUP_PREFIX, backup_container_dir, clean_old_backups,
    manual_backup_filename, parse_backup_filename,
};

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

    /// Container-side path of `filename` in this database's backup directory.
    fn container_backup_path(&self, filename: &str) -> String {
        format!("{}/{filename}", backup_container_dir(&self.db_name))
    }

    async fn dump_to(&self, filename: &str) -> Result<(), String> {
        let backup_path = self.container_backup_path(filename);
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

    /// Read the source database name recorded in a pg_dump archive's header.
    ///
    /// Runs: docker exec <container> pg_restore -l /backup/<dbname>/<filename>
    async fn archive_db_name(&self, filename: &str) -> Result<String, String> {
        let backup_path = self.container_backup_path(filename);
        let output = Command::new(&self.container_command)
            .args([
                "exec",
                &self.container_name,
                "pg_restore",
                "-l",
                &backup_path,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("failed to run {}: {e}", self.container_command))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("pg_restore -l failed: {stderr}"));
        }
        let listing = String::from_utf8_lossy(&output.stdout);
        archive_header_db_name(&listing)
            .map(str::to_owned)
            .ok_or_else(|| format!("no dbname in archive header of {filename}"))
    }

    async fn restore_from(&self, filename: &str) -> Result<(), String> {
        let backup_path = self.container_backup_path(filename);
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

/// Extract the `dbname:` value from `pg_restore -l` output.
fn archive_header_db_name(listing: &str) -> Option<&str> {
    listing.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix(';')?
            .trim()
            .strip_prefix("dbname:")
            .map(str::trim)
    })
}

use crate::output::{
    BackupCreated, BackupEntry, BackupList, BackupLists, BackupsCreated, OutputManager,
};

/// Which live database a `ur db` command operates on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DbTarget {
    /// The ticket database (`[ticket_db]`, default `ur_tickets`)
    Tickets,
    /// The workflow database (`[workflow_db]`, default `ur_workflow`)
    Workflow,
}

impl DbTarget {
    const ALL: [DbTarget; 2] = [DbTarget::Tickets, DbTarget::Workflow];

    /// The `ur.toml` section configuring this database's backups.
    fn backup_section(self) -> &'static str {
        match self {
            DbTarget::Tickets => "[ticket_db.backup]",
            DbTarget::Workflow => "[workflow_db.backup]",
        }
    }
}

/// A database resolved from config together with its backup settings.
struct BackupTarget<'a> {
    target: DbTarget,
    db_name: &'a str,
    backup: &'a BackupConfig,
}

fn backup_target(config: &ur_config::Config, target: DbTarget) -> BackupTarget<'_> {
    match target {
        DbTarget::Tickets => BackupTarget {
            target,
            db_name: &config.ticket_db.name,
            backup: &config.ticket_db.backup,
        },
        DbTarget::Workflow => BackupTarget {
            target,
            db_name: &config.workflow_db.name,
            backup: &config.workflow_db.backup,
        },
    }
}

/// The databases a command covers: the one selected by `--db`, or both.
fn selected_targets(config: &ur_config::Config, db: Option<DbTarget>) -> Vec<BackupTarget<'_>> {
    match db {
        Some(target) => vec![backup_target(config, target)],
        None => DbTarget::ALL
            .into_iter()
            .map(|target| backup_target(config, target))
            .collect(),
    }
}

/// Run an on-demand backup of the live databases via pg_dump into the postgres container.
///
/// With `db` set, backs up only that database and fails if it has no backup
/// path. Without it, backs up every database that has a backup path, reports
/// the ones skipped, and fails if neither has one.
pub async fn backup(
    config: &ur_config::Config,
    db: Option<DbTarget>,
    output: &OutputManager,
) -> Result<()> {
    let mut created = Vec::new();
    let mut skipped = Vec::new();
    let mut failures = Vec::new();
    for target in selected_targets(config, db) {
        let Some(backup_path) = target.backup.path.as_ref() else {
            if db.is_some() {
                bail!(
                    "no backup path configured for {} — set {} path in ur.toml",
                    target.db_name,
                    target.target.backup_section()
                );
            }
            skipped.push(target.db_name.to_owned());
            continue;
        };
        match backup_one(config, &target, backup_path).await {
            Ok(path) => created.push(BackupCreated {
                database: target.db_name.to_owned(),
                path: path.display().to_string(),
            }),
            Err(e) => failures.push(format!("{}: {e:#}", target.db_name)),
        }
    }

    if !failures.is_empty() {
        let succeeded: Vec<String> = created
            .iter()
            .map(|b| format!("{} → {}", b.database, b.path))
            .collect();
        bail!(
            "backup failed for {}{}",
            failures.join("; "),
            if succeeded.is_empty() {
                String::new()
            } else {
                format!(" (succeeded: {})", succeeded.join(", "))
            }
        );
    }

    if created.is_empty() {
        bail!(
            "no backup path configured — set [ticket_db.backup] and/or [workflow_db.backup] path in ur.toml"
        );
    }

    if output.is_json() {
        output.print_success(&BackupsCreated {
            backups: created,
            skipped,
        });
    } else {
        for backup in &created {
            println!("Backup created ({}): {}", backup.database, backup.path);
        }
        for db_name in &skipped {
            println!("Skipped {db_name}: no backup path configured");
        }
    }
    Ok(())
}

/// Dump one database into its backup directory and apply its retention.
async fn backup_one(
    config: &ur_config::Config,
    target: &BackupTarget<'_>,
    backup_path: &Path,
) -> Result<std::path::PathBuf> {
    if !backup_path.exists() {
        bail!(
            "backup directory for {} does not exist: {} — run 'ur init' first",
            target.db_name,
            backup_path.display()
        );
    }

    info!(
        db = target.db_name,
        backup_dir = %backup_path.display(),
        "creating on-demand backup via pg_dump"
    );

    let snapshot_manager = SnapshotManager::new(
        config.server.container_command.clone(),
        ur_config::DEFAULT_DB_HOST.to_string(),
        target.db_name.to_owned(),
    );
    let timestamp = chrono::Utc::now()
        .format(BACKUP_TIMESTAMP_FORMAT)
        .to_string();
    let filename = manual_backup_filename(target.db_name, &timestamp);
    let dest = backup_path.join(&filename);

    snapshot_manager
        .dump_to(&filename)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    for failure in clean_old_backups(
        backup_path,
        target.db_name,
        &filename,
        target.backup.retain_count,
    ) {
        warn!(db = target.db_name, error = %failure, "backup retention cleanup failed");
        eprintln!("warning: {failure}");
    }

    info!(path = %dest.display(), "backup completed");
    Ok(dest)
}

/// Restore a live database from a .pgdump backup file via pg_restore.
///
/// The target database comes from `db`, or else from the database name in the
/// backup's filename; legacy filenames without one require `db`. The file must
/// sit in that database's configured backup directory, since that is the only
/// host directory mounted at its container path. Restores directly into the
/// live Postgres database using pg_restore with --clean --if-exists. No server
/// restart is needed.
pub async fn restore(
    config: &ur_config::Config,
    source: &Path,
    db: Option<DbTarget>,
    output: &OutputManager,
) -> Result<()> {
    if !source.exists() {
        bail!("backup file not found: {}", source.display());
    }

    let filename = source
        .file_name()
        .context("invalid backup file path")?
        .to_string_lossy()
        .to_string();

    let target = resolve_restore_target(config, &filename, db)?;
    ensure_in_backup_dir(&target, source)?;

    info!(
        source = %source.display(),
        db = target.db_name,
        "restoring database from backup via pg_restore"
    );

    let snapshot_manager = SnapshotManager::new(
        config.server.container_command.clone(),
        ur_config::DEFAULT_DB_HOST.to_string(),
        target.db_name.to_owned(),
    );

    // The filename is only a claim; the archive header records which database
    // was actually dumped. Legacy unnamed files are often dumps of the pre-split
    // `ur` database, which must never be restored over a live one.
    let archive_db = snapshot_manager
        .archive_db_name(&filename)
        .await
        .map_err(|e| anyhow::anyhow!("failed to inspect backup: {e}"))?;
    if archive_db != target.db_name {
        bail!(
            "{filename} is a dump of database '{archive_db}', not '{}' — refusing to restore",
            target.db_name
        );
    }

    snapshot_manager
        .restore_from(&filename)
        .await
        .map_err(|e| anyhow::anyhow!("restore failed: {e}"))?;

    output.print_text(&format!(
        "Database {} restored from {}",
        target.db_name,
        source.display()
    ));

    Ok(())
}

/// Pick the database a backup file restores into.
fn resolve_restore_target<'a>(
    config: &'a ur_config::Config,
    filename: &str,
    db: Option<DbTarget>,
) -> Result<BackupTarget<'a>> {
    let file_db = parse_backup_filename(filename).and_then(|parsed| parsed.db_name);
    if let Some(target) = db {
        let target = backup_target(config, target);
        if let Some(file_db) = file_db
            && file_db != target.db_name
        {
            bail!(
                "{filename} is a backup of database '{file_db}', not '{}'",
                target.db_name
            );
        }
        return Ok(target);
    }
    let Some(file_db) = file_db else {
        bail!(
            "cannot tell which database {filename} belongs to — pass --db tickets or --db workflow"
        );
    };
    DbTarget::ALL
        .into_iter()
        .map(|target| backup_target(config, target))
        .find(|target| target.db_name == file_db)
        .with_context(|| {
            format!(
                "{filename} is a backup of database '{file_db}', which matches neither \
                 ticket_db ({}) nor workflow_db ({})",
                config.ticket_db.name, config.workflow_db.name
            )
        })
}

/// Require `source` to live directly in the target database's backup directory.
fn ensure_in_backup_dir(target: &BackupTarget<'_>, source: &Path) -> Result<()> {
    let backup_path = target.backup.path.as_ref().with_context(|| {
        format!(
            "no backup path configured for {} — set {} path in ur.toml",
            target.db_name,
            target.target.backup_section()
        )
    })?;
    let source_dir = source
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", source.display()))?
        .parent()
        .map(Path::to_path_buf)
        .context("invalid backup file path")?;
    let backup_dir = backup_path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", backup_path.display()))?;
    if source_dir != backup_dir {
        bail!(
            "{} must be in the {} backup directory {} (the directory mounted into the \
             postgres container) — copy it there first",
            source.display(),
            target.db_name,
            backup_path.display()
        );
    }
    Ok(())
}

/// List available backup files for the selected databases.
pub fn list(
    config: &ur_config::Config,
    db: Option<DbTarget>,
    output: &OutputManager,
) -> Result<()> {
    let mut lists = Vec::new();
    let mut skipped = Vec::new();
    for target in selected_targets(config, db) {
        let Some(backup_path) = target.backup.path.as_ref() else {
            if db.is_some() {
                bail!(
                    "no backup path configured for {} — set {} path in ur.toml",
                    target.db_name,
                    target.target.backup_section()
                );
            }
            skipped.push(target.db_name.to_owned());
            continue;
        };
        let backups = list_backup_entries(target.db_name, backup_path)?;
        debug!(
            db = target.db_name,
            count = backups.len(),
            "listing backup files"
        );
        lists.push(BackupList {
            database: target.db_name.to_owned(),
            directory: backup_path.display().to_string(),
            retain_count: target.backup.retain_count,
            backups,
        });
    }

    if output.is_json() {
        output.print_success(&BackupLists {
            databases: lists,
            skipped,
        });
        return Ok(());
    }
    for list in &lists {
        print_backup_list(list);
    }
    for db_name in &skipped {
        println!("{db_name}: no backup path configured");
    }
    Ok(())
}

/// Read a database's backup files from `backup_path`, newest first.
///
/// Includes legacy backups whose filename carries no database name (their
/// `database` is `None`), since they may belong to this database.
fn list_backup_entries(db_name: &str, backup_path: &Path) -> Result<Vec<BackupEntry>> {
    if !backup_path.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<BackupEntry> = std::fs::read_dir(backup_path)
        .with_context(|| format!("failed to read backup directory {}", backup_path.display()))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let parsed = parse_backup_filename(&name)?;
            if parsed.db_name.is_some_and(|db| db != db_name) {
                return None;
            }
            let database = parsed.db_name.map(str::to_owned);
            let timestamp = parsed.timestamp.to_owned();
            let size_bytes = e.metadata().ok()?.len();
            Some(BackupEntry {
                name,
                database,
                timestamp,
                size_bytes,
            })
        })
        .collect();
    // Sort descending (newest first)
    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(entries)
}

fn print_backup_list(list: &BackupList) {
    if list.backups.is_empty() {
        println!("{}: no backups found in {}", list.database, list.directory);
        return;
    }
    println!(
        "{} backups in {} (retain_count: {}):",
        list.database, list.directory, list.retain_count
    );
    for entry in &list.backups {
        let size_display = format_size(entry.size_bytes);
        let mut label = String::new();
        if entry.name.starts_with(MANUAL_BACKUP_PREFIX) {
            label.push_str(" [manual]");
        }
        if entry.database.is_none() {
            label.push_str(" [legacy: database unknown]");
        }
        println!(
            "  {}  {size_display:>8}  {}{label}",
            entry.timestamp, entry.name
        );
    }
    println!("{} backup(s) total", list.backups.len());
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use ur_config::backup::automatic_backup_filename;

    const TS: &str = "20260101T000000Z";

    fn config_with_backup_dirs(tickets: &Path, workflow: &Path) -> ur_config::Config {
        let toml = format!(
            "[ticket_db.backup]\npath = \"{}\"\n[workflow_db.backup]\npath = \"{}\"\n",
            tickets.display(),
            workflow.display()
        );
        ur_config::Config::from_toml_str(&toml, Path::new("/tmp")).unwrap()
    }

    #[test]
    fn archive_header_db_name_parsed() {
        let listing = ";\n; Archive created at 2026-01-01 00:00:00 UTC\n;     dbname: ur_tickets\n;     TOC Entries: 42\n";
        assert_eq!(archive_header_db_name(listing), Some("ur_tickets"));
        assert_eq!(archive_header_db_name("; no header here\n"), None);
    }

    #[test]
    fn restore_target_inferred_from_filename() {
        let tmp = TempDir::new().unwrap();
        let config = config_with_backup_dirs(tmp.path(), tmp.path());
        let name = automatic_backup_filename(ur_config::DEFAULT_WORKFLOW_DB_NAME, TS);
        let target = resolve_restore_target(&config, &name, None).unwrap();
        assert_eq!(target.target, DbTarget::Workflow);
        assert_eq!(target.db_name, ur_config::DEFAULT_WORKFLOW_DB_NAME);
    }

    #[test]
    fn restore_target_legacy_filename_requires_db() {
        let tmp = TempDir::new().unwrap();
        let config = config_with_backup_dirs(tmp.path(), tmp.path());
        let legacy = "ur-backup-20260101T000000Z.pgdump";
        let err = resolve_restore_target(&config, legacy, None)
            .err()
            .expect("legacy name without --db must fail");
        assert!(err.to_string().contains("--db"), "{err}");
        let target = resolve_restore_target(&config, legacy, Some(DbTarget::Tickets)).unwrap();
        assert_eq!(target.db_name, ur_config::DEFAULT_TICKET_DB_NAME);
    }

    #[test]
    fn restore_target_rejects_db_mismatch() {
        let tmp = TempDir::new().unwrap();
        let config = config_with_backup_dirs(tmp.path(), tmp.path());
        let name = automatic_backup_filename(ur_config::DEFAULT_TICKET_DB_NAME, TS);
        let err = resolve_restore_target(&config, &name, Some(DbTarget::Workflow))
            .err()
            .expect("mismatched --db must fail");
        assert!(err.to_string().contains("not 'ur_workflow'"), "{err}");
    }

    #[test]
    fn restore_target_rejects_unknown_db() {
        let tmp = TempDir::new().unwrap();
        let config = config_with_backup_dirs(tmp.path(), tmp.path());
        let name = automatic_backup_filename("ur", TS);
        let err = resolve_restore_target(&config, &name, None)
            .err()
            .expect("legacy 'ur' DB must not match");
        assert!(err.to_string().contains("matches neither"), "{err}");
    }

    #[test]
    fn restore_requires_file_in_target_backup_dir() {
        let tickets = TempDir::new().unwrap();
        let workflow = TempDir::new().unwrap();
        let config = config_with_backup_dirs(tickets.path(), workflow.path());
        let name = automatic_backup_filename(ur_config::DEFAULT_TICKET_DB_NAME, TS);

        let in_place = tickets.path().join(&name);
        std::fs::write(&in_place, "x").unwrap();
        let target = backup_target(&config, DbTarget::Tickets);
        ensure_in_backup_dir(&target, &in_place).unwrap();

        let elsewhere = workflow.path().join(&name);
        std::fs::write(&elsewhere, "x").unwrap();
        let err = ensure_in_backup_dir(&target, &elsewhere).expect_err("wrong dir must fail");
        assert!(err.to_string().contains("copy it there first"), "{err}");
    }

    #[test]
    fn list_entries_scoped_to_db_including_legacy() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let tickets = ur_config::DEFAULT_TICKET_DB_NAME;
        let workflow = ur_config::DEFAULT_WORKFLOW_DB_NAME;
        for name in [
            automatic_backup_filename(tickets, "20260101T000000Z"),
            manual_backup_filename(tickets, "20260103T000000Z"),
            automatic_backup_filename(workflow, "20260104T000000Z"),
            "ur-backup-20260102T000000Z.pgdump".to_string(),
            "other.txt".to_string(),
        ] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        let names: Vec<String> = list_backup_entries(tickets, dir)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            vec![
                manual_backup_filename(tickets, "20260103T000000Z"),
                "ur-backup-20260102T000000Z.pgdump".to_string(),
                automatic_backup_filename(tickets, "20260101T000000Z"),
            ]
        );
    }
}
