// Tests for AgentRepo.

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
