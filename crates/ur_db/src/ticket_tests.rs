use crate::ticket::{CreateTicketParams, ListTicketFilters, UpdateTicketFields};
use crate::DatabaseManager;

fn fresh_db() -> DatabaseManager {
    DatabaseManager::create_in_memory().expect("create in-memory db")
}

fn create_epic(db: &DatabaseManager, project: &str) -> String {
    db.create_ticket(
        project,
        &CreateTicketParams {
            ticket_type: "epic".into(),
            status: "open".into(),
            priority: 1,
            parent_id: None,
            title: "Test Epic".into(),
            body: "An epic for testing.".into(),
        },
    )
    .expect("create epic")
}

// === create_ticket ===

#[test]
fn create_top_level_ticket_generates_project_prefixed_id() {
    let db = fresh_db();
    let id = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "epic".into(),
                status: "open".into(),
                priority: 1,
                parent_id: None,
                title: "My Epic".into(),
                body: "Body text.".into(),
            },
        )
        .unwrap();

    assert!(id.starts_with("ur."), "ID should start with project prefix, got: {id}");
    // Format: ur.XXXX (4 alphanumeric chars)
    let suffix = &id[3..];
    assert_eq!(suffix.len(), 4, "suffix should be 4 chars, got: {suffix}");
    assert!(
        suffix.chars().all(|c| c.is_ascii_alphanumeric()),
        "suffix should be alphanumeric, got: {suffix}"
    );
}

#[test]
fn create_child_ticket_generates_sequential_id() {
    let db = fresh_db();
    let epic_id = create_epic(&db, "ur");

    let child0 = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "open".into(),
                priority: 2,
                parent_id: Some(epic_id.clone()),
                title: "First child".into(),
                body: "".into(),
            },
        )
        .unwrap();
    assert_eq!(child0, format!("{epic_id}.0"));

    let child1 = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "open".into(),
                priority: 2,
                parent_id: Some(epic_id.clone()),
                title: "Second child".into(),
                body: "".into(),
            },
        )
        .unwrap();
    assert_eq!(child1, format!("{epic_id}.1"));

    let child2 = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "bug".into(),
                status: "open".into(),
                priority: 3,
                parent_id: Some(epic_id.clone()),
                title: "Third child".into(),
                body: "".into(),
            },
        )
        .unwrap();
    assert_eq!(child2, format!("{epic_id}.2"));
}

#[test]
fn create_child_ticket_with_missing_parent_fails() {
    let db = fresh_db();
    let err = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "open".into(),
                priority: 2,
                parent_id: Some("ur.nonexistent".into()),
                title: "Orphan".into(),
                body: "".into(),
            },
        )
        .expect_err("should fail with missing parent");
    assert!(
        err.contains("Parent ticket not found"),
        "error should mention parent not found, got: {err}"
    );
}

#[test]
fn create_ticket_stores_all_fields() {
    let db = fresh_db();
    let id = db
        .create_ticket(
            "proj",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "in_progress".into(),
                priority: 5,
                parent_id: None,
                title: "Important Task".into(),
                body: "Detailed body text here.".into(),
            },
        )
        .unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.ticket.ticket_type, "task");
    assert_eq!(detail.ticket.status, "in_progress");
    assert_eq!(detail.ticket.priority, 5);
    assert_eq!(detail.ticket.parent_id, "");
    assert_eq!(detail.ticket.title, "Important Task");
    assert_eq!(detail.ticket.body, "Detailed body text here.");
    assert!(!detail.ticket.created_at.is_empty());
    assert!(!detail.ticket.updated_at.is_empty());
}

#[test]
fn create_multiple_top_level_tickets_generates_unique_ids() {
    let db = fresh_db();
    let mut ids = Vec::new();
    for i in 0..20 {
        let id = db
            .create_ticket(
                "ur",
                &CreateTicketParams {
                    ticket_type: "task".into(),
                    status: "open".into(),
                    priority: 1,
                    parent_id: None,
                    title: format!("Ticket {i}"),
                    body: "".into(),
                },
            )
            .unwrap();
        ids.push(id);
    }
    // All IDs should be unique
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "all generated IDs should be unique");
}

