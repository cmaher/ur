// Tests for TicketRepo.

use crate::graph::GraphManager;
use crate::model::{EdgeKind, LifecycleStatus, NewTicket, TicketFilter, TicketUpdate};
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
        id: "epic-1".into(),
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
        id: "epic-2".into(),
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
        id: "task-1a".into(),
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
        id: "task-1b".into(),
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
        id: "task-1c".into(),
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
        id: "task-2a".into(),
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
        id: "task-2b".into(),
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
        id: "standalone".into(),
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
            id: "t-001".into(),
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
        id: "parent".into(),
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
            id: "child".into(),
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
        id: "t-upd".into(),
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
        id: "task".into(),
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
        id: "child".into(),
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
        id: "t-meta".into(),
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
        id: "t-upsert".into(),
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
        id: "t-act".into(),
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
        id: "empty-epic".into(),
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
        id: "childless".into(),
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
        id: "epic-no-kids".into(),
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
// Workflow CRUD tests
// ============================================================

#[tokio::test]
async fn create_and_get_workflow() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-t1".into(),
        type_: "task".into(),
        priority: 1,
        title: "Workflow test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let wf = repo
        .create_workflow("wf-t1", LifecycleStatus::Open)
        .await
        .unwrap();
    assert_eq!(wf.ticket_id, "wf-t1");
    assert_eq!(wf.status, LifecycleStatus::Open);
    assert!(!wf.id.is_empty());

    let fetched = repo.get_workflow_by_ticket("wf-t1").await.unwrap().unwrap();
    assert_eq!(fetched.id, wf.id);
    assert_eq!(fetched.status, LifecycleStatus::Open);

    db.cleanup().await;
}

#[tokio::test]
async fn get_workflow_returns_none_when_missing() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    let result = repo.get_workflow_by_ticket("no-such").await.unwrap();
    assert!(result.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn create_workflow_fails_duplicate_ticket() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-dup".into(),
        type_: "task".into(),
        priority: 1,
        title: "Dup test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_workflow("wf-dup", LifecycleStatus::Open)
        .await
        .unwrap();

    let result = repo
        .create_workflow("wf-dup", LifecycleStatus::Implementing)
        .await;
    assert!(result.is_err());

    db.cleanup().await;
}

#[tokio::test]
async fn update_workflow_status() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-upd".into(),
        type_: "task".into(),
        priority: 1,
        title: "Update wf".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_workflow("wf-upd", LifecycleStatus::Open)
        .await
        .unwrap();

    repo.update_workflow_status("wf-upd", LifecycleStatus::Implementing)
        .await
        .unwrap();

    let wf = repo
        .get_workflow_by_ticket("wf-upd")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wf.status, LifecycleStatus::Implementing);

    db.cleanup().await;
}

#[tokio::test]
async fn delete_workflow() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-del".into(),
        type_: "task".into(),
        priority: 1,
        title: "Del wf".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_workflow("wf-del", LifecycleStatus::Open)
        .await
        .unwrap();

    repo.delete_workflow("wf-del").await.unwrap();

    let result = repo.get_workflow_by_ticket("wf-del").await.unwrap();
    assert!(result.is_none());

    db.cleanup().await;
}

// ============================================================
// WorkflowIntent CRUD tests
// ============================================================

#[tokio::test]
async fn create_and_poll_intent() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "int-t1".into(),
        type_: "task".into(),
        priority: 1,
        title: "Intent test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let intent = repo
        .create_intent("int-t1", LifecycleStatus::Implementing)
        .await
        .unwrap();
    assert_eq!(intent.ticket_id, "int-t1");
    assert_eq!(intent.target_status, LifecycleStatus::Implementing);

    let polled = repo.poll_intent().await.unwrap().unwrap();
    assert_eq!(polled.id, intent.id);
    assert_eq!(polled.target_status, LifecycleStatus::Implementing);

    db.cleanup().await;
}

