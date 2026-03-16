// Tests for AgentRepo.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::agent_repo::AgentRepo;
use crate::model::{Agent, Slot};
use crate::tests::TestDb;

fn repo(db: &TestDb) -> AgentRepo {
    AgentRepo::new(db.db().pool().clone())
}

fn test_slot(id: &str, project_key: &str) -> Slot {
    Slot {
        id: id.to_owned(),
        project_key: project_key.to_owned(),
        slot_name: format!("slot-{id}"),
        slot_type: "exclusive".to_owned(),
        host_path: format!("/tmp/{id}"),
        status: "available".to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

fn test_agent(agent_id: &str, project_key: &str, slot_id: Option<&str>) -> Agent {
    Agent {
        agent_id: agent_id.to_owned(),
        process_id: format!("proc-{agent_id}"),
        project_key: project_key.to_owned(),
        slot_id: slot_id.map(|s| s.to_owned()),
        container_id: format!("container-{agent_id}"),
        agent_secret: format!("secret-{agent_id}"),
        strategy: "default".to_owned(),
        status: "provisioning".to_owned(),
        workspace_path: Some(format!("/workspace/{agent_id}")),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
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
    assert_eq!(fetched.slot_type, "exclusive");
    assert_eq!(fetched.status, "available");

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
async fn update_slot_status() {
    let db = TestDb::new().await;
    let r = repo(&db);
    let slot = test_slot("s1", "proj-a");

    r.insert_slot(&slot).await.unwrap();
    r.update_slot_status("s1", "in_use").await.unwrap();

    let fetched = r.get_slot("s1").await.unwrap().unwrap();
    assert_eq!(fetched.status, "in_use");

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
async fn exclusive_slots_in_use_count() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();
    r.insert_slot(&test_slot("s2", "proj-a")).await.unwrap();

    assert_eq!(r.exclusive_slots_in_use("proj-a").await.unwrap(), 0);

    r.update_slot_status("s1", "in_use").await.unwrap();
    assert_eq!(r.exclusive_slots_in_use("proj-a").await.unwrap(), 1);

    r.update_slot_status("s2", "in_use").await.unwrap();
    assert_eq!(r.exclusive_slots_in_use("proj-a").await.unwrap(), 2);

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
async fn insert_and_get_agent() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();
    let agent = test_agent("a1", "proj-a", Some("s1"));

    r.insert_agent(&agent).await.unwrap();
    let fetched = r.get_agent("a1").await.unwrap().unwrap();

    assert_eq!(fetched.agent_id, "a1");
    assert_eq!(fetched.process_id, "proc-a1");
    assert_eq!(fetched.project_key, "proj-a");
    assert_eq!(fetched.slot_id.as_deref(), Some("s1"));
    assert_eq!(fetched.container_id, "container-a1");
    assert_eq!(fetched.strategy, "default");
    assert_eq!(fetched.status, "provisioning");
    assert_eq!(fetched.workspace_path.as_deref(), Some("/workspace/a1"));

    db.cleanup().await;
}

#[tokio::test]
async fn get_nonexistent_agent_returns_none() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let fetched = r.get_agent("nonexistent").await.unwrap();
    assert!(fetched.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn update_agent_status() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let agent = test_agent("a1", "proj-a", None);
    r.insert_agent(&agent).await.unwrap();
    r.update_agent_status("a1", "running").await.unwrap();

    let fetched = r.get_agent("a1").await.unwrap().unwrap();
    assert_eq!(fetched.status, "running");

    db.cleanup().await;
}

#[tokio::test]
async fn list_agents_by_status() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_agent(&test_agent("a1", "proj-a", None))
        .await
        .unwrap();
    r.insert_agent(&test_agent("a2", "proj-a", None))
        .await
        .unwrap();
    r.insert_agent(&test_agent("a3", "proj-b", None))
        .await
        .unwrap();

    r.update_agent_status("a2", "running").await.unwrap();

    let provisioning = r.list_agents_by_status("provisioning").await.unwrap();
    assert_eq!(provisioning.len(), 2);

    let running = r.list_agents_by_status("running").await.unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].agent_id, "a2");

    db.cleanup().await;
}

#[tokio::test]
async fn verify_agent_correct_secret() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_agent(&test_agent("a1", "proj-a", None))
        .await
        .unwrap();

    assert!(r.verify_agent("a1", "secret-a1").await.unwrap());
    assert!(!r.verify_agent("a1", "wrong-secret").await.unwrap());
    assert!(!r.verify_agent("nonexistent", "secret-a1").await.unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn get_agent_context() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_agent(&test_agent("a1", "proj-a", None))
        .await
        .unwrap();
    r.insert_agent(&test_agent("a2", "proj-b", None))
        .await
        .unwrap();

    let found = r
        .get_agent_context("proj-a", "/workspace/a1")
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().agent_id, "a1");

    let not_found = r
        .get_agent_context("proj-a", "/workspace/a2")
        .await
        .unwrap();
    assert!(not_found.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn verify_agent_stopped_still_verifies() {
    // verify_agent checks agent_id + secret only, not status.
    let db = TestDb::new().await;
    let r = repo(&db);

    let mut agent = test_agent("a1", "proj-a", None);
    agent.status = "stopped".to_owned();
    r.insert_agent(&agent).await.unwrap();

    assert!(r.verify_agent("a1", "secret-a1").await.unwrap());
    assert!(!r.verify_agent("a1", "wrong").await.unwrap());

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
async fn list_active_agents_filters_by_status() {
    let db = TestDb::new().await;
    let r = repo(&db);

    // Insert agents in various statuses.
    let mut a1 = test_agent("a1", "proj-a", None);
    a1.status = "running".to_owned();
    r.insert_agent(&a1).await.unwrap();

    let mut a2 = test_agent("a2", "proj-a", None);
    a2.status = "provisioning".to_owned();
    r.insert_agent(&a2).await.unwrap();

    let mut a3 = test_agent("a3", "proj-a", None);
    a3.status = "stopped".to_owned();
    r.insert_agent(&a3).await.unwrap();

    let mut a4 = test_agent("a4", "proj-a", None);
    a4.status = "stopping".to_owned();
    r.insert_agent(&a4).await.unwrap();

    let active = r.list_active_agents().await.unwrap();
    assert_eq!(active.len(), 3);

    let active_ids: Vec<&str> = active.iter().map(|a| a.agent_id.as_str()).collect();
    assert!(active_ids.contains(&"a1"));
    assert!(active_ids.contains(&"a2"));
    assert!(active_ids.contains(&"a4"));
    assert!(!active_ids.contains(&"a3"));

    db.cleanup().await;
}

#[tokio::test]
async fn delete_agents_by_slot_id() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.insert_slot(&test_slot("s1", "proj-a")).await.unwrap();

    let a1 = test_agent("a1", "proj-a", Some("s1"));
    let a2 = test_agent("a2", "proj-a", Some("s1"));
    let a3 = test_agent("a3", "proj-a", None);
    r.insert_agent(&a1).await.unwrap();
    r.insert_agent(&a2).await.unwrap();
    r.insert_agent(&a3).await.unwrap();

    let deleted = r.delete_agents_by_slot_id("s1").await.unwrap();
    assert_eq!(deleted, 2);

    // a1 and a2 gone, a3 still present.
    assert!(r.get_agent("a1").await.unwrap().is_none());
    assert!(r.get_agent("a2").await.unwrap().is_none());
    assert!(r.get_agent("a3").await.unwrap().is_some());

    db.cleanup().await;
}

#[tokio::test]
async fn exclusive_slots_in_use_ignores_shared_slots() {
    let db = TestDb::new().await;
    let r = repo(&db);

    // Insert an exclusive slot and a shared slot, both in_use.
    let mut exclusive = test_slot("s1", "proj-a");
    exclusive.slot_type = "exclusive".to_owned();
    r.insert_slot(&exclusive).await.unwrap();
    r.update_slot_status("s1", "in_use").await.unwrap();

    let mut shared = test_slot("s2", "proj-a");
    shared.slot_type = "shared".to_owned();
    r.insert_slot(&shared).await.unwrap();
    r.update_slot_status("s2", "in_use").await.unwrap();

    // Only the exclusive slot should be counted.
    assert_eq!(r.exclusive_slots_in_use("proj-a").await.unwrap(), 1);

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
        slot_type: "exclusive".to_owned(),
        host_path: pool_dir.join("0").display().to_string(),
        status: "available".to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    r.insert_slot(&slot).await.unwrap();

    // Also insert an agent referencing this slot.
    let agent = Agent {
        agent_id: "dead-agent".to_owned(),
        process_id: "proc-dead".to_owned(),
        project_key: "proj-a".to_owned(),
        slot_id: Some("stale-slot".to_owned()),
        container_id: "ctr-dead".to_owned(),
        agent_secret: "secret".to_owned(),
        strategy: "default".to_owned(),
        status: "stopped".to_owned(),
        workspace_path: None,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    r.insert_agent(&agent).await.unwrap();

    let mut configs = HashMap::new();
    configs.insert("proj-a".to_owned(), pool_dir.clone());

    let result = r.reconcile_slots(&configs, workspace.path()).await.unwrap();

    assert_eq!(result.deleted_stale, vec!["stale-slot"]);
    assert!(result.inserted_orphaned.is_empty());

    // Slot and agent should be gone.
    assert!(r.get_slot("stale-slot").await.unwrap().is_none());
    assert!(r.get_agent("dead-agent").await.unwrap().is_none());

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

    let result = r.reconcile_slots(&configs, workspace.path()).await.unwrap();

    assert!(result.deleted_stale.is_empty());
    assert_eq!(result.inserted_orphaned.len(), 2);

    // Verify the slots exist in DB.
    let slots = r.list_slots_by_project("proj-a").await.unwrap();
    assert_eq!(slots.len(), 2);

    // Check slot types: "0" should be exclusive, "design" should be shared.
    let exclusive = slots.iter().find(|s| s.slot_name == "0").unwrap();
    assert_eq!(exclusive.slot_type, "exclusive");
    assert_eq!(exclusive.status, "available");

    let shared = slots.iter().find(|s| s.slot_name == "design").unwrap();
    assert_eq!(shared.slot_type, "shared");
    assert_eq!(shared.status, "available");

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
        slot_type: "exclusive".to_owned(),
        host_path: pool_dir.join("0").display().to_string(),
        status: "in_use".to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    r.insert_slot(&slot).await.unwrap();

    let mut configs = HashMap::new();
    configs.insert("proj-a".to_owned(), pool_dir.clone());

    let result = r.reconcile_slots(&configs, workspace.path()).await.unwrap();

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
async fn reconcile_agents_reclaims_live_containers() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let slot = test_slot("s1", "proj-a");
    r.insert_slot(&slot).await.unwrap();
    r.update_slot_status("s1", "in_use").await.unwrap();

    let mut agent = test_agent("a1", "proj-a", Some("s1"));
    agent.status = "running".to_owned();
    r.insert_agent(&agent).await.unwrap();

    // Container is alive.
    let result = r
        .reconcile_agents(|_container_id| async { true })
        .await
        .unwrap();

    assert_eq!(result.reclaimed, vec!["a1"]);
    assert!(result.marked_stopped.is_empty());

    let fetched = r.get_agent("a1").await.unwrap().unwrap();
    assert_eq!(fetched.status, "running");

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_agents_marks_dead_containers_stopped() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let slot = test_slot("s1", "proj-a");
    r.insert_slot(&slot).await.unwrap();
    r.update_slot_status("s1", "in_use").await.unwrap();

    let mut agent = test_agent("a1", "proj-a", Some("s1"));
    agent.status = "running".to_owned();
    r.insert_agent(&agent).await.unwrap();

    // Container is dead.
    let result = r
        .reconcile_agents(|_container_id| async { false })
        .await
        .unwrap();

    assert!(result.reclaimed.is_empty());
    assert_eq!(result.marked_stopped, vec!["a1"]);

    let fetched = r.get_agent("a1").await.unwrap().unwrap();
    assert_eq!(fetched.status, "stopped");

    // Slot should be released back to available.
    let slot = r.get_slot("s1").await.unwrap().unwrap();
    assert_eq!(slot.status, "available");

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_agents_mixed_live_and_dead() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let s1 = test_slot("s1", "proj-a");
    let s2 = test_slot("s2", "proj-a");
    r.insert_slot(&s1).await.unwrap();
    r.insert_slot(&s2).await.unwrap();
    r.update_slot_status("s1", "in_use").await.unwrap();
    r.update_slot_status("s2", "in_use").await.unwrap();

    let mut a1 = test_agent("a1", "proj-a", Some("s1"));
    a1.status = "running".to_owned();
    a1.container_id = "live-container".to_owned();
    r.insert_agent(&a1).await.unwrap();

    let mut a2 = test_agent("a2", "proj-a", Some("s2"));
    a2.status = "provisioning".to_owned();
    a2.container_id = "dead-container".to_owned();
    r.insert_agent(&a2).await.unwrap();

    // Only "live-container" is alive.
    let result = r
        .reconcile_agents(|container_id| async move { container_id == "live-container" })
        .await
        .unwrap();

    assert_eq!(result.reclaimed, vec!["a1"]);
    assert_eq!(result.marked_stopped, vec!["a2"]);

    // s1 stays in_use (agent reclaimed, not released), s2 released.
    // Actually, reconcile_agents only releases slots for dead agents. For live ones,
    // it just updates status to running — the slot stays as-is.
    let s1_fetched = r.get_slot("s1").await.unwrap().unwrap();
    assert_eq!(s1_fetched.status, "in_use");

    let s2_fetched = r.get_slot("s2").await.unwrap().unwrap();
    assert_eq!(s2_fetched.status, "available");

    db.cleanup().await;
}

#[tokio::test]
async fn reconcile_agents_skips_already_stopped() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let mut agent = test_agent("a1", "proj-a", None);
    agent.status = "stopped".to_owned();
    r.insert_agent(&agent).await.unwrap();

    // Nothing should happen — stopped agents are not active.
    let result = r
        .reconcile_agents(|_container_id| async { false })
        .await
        .unwrap();

    assert!(result.reclaimed.is_empty());
    assert!(result.marked_stopped.is_empty());

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
        slot_type: "exclusive".to_owned(),
        host_path: "/tmp/deleted-proj/0".to_owned(),
        status: "available".to_owned(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    r.insert_slot(&slot).await.unwrap();

    let workspace = tempfile::tempdir().unwrap();
    // Empty configs — no projects configured.
    let configs: HashMap<String, PathBuf> = HashMap::new();

    let result = r.reconcile_slots(&configs, workspace.path()).await.unwrap();

    assert_eq!(result.deleted_stale, vec!["orphan-proj-slot"]);
    assert!(r.get_slot("orphan-proj-slot").await.unwrap().is_none());

    db.cleanup().await;
}
