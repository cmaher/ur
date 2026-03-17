// Tests for SnapshotManager.

use crate::database::DatabaseManager;
use crate::graph::GraphManager;
use crate::model::{NewTicket, TicketFilter};
use crate::snapshot::SnapshotManager;
use crate::tests::TestDb;
use crate::ticket_repo::TicketRepo;

/// Build a TicketRepo from a DatabaseManager.
fn repo_from_db(db: &DatabaseManager) -> TicketRepo {
    let pool = db.pool().clone();
    let graph_manager = GraphManager::new(pool.clone());
    TicketRepo::new(pool, graph_manager)
}

/// Generate a unique temp file path for snapshots.
fn snapshot_path() -> String {
    let name = format!("ur_snap_{}.db", uuid::Uuid::new_v4());
    std::env::temp_dir().join(name).to_str().unwrap().to_owned()
}

/// Remove a file if it exists, ignoring errors.
fn cleanup_file(path: &str) {
    std::fs::remove_file(path).ok();
}

#[tokio::test]
async fn vacuum_into_creates_valid_snapshot() {
    let db = TestDb::new().await;
    let snap_path = snapshot_path();

    let manager = SnapshotManager::new(db.db().pool().clone());
    manager.vacuum_into(&snap_path).await.unwrap();

    // The snapshot file must exist.
    assert!(
        std::path::Path::new(&snap_path).exists(),
        "snapshot file should exist after vacuum_into"
    );

    // The snapshot must be a valid SQLite database that DatabaseManager can open.
    let restored_db = DatabaseManager::open(&snap_path).await.unwrap();
    restored_db.pool().close().await;

    cleanup_file(&snap_path);
    db.cleanup().await;
}

#[tokio::test]
async fn vacuum_into_data_survives() {
    let db = TestDb::new().await;
    let repo = repo_from_db(db.db());

    // Insert test data before snapshotting.
    repo.create_ticket(&NewTicket {
        id: "snap-t1".into(),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "Snapshot ticket one".into(),
        body: "Body one".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: "snap-t2".into(),
        type_: "epic".into(),
        priority: 2,
        parent_id: None,
        title: "Snapshot ticket two".into(),
        body: "Body two".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.set_meta("snap-t1", "ticket", "env", "prod")
        .await
        .unwrap();

    repo.add_activity("snap-t1", "tester", "created ticket")
        .await
        .unwrap();

    // Take a snapshot.
    let snap_path = snapshot_path();
    let manager = SnapshotManager::new(db.db().pool().clone());
    manager.vacuum_into(&snap_path).await.unwrap();

    // Open the snapshot and verify data.
    let snap_db = DatabaseManager::open(&snap_path).await.unwrap();
    let snap_repo = repo_from_db(&snap_db);

    let t1 = snap_repo.get_ticket("snap-t1").await.unwrap().unwrap();
    assert_eq!(t1.title, "Snapshot ticket one");
    assert_eq!(t1.body, "Body one");
    assert_eq!(t1.type_, "task");

    let t2 = snap_repo.get_ticket("snap-t2").await.unwrap().unwrap();
    assert_eq!(t2.title, "Snapshot ticket two");
    assert_eq!(t2.type_, "epic");

    let meta = snap_repo.get_meta("snap-t1", "ticket").await.unwrap();
    assert_eq!(meta.get("env").unwrap(), "prod");

    let activities = snap_repo.get_activities("snap-t1").await.unwrap();
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].message, "created ticket");

    snap_db.pool().close().await;
    cleanup_file(&snap_path);
    db.cleanup().await;
}

