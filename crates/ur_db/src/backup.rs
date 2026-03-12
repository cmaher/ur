use crate::schema::SchemaManager;

/// Manages database backup and restore operations for CozoDB instances.
#[derive(Clone)]
pub struct BackupManager {
    schema: SchemaManager,
}

impl BackupManager {
    /// Create a new BackupManager wrapping the given SchemaManager.
    pub fn new(schema: SchemaManager) -> Self {
        Self { schema }
    }

    /// Access the underlying SchemaManager.
    pub fn schema(&self) -> &SchemaManager {
        &self.schema
    }

    /// Create a backup of the database to the specified file path.
    ///
    /// The target path must not contain an existing database with data.
    /// The backup captures a consistent transactional snapshot and does not
    /// block concurrent writes.
    pub fn backup(&self, path: &std::path::Path) -> Result<(), String> {
        self.schema
            .db()
            .backup_db(path)
            .map_err(|e| format!("Backup failed: {e}"))
    }

    /// Restore a backup into the current database.
    ///
    /// The current database must be empty (freshly created with no relations or data).
    /// This is intended for disaster recovery into a new instance.
    pub fn restore(&self, path: &std::path::Path) -> Result<(), String> {
        self.schema
            .db()
            .restore_backup(path)
            .map_err(|e| format!("Restore failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::QueryManager;
    use tempfile::TempDir;

    /// Helper: insert sample data into a SchemaManager for testing.
    fn populate_sample_data(mgr: &SchemaManager) {
        mgr.run(
            r#"
            ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [
                ["ur.001", "epic", "open", 1, "",
                 "Test Epic", "An epic for testing backups.",
                 "2026-03-12T10:00:00Z", "2026-03-12T10:00:00Z"],
                ["ur.001.0", "task", "open", 2, "ur.001",
                 "Task Alpha", "First task.",
                 "2026-03-12T10:01:00Z", "2026-03-12T10:01:00Z"],
                ["ur.001.1", "task", "closed", 2, "ur.001",
                 "Task Beta", "Second task.",
                 "2026-03-12T10:02:00Z", "2026-03-12T10:02:00Z"],
                ["ur.001.2", "bug", "open", 3, "ur.001",
                 "Bug Gamma", "A bug to fix.",
                 "2026-03-12T10:03:00Z", "2026-03-12T10:03:00Z"]
            ]
            :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
        "#,
        )
        .expect("insert tickets");

        mgr.run(
            r#"
            ?[ticket_id, key, value] <- [
                ["ur.001", "assignee", "christian"],
                ["ur.001.0", "tag", "infra"],
                ["ur.001.1", "assignee", "agent-1"]
            ]
            :put ticket_meta {ticket_id, key => value}
        "#,
        )
        .expect("insert metadata");

        mgr.run(
            r#"
            ?[blocker_id, blocked_id] <- [
                ["ur.001.1", "ur.001.0"]
            ]
            :put blocks {blocker_id, blocked_id}
        "#,
        )
        .expect("insert blocks");

        mgr.run(
            r#"
            ?[id, ticket_id, timestamp, author, message] <- [
                ["act.b01", "ur.001.0", "2026-03-12T11:00:00Z", "agent-1",
                 "Started work on Task Alpha."]
            ]
            :put activity {id => ticket_id, timestamp, author, message}
        "#,
        )
        .expect("insert activity");
    }

    /// Verify that an in-memory DB can be backed up to an SQLite file.
    #[test]
    fn backup_in_memory_to_sqlite_file() {
        let mgr = SchemaManager::create_in_memory().expect("create in-memory db");
        populate_sample_data(&mgr);

        let tmp = TempDir::new().expect("create temp dir");
        let backup_path = tmp.path().join("backup.db");

        let bm = BackupManager::new(mgr);
        bm.backup(&backup_path).expect("backup should succeed");

        // Verify the backup file exists and has content
        assert!(backup_path.exists(), "backup file should exist");
        let metadata = std::fs::metadata(&backup_path).expect("read backup metadata");
        assert!(metadata.len() > 0, "backup file should not be empty");
    }

    /// Verify that a SQLite-backed DB can be backed up and the backup is readable.
    #[test]
    fn backup_sqlite_to_sqlite_and_verify_data() {
        let tmp = TempDir::new().expect("create temp dir");
        let db_path = tmp.path().join("primary.db");
        let backup_path = tmp.path().join("backup.db");

        let mgr = SchemaManager::create_with_sqlite(&db_path).expect("create sqlite db");
        populate_sample_data(&mgr);

        let bm = BackupManager::new(mgr);
        bm.backup(&backup_path).expect("backup should succeed");

        // Open the backup and verify data is intact
        let backup_mgr = SchemaManager::open_sqlite(&backup_path).expect("open backup db");
        let result = backup_mgr
            .run("?[id, title] := *ticket{id, title} :order id")
            .unwrap();
        assert_eq!(result.rows.len(), 4, "backup should contain all 4 tickets");

        // Verify specific ticket data survived the backup
        let first_id = result.rows[0][0].get_str().unwrap();
        assert_eq!(first_id, "ur.001");

        // Verify metadata survived
        let meta = backup_mgr
            .run("?[ticket_id, key, value] := *ticket_meta{ticket_id, key, value}")
            .unwrap();
        assert_eq!(
            meta.rows.len(),
            3,
            "backup should contain all 3 metadata entries"
        );

        // Verify blocks survived
        let blocks = backup_mgr
            .run("?[blocker_id, blocked_id] := *blocks{blocker_id, blocked_id}")
            .unwrap();
        assert_eq!(
            blocks.rows.len(),
            1,
            "backup should contain the blocks edge"
        );

        // Verify activity survived
        let activity = backup_mgr
            .run("?[id, message] := *activity{id, message}")
            .unwrap();
        assert_eq!(
            activity.rows.len(),
            1,
            "backup should contain the activity entry"
        );
    }

    /// Verify that query results are identical between the original and backup.
    #[test]
    fn backup_preserves_query_semantics() {
        let tmp = TempDir::new().expect("create temp dir");
        let db_path = tmp.path().join("primary.db");
        let backup_path = tmp.path().join("backup.db");

        let mgr = SchemaManager::create_with_sqlite(&db_path).expect("create sqlite db");
        populate_sample_data(&mgr);

        // Run queries on original
        let orig_qm = QueryManager::new(mgr.clone());
        let orig_dispatch = orig_qm.dispatchable_tickets("ur.001").unwrap();
        let orig_blockers = orig_qm.transitive_blockers("ur.001.0").unwrap();
        let orig_rollup = orig_qm.epic_all_children_closed("ur.001").unwrap();

        // Backup and run the same queries on the backup
        let bm = BackupManager::new(mgr);
        bm.backup(&backup_path).expect("backup should succeed");

        let backup_mgr = SchemaManager::open_sqlite(&backup_path).expect("open backup db");
        let backup_qm = QueryManager::new(backup_mgr);
        let backup_dispatch = backup_qm.dispatchable_tickets("ur.001").unwrap();
        let backup_blockers = backup_qm.transitive_blockers("ur.001.0").unwrap();
        let backup_rollup = backup_qm.epic_all_children_closed("ur.001").unwrap();

        // Compare results
        assert_eq!(orig_dispatch.len(), backup_dispatch.len());
        for (a, b) in orig_dispatch.iter().zip(backup_dispatch.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.title, b.title);
        }
        assert_eq!(orig_blockers, backup_blockers);
        assert_eq!(orig_rollup, backup_rollup);
    }

    /// Verify that backup fails if the target path already contains data.
    #[test]
    fn backup_fails_if_target_has_data() {
        let tmp = TempDir::new().expect("create temp dir");
        let db_path = tmp.path().join("primary.db");
        let backup_path = tmp.path().join("backup.db");

        let mgr = SchemaManager::create_with_sqlite(&db_path).expect("create sqlite db");
        populate_sample_data(&mgr);

        let bm = BackupManager::new(mgr);
        bm.backup(&backup_path)
            .expect("first backup should succeed");

        // Second backup to the same path should fail (data already exists)
        let err = bm
            .backup(&backup_path)
            .expect_err("second backup should fail");
        assert!(
            err.contains("data exists"),
            "error should mention existing data, got: {err}"
        );
    }

    /// Verify that backup can proceed safely while writes are happening.
    ///
    /// CozoDB's backup_db() opens a read transaction that captures a consistent snapshot.
    /// Writes that occur concurrently are not included in the backup but do not cause
    /// errors or corruption. This test verifies that:
    /// 1. Data written before backup appears in the backup
    /// 2. Data written after backup starts does not corrupt the source DB
    /// 3. The backup is a valid, queryable database
    #[test]
    fn backup_is_safe_during_writes() {
        let tmp = TempDir::new().expect("create temp dir");
        let db_path = tmp.path().join("primary.db");

        let mgr = SchemaManager::create_with_sqlite(&db_path).expect("create sqlite db");
        populate_sample_data(&mgr);

        // Take a backup
        let backup_path_1 = tmp.path().join("backup1.db");
        let bm = BackupManager::new(mgr.clone());
        bm.backup(&backup_path_1).expect("backup should succeed");

        // Write more data AFTER the backup
        mgr.run(
            r#"
            ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
                "ur.001.3", "task", "open", 1, "ur.001",
                "Post-backup task", "Added after backup.",
                "2026-03-12T12:00:00Z", "2026-03-12T12:00:00Z"
            ]]
            :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
        "#,
        )
        .expect("post-backup write should succeed");

        // Verify the source DB has the new data
        let source_count = mgr.run("?[id] := *ticket{id}").unwrap();
        assert_eq!(
            source_count.rows.len(),
            5,
            "source should have 5 tickets now"
        );

        // Verify the backup only has the original 4 tickets
        let backup_mgr = SchemaManager::open_sqlite(&backup_path_1).expect("open backup");
        let backup_count = backup_mgr.run("?[id] := *ticket{id}").unwrap();
        assert_eq!(
            backup_count.rows.len(),
            4,
            "backup should only have the original 4 tickets"
        );

        // Take a second backup that includes the new data
        let backup_path_2 = tmp.path().join("backup2.db");
        bm.backup(&backup_path_2)
            .expect("second backup should succeed");

        let backup_mgr_2 = SchemaManager::open_sqlite(&backup_path_2).expect("open second backup");
        let backup_count_2 = backup_mgr_2.run("?[id] := *ticket{id}").unwrap();
        assert_eq!(
            backup_count_2.rows.len(),
            5,
            "second backup should include all 5 tickets"
        );
    }

