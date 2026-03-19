// Tests for WorkerRepo.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::model::{Slot, Worker};
use crate::tests::TestDb;
use crate::worker_repo::WorkerRepo;

fn repo(db: &TestDb) -> WorkerRepo {
    WorkerRepo::new(db.db().pool().clone())
}

fn test_slot(id: &str, project_key: &str) -> Slot {
    Slot {
        id: id.to_owned(),
        project_key: project_key.to_owned(),
        slot_name: format!("slot-{id}"),
        host_path: format!("/tmp/{id}"),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

fn test_worker(worker_id: &str, project_key: &str) -> Worker {
    Worker {
        worker_id: worker_id.to_owned(),
        process_id: format!("proc-{worker_id}"),
        project_key: project_key.to_owned(),
        container_id: format!("container-{worker_id}"),
        worker_secret: format!("secret-{worker_id}"),
        strategy: "default".to_owned(),
        container_status: "provisioning".to_owned(),
        agent_status: "starting".to_owned(),
        workspace_path: Some(format!("/workspace/{worker_id}")),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
        idle_redispatch_count: 0,
    }
}

#[tokio::test]
async fn insert_and_get_slot() {
    let db = TestDb::new().await;
    let r = repo(&db);
    let slot = test_slot("s1", "proj-a");

    r.insert_slot(&slot).await.unwrap();
    let fetched = r.get_slot("s1").await.unwrap().unwrap();

    assert_eq!(fetched.id, "s1");
    assert_eq!(fetched.project_key, "proj-a");
    assert_eq!(fetched.slot_name, "slot-s1");

    db.cleanup().await;
}

#[tokio::test]
async fn get_nonexistent_slot_returns_none() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let fetched = r.get_slot("nonexistent").await.unwrap();
    assert!(fetched.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn list_slots_by_project() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();
    r.insert_slot(&test_slot("s2", "proj-a")).await.unwrap();
    r.insert_slot(&test_slot("s3", "proj-b")).await.unwrap();

    let slots = r.list_slots_by_project("proj-a").await.unwrap();
    assert_eq!(slots.len(), 2);

    let slots_b = r.list_slots_by_project("proj-b").await.unwrap();
    assert_eq!(slots_b.len(), 1);

    db.cleanup().await;
}

#[tokio::test]
async fn slots_in_use_count() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();
    r.insert_slot(&test_slot("s2", "proj-a")).await.unwrap();

    assert_eq!(r.slots_in_use("proj-a").await.unwrap(), 0);

    // Link a running worker to s1.
    let mut w1 = test_worker("w1", "proj-a");
    w1.container_status = "running".to_owned();
    r.insert_worker(&w1).await.unwrap();
    r.link_worker_slot("w1", "s1").await.unwrap();
    assert_eq!(r.slots_in_use("proj-a").await.unwrap(), 1);

    // Link a running worker to s2.
    let mut w2 = test_worker("w2", "proj-a");
    w2.container_status = "running".to_owned();
    r.insert_worker(&w2).await.unwrap();
    r.link_worker_slot("w2", "s2").await.unwrap();
    assert_eq!(r.slots_in_use("proj-a").await.unwrap(), 2);

    db.cleanup().await;
}

#[tokio::test]
async fn delete_slot() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();
    r.delete_slot("s1").await.unwrap();

    let fetched = r.get_slot("s1").await.unwrap();
    assert!(fetched.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn insert_and_get_worker() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();
    let worker = test_worker("a1", "proj-a");

    r.insert_worker(&worker).await.unwrap();
    r.link_worker_slot("a1", "s1").await.unwrap();
    let fetched = r.get_worker("a1").await.unwrap().unwrap();

    assert_eq!(fetched.worker_id, "a1");
    assert_eq!(fetched.process_id, "proc-a1");
    assert_eq!(fetched.project_key, "proj-a");
    assert_eq!(fetched.container_id, "container-a1");
    assert_eq!(fetched.strategy, "default");
    assert_eq!(fetched.container_status, "provisioning");
    assert_eq!(fetched.workspace_path.as_deref(), Some("/workspace/a1"));

    // Verify link exists.
    let link = r.get_worker_slot("a1").await.unwrap().unwrap();
    assert_eq!(link.slot_id, "s1");

    db.cleanup().await;
}

#[tokio::test]
async fn get_nonexistent_worker_returns_none() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let fetched = r.get_worker("nonexistent").await.unwrap();
    assert!(fetched.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn update_worker_container_status() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let worker = test_worker("a1", "proj-a");
    r.insert_worker(&worker).await.unwrap();
    r.update_worker_container_status("a1", "running")
        .await
        .unwrap();

    let fetched = r.get_worker("a1").await.unwrap().unwrap();
    assert_eq!(fetched.container_status, "running");

    db.cleanup().await;
}

#[tokio::test]
async fn list_workers_by_container_status() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_worker(&test_worker("a1", "proj-a")).await.unwrap();
    r.insert_worker(&test_worker("a2", "proj-a")).await.unwrap();
    r.insert_worker(&test_worker("a3", "proj-b")).await.unwrap();

    r.update_worker_container_status("a2", "running")
        .await
        .unwrap();

    let provisioning = r
        .list_workers_by_container_status("provisioning")
        .await
        .unwrap();
    assert_eq!(provisioning.len(), 2);

    let running = r.list_workers_by_container_status("running").await.unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].worker_id, "a2");

    db.cleanup().await;
}

