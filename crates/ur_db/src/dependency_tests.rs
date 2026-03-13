use crate::ticket::CreateTicketParams;
use crate::DatabaseManager;

fn fresh_db() -> DatabaseManager {
    DatabaseManager::create_in_memory().expect("create in-memory db")
}

/// Create an epic and return its ID.
fn create_epic(db: &DatabaseManager, project: &str) -> String {
    db.create_ticket(
        project,
        &CreateTicketParams {
            ticket_type: "epic".into(),
            status: "open".into(),
            priority: 1,
            parent_id: None,
            title: "Test Epic".into(),
            body: "".into(),
        },
    )
    .expect("create epic")
}

/// Create a task child under a parent and return its ID.
fn create_task(db: &DatabaseManager, project: &str, parent_id: &str, title: &str) -> String {
    db.create_ticket(
        project,
        &CreateTicketParams {
            ticket_type: "task".into(),
            status: "open".into(),
            priority: 2,
            parent_id: Some(parent_id.to_string()),
            title: title.into(),
            body: "".into(),
        },
    )
    .expect("create task")
}

/// Create a bug child under a parent and return its ID.
fn create_bug(db: &DatabaseManager, project: &str, parent_id: &str, title: &str) -> String {
    db.create_ticket(
        project,
        &CreateTicketParams {
            ticket_type: "bug".into(),
            status: "open".into(),
            priority: 3,
            parent_id: Some(parent_id.to_string()),
            title: title.into(),
            body: "".into(),
        },
    )
    .expect("create bug")
}

// === add_block ===

#[test]
fn add_block_simple() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    db.add_block(&t1, &t2).unwrap();

    // Verify edge exists
    let result = db
        .run(&format!(
            r#"?[b, d] := *blocks{{blocker_id: b, blocked_id: d}}, b = "{t1}", d = "{t2}""#
        ))
        .unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn add_block_self_loop_rejected() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");

    let err = db.add_block(&t1, &t1).expect_err("should reject self-loop");
    assert!(
        err.contains("Cannot block a ticket on itself"),
        "unexpected error: {err}"
    );
}

#[test]
fn add_block_nonexistent_blocker_fails() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");

    let err = db
        .add_block("ur.nonexistent", &t1)
        .expect_err("should fail");
    assert!(err.contains("Ticket not found"), "unexpected error: {err}");
}

#[test]
fn add_block_nonexistent_blocked_fails() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");

    let err = db
        .add_block(&t1, "ur.nonexistent")
        .expect_err("should fail");
    assert!(err.contains("Ticket not found"), "unexpected error: {err}");
}

#[test]
fn add_block_direct_cycle_rejected() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    db.add_block(&t1, &t2).unwrap();

    let err = db
        .add_block(&t2, &t1)
        .expect_err("should reject reverse edge creating direct cycle");
    assert!(
        err.contains("would create a cycle"),
        "unexpected error: {err}"
    );
}

#[test]
fn add_block_transitive_cycle_rejected() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");
    let t3 = create_task(&db, "ur", &epic, "Task 3");

    // t1 -> t2 -> t3
    db.add_block(&t1, &t2).unwrap();
    db.add_block(&t2, &t3).unwrap();

    // t3 -> t1 would create a cycle
    let err = db
        .add_block(&t3, &t1)
        .expect_err("should reject transitive cycle");
    assert!(
        err.contains("would create a cycle"),
        "unexpected error: {err}"
    );
}

#[test]
fn add_block_long_chain_cycle_rejected() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");
    let t3 = create_task(&db, "ur", &epic, "Task 3");
    let t4 = create_task(&db, "ur", &epic, "Task 4");

    // t1 -> t2 -> t3 -> t4
    db.add_block(&t1, &t2).unwrap();
    db.add_block(&t2, &t3).unwrap();
    db.add_block(&t3, &t4).unwrap();

    // t4 -> t1 would create a 4-node cycle
    let err = db
        .add_block(&t4, &t1)
        .expect_err("should reject long chain cycle");
    assert!(err.contains("would create a cycle"));
}

#[test]
fn add_block_non_cycle_path_allowed() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");
    let t3 = create_task(&db, "ur", &epic, "Task 3");

    // t1 -> t2, t1 -> t3 (diamond, no cycle)
    db.add_block(&t1, &t2).unwrap();
    db.add_block(&t1, &t3).unwrap();

    // t2 -> t3 is fine (no cycle, just converging paths)
    db.add_block(&t2, &t3).unwrap();
}

// === remove_block ===

#[test]
fn remove_block_removes_edge() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    db.add_block(&t1, &t2).unwrap();
    db.remove_block(&t1, &t2).unwrap();

    let result = db
        .run("?[b, d] := *blocks{blocker_id: b, blocked_id: d}")
        .unwrap();
    assert!(result.rows.is_empty());
}

#[test]
fn remove_block_nonexistent_edge_succeeds() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    // No edge exists; should succeed silently
    db.remove_block(&t1, &t2).unwrap();
}

#[test]
fn remove_block_nonexistent_ticket_fails() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");

    let err = db
        .remove_block(&t1, "ur.nonexistent")
        .expect_err("should fail");
    assert!(err.contains("Ticket not found"));
}

// === add_link / remove_link ===

