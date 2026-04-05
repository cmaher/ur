// Tests for SnapshotManager.
//
// SnapshotManager now delegates to docker exec pg_dump/pg_restore,
// so integration tests require a running postgres container. These
// unit tests verify construction and API surface.

use crate::snapshot::SnapshotManager;

#[test]
fn snapshot_manager_construction() {
    let sm = SnapshotManager::new(
        "docker".to_string(),
        "ur-postgres".to_string(),
        "ur".to_string(),
    );
    // Verify Clone works (required by BackupTaskManager)
    let _cloned = sm.clone();
}

#[test]
fn snapshot_manager_with_nerdctl() {
    let sm = SnapshotManager::new(
        "nerdctl".to_string(),
        "ur-postgres".to_string(),
        "mydb".to_string(),
    );
    let _cloned = sm.clone();
}