#[tokio::test]
async fn poll_intent_returns_none_when_empty() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    let result = repo.poll_intent().await.unwrap();
    assert!(result.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn delete_intent() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "int-del".into(),
        type_: "task".into(),
        priority: 1,
        title: "Del intent".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let intent = repo
        .create_intent("int-del", LifecycleStatus::Pushing)
        .await
        .unwrap();

    repo.delete_intent(&intent.id).await.unwrap();

    let polled = repo.poll_intent().await.unwrap();
    assert!(polled.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn poll_intent_returns_oldest_first() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "int-ord1".into(),
        type_: "task".into(),
        priority: 1,
        title: "Order 1".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: "int-ord2".into(),
        type_: "task".into(),
        priority: 1,
        title: "Order 2".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let first = repo
        .create_intent("int-ord1", LifecycleStatus::Implementing)
        .await
        .unwrap();
    repo.create_intent("int-ord2", LifecycleStatus::Pushing)
        .await
        .unwrap();

    let polled = repo.poll_intent().await.unwrap().unwrap();
    assert_eq!(polled.id, first.id);
    assert_eq!(polled.ticket_id, "int-ord1");

    // Delete first, poll should return second
    repo.delete_intent(&first.id).await.unwrap();
    let polled2 = repo.poll_intent().await.unwrap().unwrap();
    assert_eq!(polled2.ticket_id, "int-ord2");

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
// Workflow stall and lifecycle column tests
// ============================================================

#[tokio::test]
async fn workflow_new_has_default_stall_fields() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-stall1".into(),
        type_: "task".into(),
        priority: 1,
        title: "Stall test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let wf = repo
        .create_workflow("wf-stall1", LifecycleStatus::Open)
        .await
        .unwrap();

    assert!(!wf.stalled);
    assert_eq!(wf.stall_reason, "");
    assert_eq!(wf.implement_cycles, 0);
    assert_eq!(wf.worker_id, "");
    assert!(!wf.noverify);
    assert_eq!(wf.feedback_mode, "");

    // Verify defaults are returned by get_workflow_by_ticket too.
    let fetched = repo
        .get_workflow_by_ticket("wf-stall1")
        .await
        .unwrap()
        .unwrap();
    assert!(!fetched.stalled);
    assert_eq!(fetched.implement_cycles, 0);

    db.cleanup().await;
}

#[tokio::test]
async fn set_and_clear_workflow_stall() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-stall2".into(),
        type_: "task".into(),
        priority: 1,
        title: "Stall set/clear".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_workflow("wf-stall2", LifecycleStatus::Implementing)
        .await
        .unwrap();

    repo.set_workflow_stalled("wf-stall2", "handler failed: timeout")
        .await
        .unwrap();

    let wf = repo
        .get_workflow_by_ticket("wf-stall2")
        .await
        .unwrap()
        .unwrap();
    assert!(wf.stalled);
    assert_eq!(wf.stall_reason, "handler failed: timeout");

    repo.clear_workflow_stall("wf-stall2").await.unwrap();

    let wf = repo
        .get_workflow_by_ticket("wf-stall2")
        .await
        .unwrap()
        .unwrap();
    assert!(!wf.stalled);
    assert_eq!(wf.stall_reason, "");

    db.cleanup().await;
}

#[tokio::test]
async fn increment_implement_cycles() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-cyc1".into(),
        type_: "task".into(),
        priority: 1,
        title: "Cycle test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_workflow("wf-cyc1", LifecycleStatus::Implementing)
        .await
        .unwrap();

    repo.increment_implement_cycles("wf-cyc1").await.unwrap();
    repo.increment_implement_cycles("wf-cyc1").await.unwrap();
    repo.increment_implement_cycles("wf-cyc1").await.unwrap();

    let wf = repo
        .get_workflow_by_ticket("wf-cyc1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wf.implement_cycles, 3);

    db.cleanup().await;
}

#[tokio::test]
async fn set_workflow_worker_id() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-wid1".into(),
        type_: "task".into(),
        priority: 1,
        title: "Worker id test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_workflow("wf-wid1", LifecycleStatus::Implementing)
        .await
        .unwrap();

    repo.set_workflow_worker_id("wf-wid1", "worker-abc123")
        .await
        .unwrap();

    let wf = repo
        .get_workflow_by_ticket("wf-wid1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wf.worker_id, "worker-abc123");

    db.cleanup().await;
}

#[tokio::test]
async fn set_workflow_noverify() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-nv1".into(),
        type_: "task".into(),
        priority: 1,
        title: "Noverify test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_workflow("wf-nv1", LifecycleStatus::Implementing)
        .await
        .unwrap();

    repo.set_workflow_noverify("wf-nv1", true).await.unwrap();

    let wf = repo
        .get_workflow_by_ticket("wf-nv1")
        .await
        .unwrap()
        .unwrap();
    assert!(wf.noverify);

    repo.set_workflow_noverify("wf-nv1", false).await.unwrap();

    let wf = repo
        .get_workflow_by_ticket("wf-nv1")
        .await
        .unwrap()
        .unwrap();
    assert!(!wf.noverify);

    db.cleanup().await;
}

#[tokio::test]
async fn set_workflow_feedback_mode() {
    let db = TestDb::new().await;
    let repo = repo(&db);

    repo.create_ticket(&NewTicket {
        id: "wf-fb1".into(),
        type_: "task".into(),
        priority: 1,
        title: "Feedback mode test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_workflow("wf-fb1", LifecycleStatus::Implementing)
        .await
        .unwrap();

    repo.set_workflow_feedback_mode("wf-fb1", "inline")
        .await
        .unwrap();

    let wf = repo
        .get_workflow_by_ticket("wf-fb1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wf.feedback_mode, "inline");

    db.cleanup().await;
}