// === get_ticket ===

#[test]
fn get_ticket_returns_ticket_with_metadata_and_activity() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");

    // Add metadata
    db.set_meta(&id, "assignee", "alice").unwrap();
    db.set_meta(&id, "tag", "infra").unwrap();

    // Add activity (manually since add_activity is a separate ticket)
    db.run(&format!(
        r#"
        ?[id, ticket_id, timestamp, author, message] <- [
            ["act.1", "{id}", "2026-03-13T10:00:00Z", "alice", "Started work."]
        ]
        :put activity {{id => ticket_id, timestamp, author, message}}
        "#,
    ))
    .unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.ticket.id, id);
    assert_eq!(detail.metadata.len(), 2);
    assert_eq!(detail.activities.len(), 1);
    assert_eq!(detail.activities[0].author, "alice");
}

#[test]
fn get_ticket_not_found() {
    let db = fresh_db();
    let err = db
        .get_ticket("ur.nonexistent")
        .expect_err("should fail for missing ticket");
    assert!(
        err.contains("Ticket not found"),
        "error should mention not found, got: {err}"
    );
}

#[test]
fn get_ticket_with_no_metadata_or_activity() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.ticket.id, id);
    assert!(detail.metadata.is_empty());
    assert!(detail.activities.is_empty());
}

// === list_tickets ===