#[tokio::test]
async fn verify_worker_correct_secret() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_worker(&test_worker("a1", "proj-a")).await.unwrap();

    assert!(r.verify_worker("a1", "secret-a1").await.unwrap());
    assert!(!r.verify_worker("a1", "wrong-secret").await.unwrap());
    assert!(!r.verify_worker("nonexistent", "secret-a1").await.unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn get_worker_context() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_worker(&test_worker("a1", "proj-a")).await.unwrap();
    r.insert_worker(&test_worker("a2", "proj-b")).await.unwrap();

    let found = r
        .get_worker_context("proj-a", "/workspace/a1")
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().worker_id, "a1");

    let not_found = r
        .get_worker_context("proj-a", "/workspace/a2")
        .await
        .unwrap();
    assert!(not_found.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn verify_worker_stopped_still_verifies() {
    // verify_worker checks worker_id + secret only, not status.
    let db = TestDb::new().await;
    let r = repo(&db);

    let mut worker = test_worker("a1", "proj-a");
    worker.container_status = "stopped".to_owned();
    r.insert_worker(&worker).await.unwrap();

    assert!(r.verify_worker("a1", "secret-a1").await.unwrap());
    assert!(!r.verify_worker("a1", "wrong").await.unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn get_slot_by_host_path() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();

    let found = r.get_slot_by_host_path("/tmp/s1").await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, "s1");

    let missing = r.get_slot_by_host_path("/tmp/nonexistent").await.unwrap();
    assert!(missing.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn list_all_slots_across_projects() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();
    r.insert_slot(&test_slot("s2", "proj-b")).await.unwrap();
    r.insert_slot(&test_slot("s3", "proj-a")).await.unwrap();

    let all = r.list_all_slots().await.unwrap();
    assert_eq!(all.len(), 3);

    db.cleanup().await;
}

#[tokio::test]
async fn list_active_workers_filters_by_status() {
    let db = TestDb::new().await;
    let r = repo(&db);

    // Insert workers in various statuses.
    let mut a1 = test_worker("a1", "proj-a");
    a1.container_status = "running".to_owned();
    r.insert_worker(&a1).await.unwrap();

    let mut a2 = test_worker("a2", "proj-a");
    a2.container_status = "provisioning".to_owned();
    r.insert_worker(&a2).await.unwrap();

    let mut a3 = test_worker("a3", "proj-a");
    a3.container_status = "stopped".to_owned();
    r.insert_worker(&a3).await.unwrap();

    let mut a4 = test_worker("a4", "proj-a");
    a4.container_status = "stopping".to_owned();
    r.insert_worker(&a4).await.unwrap();

    let active = r.list_active_workers().await.unwrap();
    assert_eq!(active.len(), 3);

    let active_ids: Vec<&str> = active.iter().map(|a| a.worker_id.as_str()).collect();
    assert!(active_ids.contains(&"a1"));
    assert!(active_ids.contains(&"a2"));
    assert!(active_ids.contains(&"a4"));
    assert!(!active_ids.contains(&"a3"));

    db.cleanup().await;
}

#[tokio::test]
async fn delete_workers_by_slot_id() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();

    let a1 = test_worker("a1", "proj-a");
    let a2 = test_worker("a2", "proj-a");
    let a3 = test_worker("a3", "proj-a");
    r.insert_worker(&a1).await.unwrap();
    r.insert_worker(&a2).await.unwrap();
    r.insert_worker(&a3).await.unwrap();

    // Link a1 and a2 to s1, but not a3.
    r.link_worker_slot("a1", "s1").await.unwrap();
    r.link_worker_slot("a2", "s1").await.unwrap();

    let deleted = r.delete_workers_by_slot_id("s1").await.unwrap();
    assert_eq!(deleted, 2);

    // a1 and a2 gone, a3 still present.
    assert!(r.get_worker("a1").await.unwrap().is_none());
    assert!(r.get_worker("a2").await.unwrap().is_none());
    assert!(r.get_worker("a3").await.unwrap().is_some());

    db.cleanup().await;
}

#[tokio::test]
async fn slots_in_use_ignores_stopped_workers() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();

    // Link a stopped worker to s1 — should not count.
    let mut w1 = test_worker("w1", "proj-a");
    w1.container_status = "stopped".to_owned();
    r.insert_worker(&w1).await.unwrap();
    r.link_worker_slot("w1", "s1").await.unwrap();

    assert_eq!(r.slots_in_use("proj-a").await.unwrap(), 0);

    db.cleanup().await;
}