#[tokio::test]
async fn restore_into_new_file() {
    let db = TestDb::new().await;
    let repo = repo_from_db(db.db());

    // Seed data.
    repo.create_ticket(&NewTicket {
        id: "restore-t1".into(),
        type_: "task".into(),
        priority: 3,
        parent_id: None,
        title: "Restore test ticket".into(),
        body: "Restore body".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.set_meta("restore-t1", "ticket", "region", "us-east")
        .await
        .unwrap();

    // Snapshot.
    let snap_path = snapshot_path();
    let manager = SnapshotManager::new(db.db().pool().clone());
    manager.vacuum_into(&snap_path).await.unwrap();

    // Restore to a new file.
    let restore_path = snapshot_path();
    let restored_db = SnapshotManager::restore(&snap_path, &restore_path)
        .await
        .unwrap();

    let restored_repo = repo_from_db(&restored_db);

    let t1 = restored_repo
        .get_ticket("restore-t1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t1.title, "Restore test ticket");
    assert_eq!(t1.priority, 3);

    let meta = restored_repo
        .get_meta("restore-t1", "ticket")
        .await
        .unwrap();
    assert_eq!(meta.get("region").unwrap(), "us-east");

    restored_db.pool().close().await;
    cleanup_file(&restore_path);
    cleanup_file(&snap_path);
    db.cleanup().await;
}

#[tokio::test]
async fn restore_fails_if_target_exists() {
    let db = TestDb::new().await;

    // Snapshot.
    let snap_path = snapshot_path();
    let manager = SnapshotManager::new(db.db().pool().clone());
    manager.vacuum_into(&snap_path).await.unwrap();

    // Create a file at the target path so restore should fail.
    let target_path = snapshot_path();
    std::fs::write(&target_path, b"occupied").unwrap();

    let result = SnapshotManager::restore(&snap_path, &target_path).await;
    assert!(result.is_err(), "restore should fail when target exists");

    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("already exists"),
        "error message should mention 'already exists', got: {err_msg}"
    );

    cleanup_file(&target_path);
    cleanup_file(&snap_path);
    db.cleanup().await;
}

#[tokio::test]
async fn restore_fails_if_source_missing() {
    let nonexistent = snapshot_path();
    let target_path = snapshot_path();

    let result = SnapshotManager::restore(&nonexistent, &target_path).await;
    assert!(
        result.is_err(),
        "restore should fail when source is missing"
    );

    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("does not exist"),
        "error message should mention 'does not exist', got: {err_msg}"
    );

    // No cleanup needed -- neither file should exist.
}

#[tokio::test]
async fn snapshot_data_integrity_full_round_trip() {
    let db = TestDb::new().await;
    let repo = repo_from_db(db.db());

    // Build a richer dataset: parent epic + children + metadata + activities + edges.
    repo.create_ticket(&NewTicket {
        id: "int-epic".into(),
        type_: "epic".into(),
        priority: 1,
        parent_id: None,
        title: "Integrity epic".into(),
        body: "Epic body".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: "int-t1".into(),
        type_: "task".into(),
        priority: 1,
        parent_id: Some("int-epic".into()),
        title: "Integrity task one".into(),
        body: "Task one body".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: "int-t2".into(),
        type_: "task".into(),
        priority: 2,
        parent_id: Some("int-epic".into()),
        title: "Integrity task two".into(),
        body: "Task two body".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // Metadata on tickets.
    repo.set_meta("int-t1", "ticket", "component", "backend")
        .await
        .unwrap();
    repo.set_meta("int-t2", "ticket", "component", "frontend")
        .await
        .unwrap();

    // Edge: int-t1 blocks int-t2.
    repo.add_edge("int-t1", "int-t2", crate::model::EdgeKind::Blocks)
        .await
        .unwrap();

    // Activities.
    repo.add_activity("int-t1", "alice", "started work")
        .await
        .unwrap();
    repo.add_activity("int-t1", "bob", "code review done")
        .await
        .unwrap();
    repo.add_activity("int-t2", "alice", "waiting on int-t1")
        .await
        .unwrap();

    // --- Snapshot and restore ---
    let snap_path = snapshot_path();
    let manager = SnapshotManager::new(db.db().pool().clone());
    manager.vacuum_into(&snap_path).await.unwrap();

    let restore_path = snapshot_path();
    let restored_db = SnapshotManager::restore(&snap_path, &restore_path)
        .await
        .unwrap();
    let restored_repo = repo_from_db(&restored_db);

    // Verify tickets.
    let all_tickets = restored_repo
        .list_tickets(&TicketFilter {
            project: None,
            status: None,
            type_: None,
            parent_id: None,
            lifecycle_status: None,
        })
        .await
        .unwrap();
    assert_eq!(all_tickets.len(), 3);

    let epic = restored_repo.get_ticket("int-epic").await.unwrap().unwrap();
    assert_eq!(epic.type_, "epic");
    assert_eq!(epic.title, "Integrity epic");

    let t1 = restored_repo.get_ticket("int-t1").await.unwrap().unwrap();
    assert_eq!(t1.parent_id.as_deref(), Some("int-epic"));
    assert_eq!(t1.title, "Integrity task one");

    let t2 = restored_repo.get_ticket("int-t2").await.unwrap().unwrap();
    assert_eq!(t2.parent_id.as_deref(), Some("int-epic"));

    // Verify metadata.
    let t1_meta = restored_repo.get_meta("int-t1", "ticket").await.unwrap();
    assert_eq!(t1_meta.get("component").unwrap(), "backend");
    let t2_meta = restored_repo.get_meta("int-t2", "ticket").await.unwrap();
    assert_eq!(t2_meta.get("component").unwrap(), "frontend");

    // Verify edges.
    let edges = restored_repo
        .edges_for("int-t1", Some(crate::model::EdgeKind::Blocks))
        .await
        .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].source_id, "int-t1");
    assert_eq!(edges[0].target_id, "int-t2");

    // Verify activities.
    let t1_acts = restored_repo.get_activities("int-t1").await.unwrap();
    assert_eq!(t1_acts.len(), 2);
    let messages: Vec<&str> = t1_acts.iter().map(|a| a.message.as_str()).collect();
    assert!(messages.contains(&"started work"));
    assert!(messages.contains(&"code review done"));

    let t2_acts = restored_repo.get_activities("int-t2").await.unwrap();
    assert_eq!(t2_acts.len(), 1);
    assert_eq!(t2_acts[0].message, "waiting on int-t1");

    restored_db.pool().close().await;
    cleanup_file(&restore_path);
    cleanup_file(&snap_path);
    db.cleanup().await;
}