#[test]
fn list_tickets_no_filters_returns_all() {
    let db = fresh_db();
    create_epic(&db, "ur");
    create_epic(&db, "ur");
    create_epic(&db, "other");

    let all = db.list_tickets(&ListTicketFilters::default()).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn list_tickets_filter_by_project() {
    let db = fresh_db();
    create_epic(&db, "ur");
    create_epic(&db, "ur");
    create_epic(&db, "other");

    let filtered = db
        .list_tickets(&ListTicketFilters {
            project: Some("ur".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(filtered.len(), 2);
    assert!(filtered.iter().all(|t| t.id.starts_with("ur.")));
}

#[test]
fn list_tickets_filter_by_type() {
    let db = fresh_db();
    let epic_id = create_epic(&db, "ur");
    db.create_ticket(
        "ur",
        &CreateTicketParams {
            ticket_type: "task".into(),
            status: "open".into(),
            priority: 2,
            parent_id: Some(epic_id),
            title: "A task".into(),
            body: "".into(),
        },
    )
    .unwrap();

    let tasks = db
        .list_tickets(&ListTicketFilters {
            ticket_type: Some("task".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].ticket_type, "task");
}

#[test]
fn list_tickets_filter_by_status() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");
    db.update_ticket(
        &id,
        &UpdateTicketFields {
            status: Some("closed".into()),
            priority: None,
            title: None,
            body: None,
        },
    )
    .unwrap();
    create_epic(&db, "ur"); // another epic, still open

    let closed = db
        .list_tickets(&ListTicketFilters {
            status: Some("closed".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].id, id);
}

#[test]
fn list_tickets_filter_by_parent() {
    let db = fresh_db();
    let epic_id = create_epic(&db, "ur");
    for i in 0..3 {
        db.create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "open".into(),
                priority: 2,
                parent_id: Some(epic_id.clone()),
                title: format!("Child {i}"),
                body: "".into(),
            },
        )
        .unwrap();
    }

    let children = db
        .list_tickets(&ListTicketFilters {
            parent_id: Some(epic_id.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(children.len(), 3);
    assert!(children.iter().all(|t| t.parent_id == epic_id));
}

#[test]
fn list_tickets_filter_by_metadata() {
    let db = fresh_db();
    let id1 = create_epic(&db, "ur");
    let id2 = create_epic(&db, "ur");
    create_epic(&db, "ur"); // no metadata

    db.set_meta(&id1, "assignee", "alice").unwrap();
    db.set_meta(&id2, "assignee", "bob").unwrap();

    // Filter by key only
    let with_assignee = db
        .list_tickets(&ListTicketFilters {
            meta_key: Some("assignee".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(with_assignee.len(), 2);

    // Filter by key and value
    let alice_tickets = db
        .list_tickets(&ListTicketFilters {
            meta_key: Some("assignee".into()),
            meta_value: Some("alice".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(alice_tickets.len(), 1);
    assert_eq!(alice_tickets[0].id, id1);
}

#[test]
fn list_tickets_combined_filters() {
    let db = fresh_db();
    let epic_id = create_epic(&db, "ur");

    let task_id = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "open".into(),
                priority: 2,
                parent_id: Some(epic_id.clone()),
                title: "Open task".into(),
                body: "".into(),
            },
        )
        .unwrap();

    db.create_ticket(
        "ur",
        &CreateTicketParams {
            ticket_type: "bug".into(),
            status: "open".into(),
            priority: 3,
            parent_id: Some(epic_id.clone()),
            title: "Open bug".into(),
            body: "".into(),
        },
    )
    .unwrap();

    let filtered = db
        .list_tickets(&ListTicketFilters {
            parent_id: Some(epic_id),
            ticket_type: Some("task".into()),
            status: Some("open".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, task_id);
}

#[test]
fn list_tickets_empty_result() {
    let db = fresh_db();
    let result = db.list_tickets(&ListTicketFilters::default()).unwrap();
    assert!(result.is_empty());
}

// === update_ticket ===

#[test]
fn update_ticket_changes_status() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");

    db.update_ticket(
        &id,
        &UpdateTicketFields {
            status: Some("in_progress".into()),
            priority: None,
            title: None,
            body: None,
        },
    )
    .unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.ticket.status, "in_progress");
    assert_eq!(detail.ticket.title, "Test Epic"); // unchanged
}

#[test]
fn update_ticket_changes_multiple_fields() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");

    db.update_ticket(
        &id,
        &UpdateTicketFields {
            status: Some("closed".into()),
            priority: Some(10),
            title: Some("Updated Title".into()),
            body: Some("Updated body.".into()),
        },
    )
    .unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.ticket.status, "closed");
    assert_eq!(detail.ticket.priority, 10);
    assert_eq!(detail.ticket.title, "Updated Title");
    assert_eq!(detail.ticket.body, "Updated body.");
}

#[test]
fn update_ticket_preserves_unchanged_fields() {
    let db = fresh_db();
    let id = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "open".into(),
                priority: 3,
                parent_id: None,
                title: "Original Title".into(),
                body: "Original Body".into(),
            },
        )
        .unwrap();

    db.update_ticket(
        &id,
        &UpdateTicketFields {
            status: Some("closed".into()),
            priority: None,
            title: None,
            body: None,
        },
    )
    .unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.ticket.status, "closed");
    assert_eq!(detail.ticket.priority, 3);
    assert_eq!(detail.ticket.title, "Original Title");
    assert_eq!(detail.ticket.body, "Original Body");
}

#[test]
fn update_ticket_not_found() {
    let db = fresh_db();
    let err = db
        .update_ticket(
            "ur.nonexistent",
            &UpdateTicketFields {
                status: Some("closed".into()),
                priority: None,
                title: None,
                body: None,
            },
        )
        .expect_err("should fail for missing ticket");
    assert!(err.contains("Ticket not found"));
}

#[test]
fn update_ticket_updates_timestamp() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");
    let before = db.get_ticket(&id).unwrap();

    // Small delay to ensure timestamp changes (at least 1 second resolution)
    std::thread::sleep(std::time::Duration::from_millis(1100));

    db.update_ticket(
        &id,
        &UpdateTicketFields {
            status: Some("closed".into()),
            priority: None,
            title: None,
            body: None,
        },
    )
    .unwrap();

    let after = db.get_ticket(&id).unwrap();
    assert_eq!(before.ticket.created_at, after.ticket.created_at);
    assert_ne!(
        before.ticket.updated_at, after.ticket.updated_at,
        "updated_at should change after update"
    );
}

// === set_meta / delete_meta ===

#[test]
fn set_meta_creates_new_entry() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");

    db.set_meta(&id, "assignee", "alice").unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.metadata.len(), 1);
    assert_eq!(detail.metadata[0].key, "assignee");
    assert_eq!(detail.metadata[0].value, "alice");
}

#[test]
fn set_meta_updates_existing_key() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");

    db.set_meta(&id, "assignee", "alice").unwrap();
    db.set_meta(&id, "assignee", "bob").unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.metadata.len(), 1);
    assert_eq!(detail.metadata[0].value, "bob");
}