#[tokio::test]
async fn link_and_unlink_worker_slot() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();
    r.insert_worker(&test_worker("a1", "proj-a")).await.unwrap();

    // Link.
    r.link_worker_slot("a1", "s1").await.unwrap();
    let link = r.get_worker_slot("a1").await.unwrap();
    assert!(link.is_some());
    assert_eq!(link.unwrap().slot_id, "s1");

    // Unlink.
    r.unlink_worker_slot("a1").await.unwrap();
    let link = r.get_worker_slot("a1").await.unwrap();
    assert!(link.is_none());

    db.cleanup().await;
}

// --- Reconciliation tests ---

#[tokio::test]
async fn reconcile_slots_deletes_stale_db_rows() {
    let db = TestDb::new().await;
    let r = repo(&db);

    // Create a temp workspace dir with NO slot directories on disk.
    let workspace = tempfile::tempdir().unwrap();
    let pool_dir = workspace.path().join("pool").join("proj-a");
    tokio::fs::create_dir_all(&pool_dir).await.unwrap();

    // Insert a slot in DB that has no on-disk directory.
    let slot = Slot {
        id: "stale-slot".to_owned(),
        project_key: "proj-a".to_owned(),
        slot_name: "0".to_owned(),

        host_path: pool_dir.join("0").display().to_string(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    r.insert_slot(&slot).await.unwrap();

    // Also insert a worker linked to this slot.
    let worker = Worker {
        worker_id: "dead-worker".to_owned(),
        process_id: "proc-dead".to_owned(),
        project_key: "proj-a".to_owned(),
        container_id: "ctr-dead".to_owned(),
        worker_secret: "secret".to_owned(),
        strategy: "default".to_owned(),
        container_status: "stopped".to_owned(),
        agent_status: "starting".to_owned(),
        workspace_path: None,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
        idle_redispatch_count: 0,
    };
    r.insert_worker(&worker).await.unwrap();
    r.link_worker_slot("dead-worker", "stale-slot")
        .await
        .unwrap();

    let mut configs = HashMap::new();
    configs.insert("proj-a".to_owned(), pool_dir.clone());

    let result = r
        .reconcile_slots(&configs, workspace.path(), workspace.path())
        .await
        .unwrap();

    assert_eq!(result.deleted_stale, vec!["stale-slot"]);
    assert!(result.inserted_orphaned.is_empty());

    // Slot and worker should be gone.
    assert!(r.get_slot("stale-slot").await.unwrap().is_none());
    assert!(r.get_worker("dead-worker").await.unwrap().is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_slots_inserts_orphaned_directories() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let workspace = tempfile::tempdir().unwrap();
    let pool_dir = workspace.path().join("pool").join("proj-a");
    // Create two slot directories on disk.
    tokio::fs::create_dir_all(pool_dir.join("0")).await.unwrap();
    tokio::fs::create_dir_all(pool_dir.join("design"))
        .await
        .unwrap();

    let mut configs = HashMap::new();
    configs.insert("proj-a".to_owned(), pool_dir.clone());

    let result = r
        .reconcile_slots(&configs, workspace.path(), workspace.path())
        .await
        .unwrap();

    assert!(result.deleted_stale.is_empty());
    assert_eq!(result.inserted_orphaned.len(), 2);

    // Verify the slots exist in DB.
    let slots = r.list_slots_by_project("proj-a").await.unwrap();
    assert_eq!(slots.len(), 2);

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_slots_mixed_stale_and_orphaned() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let workspace = tempfile::tempdir().unwrap();
    let pool_dir = workspace.path().join("pool").join("proj-a");
    // Only slot "1" exists on disk.
    tokio::fs::create_dir_all(pool_dir.join("1")).await.unwrap();

    // Slot "0" is in DB but not on disk (stale).
    let slot = Slot {
        id: "slot-0".to_owned(),
        project_key: "proj-a".to_owned(),
        slot_name: "0".to_owned(),

        host_path: pool_dir.join("0").display().to_string(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    r.insert_slot(&slot).await.unwrap();

    let mut configs = HashMap::new();
    configs.insert("proj-a".to_owned(), pool_dir.clone());

    let result = r
        .reconcile_slots(&configs, workspace.path(), workspace.path())
        .await
        .unwrap();

    assert_eq!(result.deleted_stale.len(), 1);
    assert_eq!(result.inserted_orphaned.len(), 1);

    // slot-0 gone, slot "1" inserted.
    assert!(r.get_slot("slot-0").await.unwrap().is_none());
    let slots = r.list_slots_by_project("proj-a").await.unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].slot_name, "1");

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_workers_reclaims_live_containers() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let slot = test_slot("s1", "proj-a");
    r.insert_slot(&slot).await.unwrap();

    let mut worker = test_worker("a1", "proj-a");
    worker.container_status = "running".to_owned();
    r.insert_worker(&worker).await.unwrap();
    r.link_worker_slot("a1", "s1").await.unwrap();

    // Container is alive.
    let result = r
        .reconcile_workers(|_container_id| async { true })
        .await
        .unwrap();

    assert_eq!(result.reclaimed, vec!["a1"]);
    assert!(result.marked_stopped.is_empty());

    let fetched = r.get_worker("a1").await.unwrap().unwrap();
    assert_eq!(fetched.container_status, "running");

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_workers_marks_dead_containers_stopped() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let slot = test_slot("s1", "proj-a");
    r.insert_slot(&slot).await.unwrap();

    let mut worker = test_worker("a1", "proj-a");
    worker.container_status = "running".to_owned();
    r.insert_worker(&worker).await.unwrap();
    r.link_worker_slot("a1", "s1").await.unwrap();

    // Container is dead.
    let result = r
        .reconcile_workers(|_container_id| async { false })
        .await
        .unwrap();

    assert!(result.reclaimed.is_empty());
    assert_eq!(result.marked_stopped, vec!["a1"]);

    let fetched = r.get_worker("a1").await.unwrap().unwrap();
    assert_eq!(fetched.container_status, "stopped");

    // Worker-slot link should be removed.
    let link = r.get_worker_slot("a1").await.unwrap();
    assert!(link.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_workers_mixed_live_and_dead() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let s1 = test_slot("s1", "proj-a");
    let s2 = test_slot("s2", "proj-a");
    r.insert_slot(&s1).await.unwrap();
    r.insert_slot(&s2).await.unwrap();

    let mut a1 = test_worker("a1", "proj-a");
    a1.container_status = "running".to_owned();
    a1.container_id = "live-container".to_owned();
    r.insert_worker(&a1).await.unwrap();
    r.link_worker_slot("a1", "s1").await.unwrap();

    let mut a2 = test_worker("a2", "proj-a");
    a2.container_status = "provisioning".to_owned();
    a2.container_id = "dead-container".to_owned();
    r.insert_worker(&a2).await.unwrap();
    r.link_worker_slot("a2", "s2").await.unwrap();

    // Only "live-container" is alive.
    let result = r
        .reconcile_workers(|container_id| async move { container_id == "live-container" })
        .await
        .unwrap();

    assert_eq!(result.reclaimed, vec!["a1"]);
    assert_eq!(result.marked_stopped, vec!["a2"]);

    // a1 still linked, a2 unlinked.
    let link_a1 = r.get_worker_slot("a1").await.unwrap();
    assert!(link_a1.is_some());

    let link_a2 = r.get_worker_slot("a2").await.unwrap();
    assert!(link_a2.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_workers_stopped_dead_is_noop() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let mut worker = test_worker("a1", "proj-a");
    worker.container_status = "stopped".to_owned();
    worker.agent_status = "idle".to_owned();
    r.insert_worker(&worker).await.unwrap();

    // Stopped + dead: no-op.
    let result = r
        .reconcile_workers(|_container_id| async { false })
        .await
        .unwrap();

    assert!(result.reclaimed.is_empty());
    assert!(result.marked_stopped.is_empty());

    // Worker unchanged.
    let fetched = r.get_worker("a1").await.unwrap().unwrap();
    assert_eq!(fetched.container_status, "stopped");
    assert_eq!(fetched.agent_status, "idle");

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_workers_stopped_alive_is_reclaimed() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let mut worker = test_worker("a1", "proj-a");
    worker.container_status = "stopped".to_owned();
    worker.agent_status = "idle".to_owned();
    r.insert_worker(&worker).await.unwrap();

    // Stopped + alive: reclaim (set container_status to "running", preserve agent_status).
    let result = r
        .reconcile_workers(|_container_id| async { true })
        .await
        .unwrap();

    assert_eq!(result.reclaimed, vec!["a1"]);
    assert!(result.marked_stopped.is_empty());

    let fetched = r.get_worker("a1").await.unwrap().unwrap();
    assert_eq!(fetched.container_status, "running");
    // agent_status must NOT be modified during reclaim.
    assert_eq!(fetched.agent_status, "idle");

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_slots_cleans_stale_project_slots() {
    let db = TestDb::new().await;
    let r = repo(&db);

    // Insert a slot for a project that no longer exists in configs.
    let slot = Slot {
        id: "orphan-proj-slot".to_owned(),
        project_key: "deleted-proj".to_owned(),
        slot_name: "0".to_owned(),

        host_path: "/tmp/deleted-proj/0".to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    r.insert_slot(&slot).await.unwrap();

    let workspace = tempfile::tempdir().unwrap();
    // Empty configs -- no projects configured.
    let configs: HashMap<String, PathBuf> = HashMap::new();

    let result = r
        .reconcile_slots(&configs, workspace.path(), workspace.path())
        .await
        .unwrap();

    assert_eq!(result.deleted_stale, vec!["orphan-proj-slot"]);
    assert!(r.get_slot("orphan-proj-slot").await.unwrap().is_none());

    db.cleanup().await;
}
