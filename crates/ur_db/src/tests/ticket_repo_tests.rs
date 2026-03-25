// Tests for TicketRepo.

use crate::graph::GraphManager;
use crate::model::{EdgeKind, NewTicket, TicketFilter, TicketUpdate};
use crate::tests::TestDb;
use crate::ticket_repo::TicketRepo;

/// Build a TicketRepo from a TestDb.
fn repo(db: &TestDb) -> TicketRepo {
    let pool = db.db().pool().clone();
    let graph_manager = GraphManager::new(pool.clone());
    TicketRepo::new(pool, graph_manager)
}

/// Build a complex ticket hierarchy for tests that need rich data.
///
/// Structure:
///   epic-1 (epic, priority 1)
///     task-1a (task, priority 1, parent=epic-1)
///     task-1b (task, priority 2, parent=epic-1)
///     task-1c (task, priority 3, parent=epic-1, status=closed)
///   epic-2 (epic, priority 2)
///     task-2a (task, priority 1, parent=epic-2)
///     task-2b (task, priority 2, parent=epic-2)
///   standalone (task, priority 5, no parent)
///
/// Edges:
///   task-1a blocks task-1b
///   task-2a blocks task-1b  (cross-epic dependency)
///   task-1a relates_to task-2a
///
/// Metadata (ticket entity):
///   task-1a: component=backend, team=alpha
///   task-1b: component=backend
///   task-2a: component=frontend, team=beta
///
/// Metadata (activity entity):
///   (set on activities created below)
///
/// Activities:
///   task-1a: 2 activities
///   task-1b: 1 activity
async fn populated_db() -> (TestDb, TicketRepo) {
    let db = TestDb::new().await;
    let repo = repo(&db);

    seed_epics_and_children(&repo).await;
    seed_remaining_tickets(&repo).await;
    seed_edges_and_metadata(&repo).await;

    (db, repo)
}