#[test]
fn set_meta_multiple_keys() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");

    db.set_meta(&id, "assignee", "alice").unwrap();
    db.set_meta(&id, "tag", "infra").unwrap();
    db.set_meta(&id, "priority_label", "high").unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.metadata.len(), 3);
}

#[test]
fn set_meta_on_nonexistent_ticket_fails() {
    let db = fresh_db();
    let err = db
        .set_meta("ur.nonexistent", "key", "value")
        .expect_err("should fail");
    assert!(err.contains("Ticket not found"));
}

#[test]
fn delete_meta_removes_entry() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");

    db.set_meta(&id, "assignee", "alice").unwrap();
    db.set_meta(&id, "tag", "infra").unwrap();

    db.delete_meta(&id, "assignee").unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.metadata.len(), 1);
    assert_eq!(detail.metadata[0].key, "tag");
}

#[test]
fn delete_meta_nonexistent_key_succeeds_silently() {
    let db = fresh_db();
    let id = create_epic(&db, "ur");

    // Should not error even if key doesn't exist
    db.delete_meta(&id, "nonexistent_key").unwrap();
}

#[test]
fn delete_meta_on_nonexistent_ticket_fails() {
    let db = fresh_db();
    let err = db
        .delete_meta("ur.nonexistent", "key")
        .expect_err("should fail");
    assert!(err.contains("Ticket not found"));
}

// === Edge cases ===

#[test]
fn create_ticket_with_special_characters_in_title() {
    let db = fresh_db();
    let id = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "open".into(),
                priority: 1,
                parent_id: None,
                title: r#"Fix "quoted" thing & backslash \ test"#.into(),
                body: "Body with \"quotes\" and \\backslashes\\".into(),
            },
        )
        .unwrap();

    let detail = db.get_ticket(&id).unwrap();
    assert_eq!(detail.ticket.title, r#"Fix "quoted" thing & backslash \ test"#);
    assert_eq!(detail.ticket.body, "Body with \"quotes\" and \\backslashes\\");
}

#[test]
fn create_nested_children() {
    let db = fresh_db();
    let epic_id = create_epic(&db, "ur");

    let child_id = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "open".into(),
                priority: 2,
                parent_id: Some(epic_id.clone()),
                title: "Child".into(),
                body: "".into(),
            },
        )
        .unwrap();
    assert_eq!(child_id, format!("{epic_id}.0"));

    // Create a grandchild under the child
    let grandchild_id = db
        .create_ticket(
            "ur",
            &CreateTicketParams {
                ticket_type: "task".into(),
                status: "open".into(),
                priority: 2,
                parent_id: Some(child_id.clone()),
                title: "Grandchild".into(),
                body: "".into(),
            },
        )
        .unwrap();
    assert_eq!(grandchild_id, format!("{child_id}.0"));

    // Verify the grandchild is retrievable
    let detail = db.get_ticket(&grandchild_id).unwrap();
    assert_eq!(detail.ticket.parent_id, child_id);
}

#[test]
fn different_projects_have_independent_ids() {
    let db = fresh_db();
    let ur_id = create_epic(&db, "ur");
    let other_id = create_epic(&db, "other");

    assert!(ur_id.starts_with("ur."));
    assert!(other_id.starts_with("other."));

    // Both should be retrievable
    db.get_ticket(&ur_id).unwrap();
    db.get_ticket(&other_id).unwrap();
}