    /// Verify that a backup file can be deleted and re-created (the recommended
    /// rotation pattern for periodic backups).
    #[test]
    fn backup_rotation_via_delete_and_recreate() {
        let tmp = TempDir::new().expect("create temp dir");
        let db_path = tmp.path().join("primary.db");
        let backup_path = tmp.path().join("backup.db");

        let mgr = SchemaManager::create_with_sqlite(&db_path).expect("create sqlite db");
        populate_sample_data(&mgr);

        let bm = BackupManager::new(mgr.clone());

        // First backup
        bm.backup(&backup_path).expect("first backup");

        // Simulate adding data between backup cycles
        mgr.run(
            r#"
            ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
                "ur.002", "task", "open", 1, "",
                "New standalone task", "Created between backups.",
                "2026-03-12T13:00:00Z", "2026-03-12T13:00:00Z"
            ]]
            :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
        "#,
        )
        .expect("insert new ticket");

        // Delete old backup and create a new one
        std::fs::remove_file(&backup_path).expect("delete old backup");
        bm.backup(&backup_path)
            .expect("second backup after rotation");

        // Verify the rotated backup has all data including the new ticket
        let backup_mgr = SchemaManager::open_sqlite(&backup_path).expect("open rotated backup");
        let count = backup_mgr.run("?[id] := *ticket{id}").unwrap();
        assert_eq!(
            count.rows.len(),
            5,
            "rotated backup should have all 5 tickets"
        );
    }

    /// Verify that SQLite-backed databases persist across process restarts
    /// (simulated by dropping and reopening the SchemaManager).
    #[test]
    fn sqlite_persists_across_reopens() {
        let tmp = TempDir::new().expect("create temp dir");
        let db_path = tmp.path().join("persistent.db");

        // Create and populate
        {
            let mgr = SchemaManager::create_with_sqlite(&db_path).expect("create sqlite db");
            populate_sample_data(&mgr);

            let count = mgr.run("?[id] := *ticket{id}").unwrap();
            assert_eq!(count.rows.len(), 4);
        }
        // SchemaManager is dropped here, simulating process exit

        // Reopen the same file
        {
            let mgr = SchemaManager::open_sqlite(&db_path).expect("reopen sqlite db");
            let count = mgr.run("?[id] := *ticket{id}").unwrap();
            assert_eq!(count.rows.len(), 4, "data should persist after reopen");

            // Verify queries still work
            let qm = QueryManager::new(mgr);
            let dispatch = qm.dispatchable_tickets("ur.001").unwrap();
            assert!(!dispatch.is_empty(), "queries should work on reopened db");
        }
    }

    /// Verify that restore works into a fresh database.
    #[test]
    fn restore_into_fresh_database() {
        let tmp = TempDir::new().expect("create temp dir");
        let db_path = tmp.path().join("primary.db");
        let backup_path = tmp.path().join("backup.db");
        let restore_path = tmp.path().join("restored.db");

        // Create, populate, and backup
        let mgr = SchemaManager::create_with_sqlite(&db_path).expect("create sqlite db");
        populate_sample_data(&mgr);

        let bm = BackupManager::new(mgr);
        bm.backup(&backup_path).expect("backup");

        // Restore into a fresh database
        let fresh_db = cozo::DbInstance::new("sqlite", restore_path.to_str().unwrap(), "")
            .expect("create fresh db for restore");
        let fresh_mgr = SchemaManager::new(fresh_db);
        let restore_bm = BackupManager::new(fresh_mgr.clone());
        restore_bm
            .restore(&backup_path)
            .expect("restore should succeed");

        // Verify restored data
        let count = fresh_mgr.run("?[id] := *ticket{id}").unwrap();
        assert_eq!(count.rows.len(), 4, "restored db should have all 4 tickets");

        // Verify queries work on restored data
        let qm = QueryManager::new(fresh_mgr);
        let dispatch = qm.dispatchable_tickets("ur.001").unwrap();
        assert!(!dispatch.is_empty(), "queries should work on restored db");
    }

    /// Verify that restore fails if the target database already has data.
    #[test]
    fn restore_fails_if_target_has_data() {
        let tmp = TempDir::new().expect("create temp dir");
        let db_path = tmp.path().join("primary.db");
        let backup_path = tmp.path().join("backup.db");

        let mgr = SchemaManager::create_with_sqlite(&db_path).expect("create sqlite db");
        populate_sample_data(&mgr);

        let bm = BackupManager::new(mgr.clone());
        bm.backup(&backup_path).expect("backup");

        // Try to restore into the same non-empty database
        let err = bm
            .restore(&backup_path)
            .expect_err("restore into non-empty db should fail");
        assert!(
            err.contains("data exists"),
            "error should mention existing data, got: {err}"
        );
    }
}