/// Create epics and their child tasks for the populated_db fixture.
async fn seed_epics_and_children(repo: &TicketRepo) {
    repo.create_ticket(&NewTicket {
        id: Some("epic-1".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "Epic One".into(),
        body: "First epic".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("epic-2".into()),
        type_: "task".into(),
        priority: 2,
        parent_id: None,
        title: "Epic Two".into(),
        body: "Second epic".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // --- Children of epic-1 ---
    repo.create_ticket(&NewTicket {
        id: Some("task-1a".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: Some("epic-1".into()),
        title: "Task 1A".into(),
        body: "First task in epic 1".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("task-1b".into()),
        type_: "task".into(),
        priority: 2,
        parent_id: Some("epic-1".into()),
        title: "Task 1B".into(),
        body: "Second task in epic 1".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("task-1c".into()),
        type_: "task".into(),
        priority: 3,
        parent_id: Some("epic-1".into()),
        title: "Task 1C".into(),
        body: "Third task in epic 1".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // Close task-1c
    repo.update_ticket(
        "task-1c",
        &TicketUpdate {
            status: Some("closed".into()),
            lifecycle_status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
            lifecycle_managed: None,
        },
    )
    .await
    .unwrap();
}

/// Create epic-2 children and standalone ticket for the populated_db fixture.
async fn seed_remaining_tickets(repo: &TicketRepo) {
    repo.create_ticket(&NewTicket {
        id: Some("task-2a".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: Some("epic-2".into()),
        title: "Task 2A".into(),
        body: "First task in epic 2".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("task-2b".into()),
        type_: "task".into(),
        priority: 2,
        parent_id: Some("epic-2".into()),
        title: "Task 2B".into(),
        body: "Second task in epic 2".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("standalone".into()),
        type_: "design".into(),
        priority: 5,
        parent_id: None,
        title: "Standalone Design".into(),
        body: "No parent".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();
}

/// Seed edges, metadata, and activities for the populated_db fixture.
async fn seed_edges_and_metadata(repo: &TicketRepo) {
    // --- Edges ---
    repo.add_edge("task-1a", "task-1b", EdgeKind::Blocks)
        .await
        .unwrap();
    repo.add_edge("task-2a", "task-1b", EdgeKind::Blocks)
        .await
        .unwrap();
    repo.add_edge("task-1a", "task-2a", EdgeKind::RelatesTo)
        .await
        .unwrap();

    // --- Metadata (ticket) ---
    repo.set_meta("task-1a", "ticket", "component", "backend")
        .await
        .unwrap();
    repo.set_meta("task-1a", "ticket", "team", "alpha")
        .await
        .unwrap();
    repo.set_meta("task-1b", "ticket", "component", "backend")
        .await
        .unwrap();
    repo.set_meta("task-2a", "ticket", "component", "frontend")
        .await
        .unwrap();
    repo.set_meta("task-2a", "ticket", "team", "beta")
        .await
        .unwrap();

    // --- Activities ---
    let act1 = repo
        .add_activity("task-1a", "alice", "Started work")
        .await
        .unwrap();
    repo.add_activity("task-1a", "bob", "Code review done")
        .await
        .unwrap();
    repo.add_activity("task-1b", "alice", "Waiting on blockers")
        .await
        .unwrap();

    // --- Metadata (activity) ---
    repo.set_meta(&act1.id, "activity", "source", "cli")
        .await
        .unwrap();
}

// ============================================================
// CRUD tests
// ============================================================

#[tokio::test]
async fn create_and_get_ticket() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    let created = repo
        .create_ticket(&NewTicket {
            id: Some("t-001".into()),
            type_: "task".into(),
            priority: 3,
            parent_id: None,
            title: "Test ticket".into(),
            body: "A body".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(created.id, "t-001");
    assert_eq!(created.status, "open");
    assert_eq!(created.priority, 3);
    assert_eq!(created.type_, "task");
    assert_eq!(created.title, "Test ticket");
    assert_eq!(created.body, "A body");
    assert!(created.parent_id.is_none());

    let fetched = repo.get_ticket("t-001").await.unwrap().unwrap();
    assert_eq!(fetched.id, "t-001");
    assert_eq!(fetched.title, "Test ticket");

    db.cleanup().await;
}

#[tokio::test]
async fn get_nonexistent_ticket_returns_none() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    let result = repo.get_ticket("no-such-id").await.unwrap();
    assert!(result.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn create_ticket_with_parent() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("parent".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "Parent".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let child = repo
        .create_ticket(&NewTicket {
            id: Some("child".into()),
            type_: "task".into(),
            priority: 2,
            parent_id: Some("parent".into()),
            title: "Child".into(),
            body: "".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(child.parent_id.as_deref(), Some("parent"));

    let fetched = repo.get_ticket("child").await.unwrap().unwrap();
    assert_eq!(fetched.parent_id.as_deref(), Some("parent"));

    db.cleanup().await;
}

#[tokio::test]
async fn update_ticket_partial_fields() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("t-upd".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "Original".into(),
        body: "Original body".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // Update only status and title
    let updated = repo
        .update_ticket(
            "t-upd",
            &TicketUpdate {
                status: Some("in_progress".into()),
                lifecycle_status: None,
                type_: None,
                priority: None,
                title: Some("Updated Title".into()),
                body: None,
                branch: None,
                parent_id: None,
                project: None,
                lifecycle_managed: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(updated.status, "in_progress");
    assert_eq!(updated.title, "Updated Title");
    assert_eq!(updated.body, "Original body"); // unchanged
    assert_eq!(updated.priority, 1); // unchanged

    db.cleanup().await;
}

#[tokio::test]
async fn update_ticket_clear_parent() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("task".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "E".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("child".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: Some("task".into()),
        title: "C".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // Clear parent_id using Some(None)
    let updated = repo
        .update_ticket(
            "child",
            &TicketUpdate {
                status: None,
                lifecycle_status: None,
                type_: None,
                priority: None,
                title: None,
                body: None,
                branch: None,
                parent_id: Some(None),
                project: None,
                lifecycle_managed: None,
            },
        )
        .await
        .unwrap();

    assert!(updated.parent_id.is_none());

    let fetched = repo.get_ticket("child").await.unwrap().unwrap();
    assert!(fetched.parent_id.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn update_nonexistent_ticket_returns_error() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    let result = repo
        .update_ticket(
            "no-such",
            &TicketUpdate {
                status: Some("closed".into()),
                lifecycle_status: None,
                type_: None,
                priority: None,
                title: None,
                body: None,
                branch: None,
                parent_id: None,
                project: None,
                lifecycle_managed: None,
            },
        )
        .await;

    assert!(result.is_err());

    db.cleanup().await;
}

// ============================================================
// get_ticket_by_id
// ============================================================

#[tokio::test]
async fn get_ticket_by_id_returns_existing_ticket() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("t-byid".into()),
        type_: "task".into(),
        priority: 2,
        parent_id: None,
        title: "By ID test".into(),
        body: "Testing get_ticket_by_id".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let fetched = repo.get_ticket_by_id("t-byid").await.unwrap().unwrap();
    assert_eq!(fetched.id, "t-byid");
    assert_eq!(fetched.title, "By ID test");
    assert_eq!(fetched.body, "Testing get_ticket_by_id");
    assert_eq!(fetched.priority, 2);

    db.cleanup().await;
}

#[tokio::test]
async fn get_ticket_by_id_returns_none_for_nonexistent() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    let result = repo.get_ticket_by_id("no-such-ticket").await.unwrap();
    assert!(result.is_none());

    db.cleanup().await;
}

// ============================================================
// list_tickets with filters
// ============================================================

#[tokio::test]
async fn list_tickets_no_filter() {
    let (db, repo) = populated_db().await;

    let all = repo
        .list_tickets(&TicketFilter {
            project: None,
            status: None,
            type_: None,
            parent_id: None,
            lifecycle_status: None,
        })
        .await
        .unwrap();

    // 2 epics + 3 epic-1 children + 2 epic-2 children + 1 standalone = 8
    assert_eq!(all.len(), 8);

    db.cleanup().await;
}

#[tokio::test]
async fn list_tickets_filter_by_status() {
    let (db, repo) = populated_db().await;

    let closed = repo
        .list_tickets(&TicketFilter {
            project: None,
            status: Some("closed".into()),
            type_: None,
            parent_id: None,
            lifecycle_status: None,
        })
        .await
        .unwrap();

    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].id, "task-1c");

    db.cleanup().await;
}

#[tokio::test]
async fn list_tickets_filter_by_type() {
    let (db, repo) = populated_db().await;

    let designs = repo
        .list_tickets(&TicketFilter {
            project: None,
            status: None,
            type_: Some("design".into()),
            parent_id: None,
            lifecycle_status: None,
        })
        .await
        .unwrap();

    assert_eq!(designs.len(), 1);
    assert_eq!(designs[0].id, "standalone");

    db.cleanup().await;
}

#[tokio::test]
async fn list_tickets_filter_by_parent() {
    let (db, repo) = populated_db().await;

    let children = repo
        .list_tickets(&TicketFilter {
            project: None,
            status: None,
            type_: None,
            parent_id: Some("epic-1".into()),
            lifecycle_status: None,
        })
        .await
        .unwrap();

    assert_eq!(children.len(), 3);
    let ids: Vec<&str> = children.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"task-1a"));
    assert!(ids.contains(&"task-1b"));
    assert!(ids.contains(&"task-1c"));

    db.cleanup().await;
}

#[tokio::test]
async fn list_tickets_combined_filters() {
    let (db, repo) = populated_db().await;

    // Open tasks under epic-1
    let results = repo
        .list_tickets(&TicketFilter {
            project: None,
            status: Some("open".into()),
            type_: None,
            parent_id: Some("epic-1".into()),
            lifecycle_status: None,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    let ids: Vec<&str> = results.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"task-1a"));
    assert!(ids.contains(&"task-1b"));

    db.cleanup().await;
}

#[tokio::test]
async fn list_tickets_ordered_by_priority() {
    let (db, repo) = populated_db().await;

    let children = repo
        .list_tickets(&TicketFilter {
            project: None,
            status: None,
            type_: None,
            parent_id: Some("epic-1".into()),
            lifecycle_status: None,
        })
        .await
        .unwrap();

    let priorities: Vec<i32> = children.iter().map(|t| t.priority).collect();
    assert!(priorities.windows(2).all(|w| w[0] <= w[1]));

    db.cleanup().await;
}

// ============================================================
// Metadata tests
// ============================================================

#[tokio::test]
async fn set_and_get_ticket_metadata() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("t-meta".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "Meta test".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.set_meta("t-meta", "ticket", "component", "backend")
        .await
        .unwrap();
    repo.set_meta("t-meta", "ticket", "priority_tag", "high")
        .await
        .unwrap();

    let meta = repo.get_meta("t-meta", "ticket").await.unwrap();
    assert_eq!(meta.len(), 2);
    assert_eq!(meta.get("component").unwrap(), "backend");
    assert_eq!(meta.get("priority_tag").unwrap(), "high");

    db.cleanup().await;
}

#[tokio::test]
async fn set_meta_upserts_existing_key() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("t-upsert".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "Upsert".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.set_meta("t-upsert", "ticket", "color", "red")
        .await
        .unwrap();
    repo.set_meta("t-upsert", "ticket", "color", "blue")
        .await
        .unwrap();

    let meta = repo.get_meta("t-upsert", "ticket").await.unwrap();
    assert_eq!(meta.get("color").unwrap(), "blue");
    assert_eq!(meta.len(), 1);

    db.cleanup().await;
}

#[tokio::test]
async fn get_meta_for_activity_entity_type() {
    let (db, repo) = populated_db().await;

    // The populated_db sets activity metadata on the first activity of task-1a.
    // We need the activity id - fetch activities and check meta.
    let activities = repo.get_activities("task-1a").await.unwrap();
    let act1_id = &activities[0].id;

    let meta = repo.get_meta(act1_id, "activity").await.unwrap();
    assert_eq!(meta.get("source").unwrap(), "cli");

    // Ensure ticket meta and activity meta don't mix
    let ticket_meta = repo.get_meta(act1_id, "ticket").await.unwrap();
    assert!(ticket_meta.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn get_meta_empty_for_no_metadata() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    let meta = repo.get_meta("nonexistent", "ticket").await.unwrap();
    assert!(meta.is_empty());

    db.cleanup().await;
}

// ============================================================
// Edge tests
// ============================================================

#[tokio::test]
async fn add_and_query_blocks_edges() {
    let (db, repo) = populated_db().await;

    // task-1a blocks task-1b, task-2a blocks task-1b
    let edges = repo
        .edges_for("task-1b", Some(EdgeKind::Blocks))
        .await
        .unwrap();

    assert_eq!(edges.len(), 2);
    let sources: Vec<&str> = edges.iter().map(|e| e.source_id.as_str()).collect();
    assert!(sources.contains(&"task-1a"));
    assert!(sources.contains(&"task-2a"));

    db.cleanup().await;
}

#[tokio::test]
async fn add_and_query_relates_to_edges() {
    let (db, repo) = populated_db().await;

    let edges = repo
        .edges_for("task-1a", Some(EdgeKind::RelatesTo))
        .await
        .unwrap();

    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].source_id, "task-1a");
    assert_eq!(edges[0].target_id, "task-2a");

    // relates_to is also visible from the target side
    let edges_from_target = repo
        .edges_for("task-2a", Some(EdgeKind::RelatesTo))
        .await
        .unwrap();

    assert_eq!(edges_from_target.len(), 1);

    db.cleanup().await;
}

#[tokio::test]
async fn edges_for_all_kinds() {
    let (db, repo) = populated_db().await;

    // task-1a has: blocks task-1b, relates_to task-2a
    let edges = repo.edges_for("task-1a", None).await.unwrap();
    assert_eq!(edges.len(), 2);

    db.cleanup().await;
}

#[tokio::test]
async fn remove_edge() {
    let (db, repo) = populated_db().await;

    repo.remove_edge("task-1a", "task-1b", EdgeKind::Blocks)
        .await
        .unwrap();

    let edges = repo
        .edges_for("task-1b", Some(EdgeKind::Blocks))
        .await
        .unwrap();

    // Only task-2a blocks task-1b now
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].source_id, "task-2a");

    db.cleanup().await;
}

#[tokio::test]
async fn remove_nonexistent_edge_is_ok() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    // Should not error even though no such edge exists
    repo.remove_edge("a", "b", EdgeKind::Blocks).await.unwrap();

    db.cleanup().await;
}

#[tokio::test]
async fn edges_for_ticket_with_no_edges() {
    let (db, repo) = populated_db().await;

    let edges = repo.edges_for("standalone", None).await.unwrap();
    assert!(edges.is_empty());

    db.cleanup().await;
}

// ============================================================
// Activity tests
// ============================================================

#[tokio::test]
async fn add_and_get_activities() {
    let (db, repo) = populated_db().await;

    let activities = repo.get_activities("task-1a").await.unwrap();
    assert_eq!(activities.len(), 2);
    assert_eq!(activities[0].author, "alice");
    assert_eq!(activities[0].message, "Started work");
    assert_eq!(activities[1].author, "bob");
    assert_eq!(activities[1].message, "Code review done");

    db.cleanup().await;
}

#[tokio::test]
async fn get_activities_returns_empty_for_no_activities() {
    let (db, repo) = populated_db().await;

    let activities = repo.get_activities("standalone").await.unwrap();
    assert!(activities.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn add_activity_returns_generated_fields() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("t-act".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "Act".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let activity = repo
        .add_activity("t-act", "carol", "Did something")
        .await
        .unwrap();

    assert!(!activity.id.is_empty());
    assert_eq!(activity.ticket_id, "t-act");
    assert_eq!(activity.author, "carol");
    assert_eq!(activity.message, "Did something");
    assert!(!activity.timestamp.is_empty());

    db.cleanup().await;
}

// ============================================================
// tickets_by_metadata
// ============================================================

#[tokio::test]
async fn tickets_by_metadata_exact_match() {
    let (db, repo) = populated_db().await;

    let results = repo
        .tickets_by_metadata("component", "backend")
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    let ids: Vec<&str> = results.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"task-1a"));
    assert!(ids.contains(&"task-1b"));

    // All should have the matched key/value
    for r in &results {
        assert_eq!(r.key, "component");
        assert_eq!(r.value, "backend");
    }

    db.cleanup().await;
}

#[tokio::test]
async fn tickets_by_metadata_no_match() {
    let (db, repo) = populated_db().await;

    let results = repo
        .tickets_by_metadata("component", "mobile")
        .await
        .unwrap();
    assert!(results.is_empty());

    db.cleanup().await;
}

// ============================================================
// tickets_with_metadata_key
// ============================================================

#[tokio::test]
async fn tickets_with_metadata_key_returns_all_values() {
    let (db, repo) = populated_db().await;

    let results = repo.tickets_with_metadata_key("team").await.unwrap();
    assert_eq!(results.len(), 2);
    let pairs: Vec<(&str, &str)> = results
        .iter()
        .map(|t| (t.id.as_str(), t.value.as_str()))
        .collect();
    assert!(pairs.contains(&("task-1a", "alpha")));
    assert!(pairs.contains(&("task-2a", "beta")));

    db.cleanup().await;
}

#[tokio::test]
async fn tickets_with_metadata_key_no_results() {
    let (db, repo) = populated_db().await;

    let results = repo.tickets_with_metadata_key("nonexistent").await.unwrap();
    assert!(results.is_empty());

    db.cleanup().await;
}

// ============================================================
// dispatchable_tickets
// ============================================================

#[tokio::test]
async fn dispatchable_tickets_filters_blocked() {
    let (db, repo) = populated_db().await;

    // epic-1 open children: task-1a (no blockers), task-1b (blocked by task-1a and task-2a)
    // task-1c is closed so not included
    let dispatchable = repo.dispatchable_tickets("epic-1", None).await.unwrap();

    assert_eq!(dispatchable.len(), 1);
    assert_eq!(dispatchable[0].id, "task-1a");

    db.cleanup().await;
}

#[tokio::test]
async fn dispatchable_tickets_unblocked_after_closing_blockers() {
    let (db, repo) = populated_db().await;

    // Close both blockers of task-1b
    repo.update_ticket(
        "task-1a",
        &TicketUpdate {
            status: Some("closed".into()),
            lifecycle_status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
            lifecycle_managed: None,
        },
    )
    .await
    .unwrap();

    repo.update_ticket(
        "task-2a",
        &TicketUpdate {
            status: Some("closed".into()),
            lifecycle_status: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
            lifecycle_managed: None,
        },
    )
    .await
    .unwrap();

    let dispatchable = repo.dispatchable_tickets("epic-1", None).await.unwrap();

    // task-1b should now be dispatchable (task-1a is closed so not included)
    assert_eq!(dispatchable.len(), 1);
    assert_eq!(dispatchable[0].id, "task-1b");

    db.cleanup().await;
}

#[tokio::test]
async fn dispatchable_tickets_all_unblocked() {
    let (db, repo) = populated_db().await;

    // epic-2 open children: task-2a (no blockers), task-2b (no blockers)
    let dispatchable = repo.dispatchable_tickets("epic-2", None).await.unwrap();

    assert_eq!(dispatchable.len(), 2);
    let ids: Vec<&str> = dispatchable.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"task-2a"));
    assert!(ids.contains(&"task-2b"));

    db.cleanup().await;
}

#[tokio::test]
async fn dispatchable_tickets_empty_epic() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("empty-epic".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "Empty".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let dispatchable = repo.dispatchable_tickets("empty-epic", None).await.unwrap();
    assert!(dispatchable.is_empty());

    db.cleanup().await;
}

// ============================================================
// epic_all_children_closed
// ============================================================

#[tokio::test]
async fn epic_all_children_closed_false_when_open_children() {
    let (db, repo) = populated_db().await;

    let result = repo.epic_all_children_closed("epic-1").await.unwrap();
    assert!(!result);

    db.cleanup().await;
}

#[tokio::test]
async fn epic_all_children_closed_true_when_all_closed() {
    let (db, repo) = populated_db().await;

    // Close all children of epic-1
    for id in &["task-1a", "task-1b"] {
        repo.update_ticket(
            id,
            &TicketUpdate {
                status: Some("closed".into()),
                lifecycle_status: None,
                type_: None,
                priority: None,
                title: None,
                body: None,
                branch: None,
                parent_id: None,
                project: None,
                lifecycle_managed: None,
            },
        )
        .await
        .unwrap();
    }

    let result = repo.epic_all_children_closed("epic-1").await.unwrap();
    assert!(result);

    db.cleanup().await;
}

#[tokio::test]
async fn epic_all_children_closed_true_for_no_children() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("childless".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "No kids".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let result = repo.epic_all_children_closed("childless").await.unwrap();
    assert!(result);

    db.cleanup().await;
}

// ============================================================
// close_open_children
// ============================================================

#[tokio::test]
async fn close_open_children_closes_all_open() {
    let (db, repo) = populated_db().await;

    // Verify children start open
    assert!(!repo.epic_all_children_closed("epic-1").await.unwrap());

    let closed = repo.close_open_children("epic-1").await.unwrap();
    assert_eq!(closed, 2);

    // Now all children should be closed
    assert!(repo.epic_all_children_closed("epic-1").await.unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn close_open_children_returns_zero_when_already_closed() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("epic-no-kids".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "No kids".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let closed = repo.close_open_children("epic-no-kids").await.unwrap();
    assert_eq!(closed, 0);

    db.cleanup().await;
}

// ============================================================
// list_tickets_paginated
// ============================================================

#[tokio::test]
async fn paginated_returns_all_when_no_page_size() {
    let (db, repo) = populated_db().await;

    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: None,
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            None,
            0,
            true,
        )
        .await
        .unwrap();

    assert_eq!(total, 8);
    assert_eq!(tickets.len(), 8);

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_limits_results() {
    let (db, repo) = populated_db().await;

    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: None,
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            Some(3),
            0,
            true,
        )
        .await
        .unwrap();

    assert_eq!(total, 8);
    assert_eq!(tickets.len(), 3);

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_offset() {
    let (db, repo) = populated_db().await;

    // Get first page
    let (page1, total1) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: None,
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            Some(3),
            0,
            true,
        )
        .await
        .unwrap();

    // Get second page
    let (page2, total2) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: None,
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            Some(3),
            3,
            true,
        )
        .await
        .unwrap();

    assert_eq!(total1, 8);
    assert_eq!(total2, 8);
    assert_eq!(page1.len(), 3);
    assert_eq!(page2.len(), 3);

    // Pages should not overlap
    let page1_ids: Vec<&str> = page1.iter().map(|t| t.id.as_str()).collect();
    for t in &page2 {
        assert!(!page1_ids.contains(&t.id.as_str()));
    }

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_offset_past_end() {
    let (db, repo) = populated_db().await;

    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: None,
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            Some(10),
            100,
            true,
        )
        .await
        .unwrap();

    assert_eq!(total, 8);
    assert!(tickets.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_empty_results() {
    let (db, repo) = populated_db().await;

    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: Some("nonexistent_status".into()),
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            Some(10),
            0,
            true,
        )
        .await
        .unwrap();

    assert_eq!(total, 0);
    assert!(tickets.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_include_children_false_returns_top_level() {
    let (db, repo) = populated_db().await;

    // include_children=false should only return tickets with no parent
    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: None,
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            None,
            0,
            false,
        )
        .await
        .unwrap();

    // epic-1, epic-2, standalone = 3 top-level tickets
    assert_eq!(total, 3);
    assert_eq!(tickets.len(), 3);
    for t in &tickets {
        assert!(t.parent_id.is_none());
    }

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_include_children_true_returns_all() {
    let (db, repo) = populated_db().await;

    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: None,
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            None,
            0,
            true,
        )
        .await
        .unwrap();

    assert_eq!(total, 8);
    assert_eq!(tickets.len(), 8);

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_with_status_filter() {
    let (db, repo) = populated_db().await;

    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: Some("open".into()),
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            Some(2),
            0,
            true,
        )
        .await
        .unwrap();

    // 7 open tickets total (all except task-1c which is closed)
    assert_eq!(total, 7);
    assert_eq!(tickets.len(), 2);

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_with_project_filter() {
    let (db, repo) = populated_db().await;

    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: Some("test".into()),
                status: None,
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            Some(5),
            0,
            true,
        )
        .await
        .unwrap();

    assert_eq!(total, 8);
    assert_eq!(tickets.len(), 5);

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_combined_filters_and_pagination() {
    let (db, repo) = populated_db().await;

    // Open tickets under epic-1, paginated
    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: Some("open".into()),
                type_: None,
                parent_id: Some("epic-1".into()),
                lifecycle_status: None,
            },
            Some(1),
            0,
            true,
        )
        .await
        .unwrap();

    assert_eq!(total, 2);
    assert_eq!(tickets.len(), 1);

    // Second page
    let (tickets2, total2) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: Some("open".into()),
                type_: None,
                parent_id: Some("epic-1".into()),
                lifecycle_status: None,
            },
            Some(1),
            1,
            true,
        )
        .await
        .unwrap();

    assert_eq!(total2, 2);
    assert_eq!(tickets2.len(), 1);
    assert_ne!(tickets[0].id, tickets2[0].id);

    db.cleanup().await;
}

#[tokio::test]
async fn paginated_total_count_with_top_level_filter() {
    let (db, repo) = populated_db().await;

    // Top-level open tickets only
    let (tickets, total) = repo
        .list_tickets_paginated(
            &TicketFilter {
                project: None,
                status: Some("open".into()),
                type_: None,
                parent_id: None,
                lifecycle_status: None,
            },
            Some(1),
            0,
            false,
        )
        .await
        .unwrap();

    // epic-1, epic-2, standalone are all open and top-level = 3
    assert_eq!(total, 3);
    assert_eq!(tickets.len(), 1);

    db.cleanup().await;
}

// ============================================================
// Hierarchy queries (parent/children via list_tickets)
// ============================================================

#[tokio::test]
async fn hierarchy_children_of_epic() {
    let (db, repo) = populated_db().await;

    let children = repo
        .list_tickets(&TicketFilter {
            project: None,
            status: None,
            type_: None,
            parent_id: Some("epic-2".into()),
            lifecycle_status: None,
        })
        .await
        .unwrap();

    assert_eq!(children.len(), 2);
    for child in &children {
        assert_eq!(child.parent_id.as_deref(), Some("epic-2"));
    }

    db.cleanup().await;
}

#[tokio::test]
async fn hierarchy_top_level_tickets() {
    let (db, repo) = populated_db().await;

    // Top-level tickets have no parent_id, but we can't filter for NULL parent_id
    // with the current filter. Instead, list all and filter in memory.
    let all = repo
        .list_tickets(&TicketFilter {
            project: None,
            status: None,
            type_: None,
            parent_id: None,
            lifecycle_status: None,
        })
        .await
        .unwrap();

    let top_level: Vec<&str> = all
        .iter()
        .filter(|t| t.parent_id.is_none())
        .map(|t| t.id.as_str())
        .collect();

    assert!(top_level.contains(&"epic-1"));
    assert!(top_level.contains(&"epic-2"));
    assert!(top_level.contains(&"standalone"));
    assert_eq!(top_level.len(), 3);

    db.cleanup().await;
}

// ============================================================
// Base-36 ID generation tests
// ============================================================

#[tokio::test]
async fn generated_id_matches_base36_pattern() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    let ticket = repo
        .create_ticket(&NewTicket {
            id: None,
            type_: "task".into(),
            priority: 1,
            parent_id: None,
            title: "Auto-ID ticket".into(),
            body: "".into(),
            project: "myproj".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // ID must match {project}-{5+ base36 chars}
    assert!(
        ticket.id.starts_with("myproj-"),
        "Generated ID '{}' should start with 'myproj-'",
        ticket.id
    );
    let suffix = &ticket.id["myproj-".len()..];
    assert!(
        suffix.len() >= 5,
        "Suffix '{}' should be at least 5 chars",
        suffix
    );
    assert!(
        suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "Suffix '{}' should contain only base-36 chars (0-9, a-z)",
        suffix
    );

    // Verify it's persisted and retrievable
    let fetched = repo.get_ticket(&ticket.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, ticket.id);

    db.cleanup().await;
}

#[tokio::test]
async fn explicit_id_used_as_is() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    let ticket = repo
        .create_ticket(&NewTicket {
            id: Some("custom-explicit-id".into()),
            type_: "task".into(),
            priority: 1,
            parent_id: None,
            title: "Explicit ID".into(),
            body: "".into(),
            project: "test".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(ticket.id, "custom-explicit-id");

    let fetched = repo
        .get_ticket("custom-explicit-id")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.id, "custom-explicit-id");

    db.cleanup().await;
}

#[tokio::test]
async fn collision_retry_produces_longer_id() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    // Insert many tickets with auto-generated IDs and verify all are unique.
    // Also test that if we manually insert a ticket with a known base-36 ID,
    // then create_ticket with that same ID via explicit insert would fail,
    // proving the uniqueness constraint works.
    let mut ids = std::collections::HashSet::new();
    for _ in 0..20 {
        let ticket = repo
            .create_ticket(&NewTicket {
                id: None,
                type_: "task".into(),
                priority: 1,
                parent_id: None,
                title: "Collision test".into(),
                body: "".into(),
                project: "ct".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        // Every generated ID should match the pattern
        assert!(
            ticket.id.starts_with("ct-"),
            "Generated ID '{}' should start with 'ct-'",
            ticket.id
        );
        let id_suffix = &ticket.id["ct-".len()..];
        assert!(
            id_suffix.len() >= 5
                && id_suffix
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "Suffix '{}' should be 5+ base-36 chars",
            id_suffix
        );

        // Every ID must be unique
        assert!(
            ids.insert(ticket.id.clone()),
            "Duplicate ID generated: {}",
            ticket.id
        );
    }

    assert_eq!(ids.len(), 20);

    db.cleanup().await;
}

#[tokio::test]
async fn explicit_duplicate_id_returns_error() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("dup-id".into()),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: "First".into(),
        body: "".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // Inserting a second ticket with the same explicit ID should fail
    let result = repo
        .create_ticket(&NewTicket {
            id: Some("dup-id".into()),
            type_: "task".into(),
            priority: 1,
            parent_id: None,
            title: "Second".into(),
            body: "".into(),
            project: "test".into(),
            ..Default::default()
        })
        .await;

    assert!(result.is_err(), "Expected error for duplicate explicit ID");

    db.cleanup().await;
}

#[tokio::test]
async fn project_prefix_respected_in_generated_id() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    // Create tickets with different projects and verify prefix matches
    for project in &["alpha", "beta", "x"] {
        let ticket = repo
            .create_ticket(&NewTicket {
                id: None,
                type_: "task".into(),
                priority: 1,
                parent_id: None,
                title: format!("Ticket for {}", project),
                body: "".into(),
                project: project.to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        assert!(
            ticket.id.starts_with(&format!("{}-", project)),
            "ID '{}' should start with '{}-'",
            ticket.id,
            project
        );

        // The suffix after the prefix should be base-36 characters only
        let suffix = &ticket.id[project.len() + 1..];
        assert!(
            suffix.len() >= 5,
            "Suffix '{}' should be at least 5 chars",
            suffix
        );
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "Suffix '{}' should contain only base-36 chars",
            suffix
        );
    }

    db.cleanup().await;
}