#[test]
fn add_link_creates_soft_link() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    db.add_link(&t1, &t2).unwrap();

    let result = db
        .run("?[l, r] := *relates_to{left_id: l, right_id: r}")
        .unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn add_link_nonexistent_ticket_fails() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");

    let err = db
        .add_link(&t1, "ur.nonexistent")
        .expect_err("should fail");
    assert!(err.contains("Ticket not found"));
}

#[test]
fn remove_link_removes_soft_link() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    db.add_link(&t1, &t2).unwrap();
    db.remove_link(&t1, &t2).unwrap();

    let result = db
        .run("?[l, r] := *relates_to{left_id: l, right_id: r}")
        .unwrap();
    assert!(result.rows.is_empty());
}

#[test]
fn remove_link_nonexistent_succeeds() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    db.remove_link(&t1, &t2).unwrap();
}

// === dispatchable_tickets ===

#[test]
fn dispatchable_tickets_returns_open_unblocked_children() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");
    let _t3 = create_bug(&db, "ur", &epic, "Bug 1");

    // t1 blocks t2
    db.add_block(&t1, &t2).unwrap();

    let dispatchable = db.dispatchable_tickets(&epic).unwrap();
    let ids: Vec<&str> = dispatchable.iter().map(|t| t.id.as_str()).collect();

    // t1 is open+unblocked, bug is open+unblocked, t2 is blocked by t1
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&t1.as_str()));
    assert!(ids.contains(&_t3.as_str()));
    assert!(!ids.contains(&t2.as_str()));
}

#[test]
fn dispatchable_tickets_blocked_by_closed_ticket_is_dispatchable() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    db.add_block(&t1, &t2).unwrap();

    // Close t1 -- t2 should now be dispatchable
    db.update_ticket(
        &t1,
        &crate::ticket::UpdateTicketFields {
            status: Some("closed".into()),
            priority: None,
            title: None,
            body: None,
        },
    )
    .unwrap();

    let dispatchable = db.dispatchable_tickets(&epic).unwrap();
    let ids: Vec<&str> = dispatchable.iter().map(|t| t.id.as_str()).collect();

    // t1 is closed so not dispatchable, t2 is now unblocked and open
    assert_eq!(ids.len(), 1);
    assert!(ids.contains(&t2.as_str()));
}

#[test]
fn dispatchable_tickets_excludes_non_dispatchable_types() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let _task = create_task(&db, "ur", &epic, "A task");

    // Create a design ticket (non-dispatchable type)
    db.create_ticket(
        "ur",
        &CreateTicketParams {
            ticket_type: "design".into(),
            status: "open".into(),
            priority: 2,
            parent_id: Some(epic.clone()),
            title: "A design".into(),
            body: "".into(),
        },
    )
    .unwrap();

    let dispatchable = db.dispatchable_tickets(&epic).unwrap();
    assert_eq!(dispatchable.len(), 1);
    assert_eq!(dispatchable[0].id, _task);
}

#[test]
fn dispatchable_tickets_excludes_non_open_status() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let _t2 = create_task(&db, "ur", &epic, "Task 2");

    // Set t1 to in_progress
    db.update_ticket(
        &t1,
        &crate::ticket::UpdateTicketFields {
            status: Some("in_progress".into()),
            priority: None,
            title: None,
            body: None,
        },
    )
    .unwrap();

    let dispatchable = db.dispatchable_tickets(&epic).unwrap();
    assert_eq!(dispatchable.len(), 1);
    assert_eq!(dispatchable[0].id, _t2);
}

#[test]
fn dispatchable_tickets_parent_child_does_not_count_as_blocking() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    // The epic is the parent of t1 and t2, but parent-child is structural,
    // not blocking. Both should be dispatchable.
    let dispatchable = db.dispatchable_tickets(&epic).unwrap();
    let ids: Vec<&str> = dispatchable.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&t1.as_str()));
    assert!(ids.contains(&t2.as_str()));
}

#[test]
fn dispatchable_tickets_empty_epic() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");

    let dispatchable = db.dispatchable_tickets(&epic).unwrap();
    assert!(dispatchable.is_empty());
}

#[test]
fn dispatchable_tickets_transitive_block_still_blocks() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");
    let t3 = create_task(&db, "ur", &epic, "Task 3");

    // t1 -> t2 -> t3 (chain)
    db.add_block(&t1, &t2).unwrap();
    db.add_block(&t2, &t3).unwrap();

    let dispatchable = db.dispatchable_tickets(&epic).unwrap();
    let ids: Vec<&str> = dispatchable.iter().map(|t| t.id.as_str()).collect();

    // Only t1 is unblocked
    assert_eq!(ids.len(), 1);
    assert!(ids.contains(&t1.as_str()));
}

#[test]
fn remove_block_makes_ticket_dispatchable() {
    let db = fresh_db();
    let epic = create_epic(&db, "ur");
    let t1 = create_task(&db, "ur", &epic, "Task 1");
    let t2 = create_task(&db, "ur", &epic, "Task 2");

    db.add_block(&t1, &t2).unwrap();

    // Before removal: only t1 is dispatchable
    let before = db.dispatchable_tickets(&epic).unwrap();
    assert_eq!(before.len(), 1);

    db.remove_block(&t1, &t2).unwrap();

    // After removal: both are dispatchable
    let after = db.dispatchable_tickets(&epic).unwrap();
    assert_eq!(after.len(), 2);
}
