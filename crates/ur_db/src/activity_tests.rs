use std::collections::HashMap;

use crate::ticket::CreateTicketParams;
use crate::DatabaseManager;

fn fresh_db() -> DatabaseManager {
    DatabaseManager::create_in_memory().expect("create in-memory db")
}

fn create_task(db: &DatabaseManager) -> String {
    db.create_ticket(
        "ur",
        &CreateTicketParams {
            ticket_type: "task".into(),
            status: "open".into(),
            priority: 1,
            parent_id: None,
            title: "Test Task".into(),
            body: "A task for testing activities.".into(),
        },
    )
    .expect("create task")
}

// === add_activity ===

#[test]
fn add_activity_returns_unique_id() {
    let db = fresh_db();
    let tid = create_task(&db);

    let id1 = db
        .add_activity(&tid, "alice", "Started work", &HashMap::new())
        .unwrap();
    let id2 = db
        .add_activity(&tid, "bob", "Reviewed code", &HashMap::new())
        .unwrap();

    assert_eq!(id1.len(), 8, "activity ID should be 8 chars");
    assert_ne!(id1, id2, "activity IDs should be unique");
}

#[test]
fn add_activity_fails_for_nonexistent_ticket() {
    let db = fresh_db();
    let result = db.add_activity("ur.zzzz", "alice", "hello", &HashMap::new());
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Ticket not found"));
}

#[test]
fn add_activity_with_metadata() {
    let db = fresh_db();
    let tid = create_task(&db);

    let mut meta = HashMap::new();
    meta.insert("old_status".to_string(), "open".to_string());
    meta.insert("new_status".to_string(), "in_progress".to_string());

    let aid = db
        .add_activity(&tid, "alice", "Changed status", &meta)
        .unwrap();
    assert!(!aid.is_empty());

    let activities = db.list_activities(&tid).unwrap();
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].metadata.len(), 2);

    // Metadata is ordered by key
    assert_eq!(activities[0].metadata[0].key, "new_status");
    assert_eq!(activities[0].metadata[0].value, "in_progress");
    assert_eq!(activities[0].metadata[1].key, "old_status");
    assert_eq!(activities[0].metadata[1].value, "open");
}

// === list_activities ===

#[test]
fn list_activities_returns_empty_for_ticket_with_no_activities() {
    let db = fresh_db();
    let tid = create_task(&db);

    let activities = db.list_activities(&tid).unwrap();
    assert!(activities.is_empty());
}

#[test]
fn list_activities_fails_for_nonexistent_ticket() {
    let db = fresh_db();
    let result = db.list_activities("ur.zzzz");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Ticket not found"));
}

#[test]
fn list_activities_ordered_by_timestamp() {
    let db = fresh_db();
    let tid = create_task(&db);

    // Add multiple activities (timestamps will be very close but sequential)
    db.add_activity(&tid, "alice", "First", &HashMap::new())
        .unwrap();
    db.add_activity(&tid, "bob", "Second", &HashMap::new())
        .unwrap();
    db.add_activity(&tid, "charlie", "Third", &HashMap::new())
        .unwrap();

    let activities = db.list_activities(&tid).unwrap();
    assert_eq!(activities.len(), 3);
    assert_eq!(activities[0].entry.message, "First");
    assert_eq!(activities[1].entry.message, "Second");
    assert_eq!(activities[2].entry.message, "Third");
    assert_eq!(activities[0].entry.author, "alice");
    assert_eq!(activities[1].entry.author, "bob");
    assert_eq!(activities[2].entry.author, "charlie");
}

#[test]
fn list_activities_includes_correct_fields() {
    let db = fresh_db();
    let tid = create_task(&db);

    let aid = db
        .add_activity(&tid, "alice", "Did something", &HashMap::new())
        .unwrap();

    let activities = db.list_activities(&tid).unwrap();
    assert_eq!(activities.len(), 1);

    let activity = &activities[0];
    assert_eq!(activity.entry.id, aid);
    assert_eq!(activity.entry.author, "alice");
    assert_eq!(activity.entry.message, "Did something");
    assert!(!activity.entry.timestamp.is_empty());
    assert!(activity.metadata.is_empty());
}

#[test]
fn list_activities_only_returns_activities_for_given_ticket() {
    let db = fresh_db();
    let tid1 = create_task(&db);
    let tid2 = create_task(&db);

    db.add_activity(&tid1, "alice", "On ticket 1", &HashMap::new())
        .unwrap();
    db.add_activity(&tid2, "bob", "On ticket 2", &HashMap::new())
        .unwrap();

    let activities1 = db.list_activities(&tid1).unwrap();
    assert_eq!(activities1.len(), 1);
    assert_eq!(activities1[0].entry.message, "On ticket 1");

    let activities2 = db.list_activities(&tid2).unwrap();
    assert_eq!(activities2.len(), 1);
    assert_eq!(activities2[0].entry.message, "On ticket 2");
}

#[test]
fn add_activity_with_special_characters() {
    let db = fresh_db();
    let tid = create_task(&db);

    let aid = db
        .add_activity(&tid, "o'brien", "It's a test with \\ backslash", &HashMap::new())
        .unwrap();
    assert!(!aid.is_empty());

    let activities = db.list_activities(&tid).unwrap();
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].entry.author, "o'brien");
    assert_eq!(activities[0].entry.message, "It's a test with \\ backslash");
}

#[test]
fn activities_appear_in_get_ticket_detail() {
    let db = fresh_db();
    let tid = create_task(&db);

    db.add_activity(&tid, "alice", "Note one", &HashMap::new())
        .unwrap();
    db.add_activity(&tid, "bob", "Note two", &HashMap::new())
        .unwrap();

    let detail = db.get_ticket(&tid).unwrap();
    assert_eq!(detail.activities.len(), 2);
    assert_eq!(detail.activities[0].message, "Note one");
    assert_eq!(detail.activities[1].message, "Note two");
}
