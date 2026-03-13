use crate::DatabaseManager;
use crate::query::QueryManager;

/// Build a DatabaseManager with all sample data populated.
fn populated_db() -> DatabaseManager {
    let db = DatabaseManager::create_in_memory().expect("failed to create in-memory db");

    // === Tickets ===
    // Initiative
    db.run(
        r#"
        ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
            "ur.o79g", "initiative", "open", 1, "",
            "Foundational State & Tickets",
            "Build the core ticket system for ur.",
            "2026-03-10T10:00:00Z", "2026-03-10T10:00:00Z"
        ]]
        :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
    "#,
    )
    .expect("insert initiative");

    // Project
    db.run(
        r#"
        ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
            "ur.o79g.0", "project", "open", 1, "ur.o79g",
            "CozoDB Integration",
            "Integrate CozoDB as the ticket persistence layer.",
            "2026-03-10T11:00:00Z", "2026-03-10T11:00:00Z"
        ]]
        :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
    "#,
    )
    .expect("insert project");

    // Epic A: Schema Design
    db.run(
        r#"
        ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
            "ur.o79g.0.a", "epic", "open", 2, "ur.o79g.0",
            "Schema Design",
            "Design and validate the CozoDB schema for tickets.",
            "2026-03-11T09:00:00Z", "2026-03-11T09:00:00Z"
        ]]
        :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
    "#,
    )
    .expect("insert epic A");

    // Epic A children
    db.run(
        r#"
        ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [
            ["ur.o79g.0.a.0", "task", "closed", 2, "ur.o79g.0.a",
             "Define ticket relation", "Create the ticket stored relation.",
             "2026-03-11T09:30:00Z", "2026-03-12T08:00:00Z"],
            ["ur.o79g.0.a.1", "task", "in_progress", 2, "ur.o79g.0.a",
             "Define metadata relations", "Create ticket_meta and activity_meta.",
             "2026-03-11T09:35:00Z", "2026-03-12T09:00:00Z"],
            ["ur.o79g.0.a.2", "task", "open", 3, "ur.o79g.0.a",
             "Define dependency relations", "Create blocks and relates_to.",
             "2026-03-11T09:40:00Z", "2026-03-11T09:40:00Z"],
            ["ur.o79g.0.a.3", "bug", "open", 2, "ur.o79g.0.a",
             "Fix nullable parent_id handling", "Empty string workaround needs validation.",
             "2026-03-12T10:00:00Z", "2026-03-12T10:00:00Z"]
        ]
        :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
    "#,
    )
    .expect("insert epic A children");

    // Epic B: Query Validation
    db.run(
        r#"
        ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
            "ur.o79g.0.b", "epic", "open", 2, "ur.o79g.0",
            "Query Validation",
            "Validate core Datalog query patterns against the schema.",
            "2026-03-11T10:00:00Z", "2026-03-11T10:00:00Z"
        ]]
        :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
    "#,
    )
    .expect("insert epic B");

    // Epic B children
    db.run(
        r#"
        ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [
            ["ur.o79g.0.b.0", "task", "open", 2, "ur.o79g.0.b",
             "Dispatchable ticket query", "Find ready-to-dispatch children of an epic.",
             "2026-03-11T10:30:00Z", "2026-03-11T10:30:00Z"],
            ["ur.o79g.0.b.1", "task", "open", 2, "ur.o79g.0.b",
             "DAG traversal query", "Transitive closure of blocks edges.",
             "2026-03-11T10:35:00Z", "2026-03-11T10:35:00Z"],
            ["ur.o79g.0.b.2", "design", "open", 3, "ur.o79g.0.b",
             "Cycle detection approach", "Evaluate CozoDB's graph cycle detection.",
             "2026-03-11T10:40:00Z", "2026-03-11T10:40:00Z"]
        ]
        :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
    "#,
    )
    .expect("insert epic B children");

    // === Metadata ===
    db.run(
        r#"
        ?[ticket_id, key, value] <- [
            ["ur.o79g", "assignee", "christian"],
            ["ur.o79g.0.a", "assignee", "christian"],
            ["ur.o79g.0.a", "tag", "schema"],
            ["ur.o79g.0.b", "assignee", "agent-1"],
            ["ur.o79g.0.b", "tag", "queries"],
            ["ur.o79g.0.a.0", "assignee", "agent-1"],
            ["ur.o79g.0.a.1", "assignee", "agent-2"],
            ["ur.o79g.0.b.0", "tag", "dispatch"],
            ["ur.o79g.0.b.1", "tag", "graph"]
        ]
        :put ticket_meta {ticket_id, key => value}
    "#,
    )
    .expect("insert metadata");

    // === Blocking dependencies ===
    db.run(
        r#"
        ?[blocker_id, blocked_id] <- [
            ["ur.o79g.0.a.0", "ur.o79g.0.a.1"],
            ["ur.o79g.0.a.1", "ur.o79g.0.a.2"],
            ["ur.o79g.0.a.0", "ur.o79g.0.b.0"],
            ["ur.o79g.0.a.2", "ur.o79g.0.b.1"]
        ]
        :put blocks {blocker_id, blocked_id}
    "#,
    )
    .expect("insert blocks");

    // === Soft relations ===
    db.run(
        r#"
        ?[left_id, right_id] <- [
            ["ur.o79g.0.a.3", "ur.o79g.0.a.0"],
            ["ur.o79g.0.b.0", "ur.o79g.0.b.1"]
        ]
        :put relates_to {left_id, right_id}
    "#,
    )
    .expect("insert relates_to");

    // === Activity entries ===
    db.run(
        r#"
        ?[id, ticket_id, timestamp, author, message] <- [
            ["act.001", "ur.o79g.0.a.0", "2026-03-11T14:00:00Z", "agent-1",
             "Created ticket relation with all fields."],
            ["act.002", "ur.o79g.0.a.0", "2026-03-12T08:00:00Z", "agent-1",
             "Completed and verified. Closing."],
            ["act.003", "ur.o79g.0.a.1", "2026-03-12T09:00:00Z", "agent-2",
             "Started work on metadata relations."],
            ["act.004", "ur.o79g.0.b.2", "2026-03-11T11:00:00Z", "christian",
             "Need to research CozoDB's built-in graph algorithms."]
        ]
        :put activity {id => ticket_id, timestamp, author, message}
    "#,
    )
    .expect("insert activity");

    // === Activity metadata ===
    db.run(
        r#"
        ?[activity_id, key, value] <- [
            ["act.001", "commit", "abc123"],
            ["act.002", "commit", "def456"],
            ["act.002", "status_change", "open->closed"],
            ["act.003", "status_change", "open->in_progress"]
        ]
        :put activity_meta {activity_id, key => value}
    "#,
    )
    .expect("insert activity_meta");

    db
}

#[test]
fn database_manager_creates_schema() {
    let db = DatabaseManager::create_in_memory().expect("schema creation should succeed");

    // Verify all six relations exist by querying each one
    db.run("?[id] := *ticket{id}").expect("ticket relation should exist");
    db.run("?[ticket_id] := *ticket_meta{ticket_id}")
        .expect("ticket_meta relation should exist");
    db.run("?[blocker_id] := *blocks{blocker_id}")
        .expect("blocks relation should exist");
    db.run("?[left_id] := *relates_to{left_id}")
        .expect("relates_to relation should exist");
    db.run("?[id] := *activity{id}")
        .expect("activity relation should exist");
    db.run("?[activity_id] := *activity_meta{activity_id}")
        .expect("activity_meta relation should exist");
}

#[test]
fn database_manager_sqlite_creates_schema() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let db_path = tmp.path().join("test.db");

    let db = DatabaseManager::create_with_sqlite(&db_path).expect("sqlite creation should succeed");

    // Verify all six relations exist
    db.run("?[id] := *ticket{id}").expect("ticket relation should exist");
    db.run("?[ticket_id] := *ticket_meta{ticket_id}")
        .expect("ticket_meta relation should exist");
    db.run("?[blocker_id] := *blocks{blocker_id}")
        .expect("blocks relation should exist");
    db.run("?[left_id] := *relates_to{left_id}")
        .expect("relates_to relation should exist");
    db.run("?[id] := *activity{id}")
        .expect("activity relation should exist");
    db.run("?[activity_id] := *activity_meta{activity_id}")
        .expect("activity_meta relation should exist");
}

#[test]
fn ticket_relation_stores_and_retrieves() {
    let db = populated_db();
    let result = db
        .run("?[id, title, type] := *ticket{id, title, type}")
        .unwrap();
    // 11 tickets total: 1 initiative + 1 project + 2 epics + 4 epic-A children + 3 epic-B children
    assert_eq!(result.rows.len(), 11);
}

#[test]
fn ticket_hierarchy_via_parent_id() {
    let db = populated_db();

    let result = db
        .run(r#"?[id, title] := *ticket{id, title, parent_id}, parent_id = "ur.o79g.0.a""#)
        .unwrap();
    assert_eq!(result.rows.len(), 4, "epic A should have 4 children");

    let result = db
        .run(r#"?[id, title] := *ticket{id, title, parent_id}, parent_id = "ur.o79g.0.b""#)
        .unwrap();
    assert_eq!(result.rows.len(), 3, "epic B should have 3 children");
}

#[test]
fn ticket_meta_stores_and_queries() {
    let db = populated_db();

    let result = db
        .run("?[ticket_id, key, value] := *ticket_meta{ticket_id, key, value}")
        .unwrap();
    assert_eq!(result.rows.len(), 9);

    let result = db
        .run(
            r#"?[ticket_id] := *ticket_meta{ticket_id, key, value}, key = "assignee", value = "agent-1""#,
        )
        .unwrap();
    assert_eq!(result.rows.len(), 2, "agent-1 has 2 assignments");

    let result = db
        .run(
            r#"?[ticket_id] := *ticket_meta{ticket_id, key, value}, key = "tag", value = "schema""#,
        )
        .unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn blocks_relation_stores_dependencies() {
    let db = populated_db();

    let result = db
        .run("?[blocker_id, blocked_id] := *blocks{blocker_id, blocked_id}")
        .unwrap();
    assert_eq!(result.rows.len(), 4, "should have 4 blocking edges");
}

#[test]
fn cross_epic_dependencies_exist() {
    let db = populated_db();

    let result = db
        .run(
            r#"?[blocker_id, blocked_id] := *blocks{blocker_id, blocked_id},
                starts_with(blocker_id, "ur.o79g.0.a"),
                starts_with(blocked_id, "ur.o79g.0.b")"#,
        )
        .unwrap();
    assert_eq!(
        result.rows.len(),
        2,
        "should have 2 cross-epic blocking edges"
    );
}

#[test]
fn relates_to_stores_soft_links() {
    let db = populated_db();

    let result = db
        .run("?[left_id, right_id] := *relates_to{left_id, right_id}")
        .unwrap();
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn activity_stores_and_retrieves() {
    let db = populated_db();

    let result = db
        .run("?[id, ticket_id, author, message] := *activity{id, ticket_id, author, message}")
        .unwrap();
    assert_eq!(result.rows.len(), 4);

    let result = db
        .run(r#"?[id, message] := *activity{id, ticket_id, message}, ticket_id = "ur.o79g.0.a.0""#)
        .unwrap();
    assert_eq!(
        result.rows.len(),
        2,
        "ticket a.0 should have 2 activity entries"
    );
}

#[test]
fn activity_meta_stores_and_retrieves() {
    let db = populated_db();

    let result = db
        .run("?[activity_id, key, value] := *activity_meta{activity_id, key, value}")
        .unwrap();
    assert_eq!(result.rows.len(), 4);

    let result = db
        .run(
            r#"?[activity_id, value] := *activity_meta{activity_id, key, value}, key = "status_change""#,
        )
        .unwrap();
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn ticket_types_are_diverse() {
    let db = populated_db();

    let result = db.run("?[type] := *ticket{type}").unwrap();
    let types: Vec<String> = result
        .rows
        .iter()
        .map(|r| r[0].get_str().unwrap().to_string())
        .collect();
    assert!(types.contains(&"initiative".to_string()));
    assert!(types.contains(&"project".to_string()));
    assert!(types.contains(&"epic".to_string()));
    assert!(types.contains(&"task".to_string()));
    assert!(types.contains(&"bug".to_string()));
    assert!(types.contains(&"design".to_string()));
}

#[test]
fn ticket_statuses_are_diverse() {
    let db = populated_db();

    let result = db.run("?[status] := *ticket{status}").unwrap();
    let statuses: Vec<String> = result
        .rows
        .iter()
        .map(|r| r[0].get_str().unwrap().to_string())
        .collect();
    assert!(statuses.contains(&"open".to_string()));
    assert!(statuses.contains(&"in_progress".to_string()));
    assert!(statuses.contains(&"closed".to_string()));
}

#[test]
fn duplicate_ticket_id_updates_instead_of_duplicating() {
    let db = populated_db();

    db.run(
        r#"
        ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
            "ur.o79g.0.a.0", "task", "closed", 2, "ur.o79g.0.a",
            "Define ticket relation (UPDATED)", "Create the ticket stored relation.",
            "2026-03-11T09:30:00Z", "2026-03-12T08:00:00Z"
        ]]
        :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
    "#,
    )
    .unwrap();

    let result = db.run("?[id] := *ticket{id}").unwrap();
    assert_eq!(result.rows.len(), 11);

    let result = db
        .run(r#"?[title] := *ticket{id, title}, id = "ur.o79g.0.a.0""#)
        .unwrap();
    let title = result.rows[0][0].get_str().unwrap();
    assert!(title.contains("UPDATED"));
}

#[test]
fn joined_query_ticket_with_metadata() {
    let db = populated_db();

    let result = db
        .run(
            r#"?[id, title, key, value] := *ticket{id, title}, *ticket_meta{ticket_id, key, value}, ticket_id = id"#,
        )
        .unwrap();
    assert!(
        result.rows.len() >= 5,
        "should have joined rows for tickets with metadata"
    );
}

#[test]
fn joined_query_activity_with_metadata() {
    let db = populated_db();

    let result = db
        .run(
            r#"?[id, message, key, value] := *activity{id, message}, *activity_meta{activity_id, key, value}, activity_id = id"#,
        )
        .unwrap();
    assert_eq!(
        result.rows.len(),
        4,
        "should have 4 joined activity+meta rows"
    );
}

// === QueryManager tests ===

fn query_mgr() -> QueryManager {
    QueryManager::new(populated_db())
}

#[test]
fn dispatchable_tickets_for_epic_a() {
    let qm = query_mgr();

    let tickets = qm.dispatchable_tickets("ur.o79g.0.a").unwrap();
    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].id, "ur.o79g.0.a.3");
    assert_eq!(tickets[0].title, "Fix nullable parent_id handling");
}

#[test]
fn dispatchable_tickets_for_epic_b() {
    let qm = query_mgr();

    let tickets = qm.dispatchable_tickets("ur.o79g.0.b").unwrap();
    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].id, "ur.o79g.0.b.0");
}

#[test]
fn dispatchable_tickets_empty_when_none_qualify() {
    let qm = query_mgr();

    let tickets = qm.dispatchable_tickets("ur.o79g").unwrap();
    assert_eq!(tickets.len(), 0);
}

#[test]
fn transitive_blockers_direct() {
    let qm = query_mgr();

    let blockers = qm.transitive_blockers("ur.o79g.0.a.1").unwrap();
    assert_eq!(blockers, vec!["ur.o79g.0.a.0"]);
}

#[test]
fn transitive_blockers_chain() {
    let qm = query_mgr();

    let blockers = qm.transitive_blockers("ur.o79g.0.b.1").unwrap();
    assert_eq!(
        blockers,
        vec!["ur.o79g.0.a.0", "ur.o79g.0.a.1", "ur.o79g.0.a.2"]
    );
}

#[test]
fn transitive_blockers_none() {
    let qm = query_mgr();

    let blockers = qm.transitive_blockers("ur.o79g.0.a.0").unwrap();
    assert!(blockers.is_empty());
}

#[test]
fn transitive_dependents_from_root() {
    let qm = query_mgr();

    let deps = qm.transitive_dependents("ur.o79g.0.a.0").unwrap();
    assert_eq!(
        deps,
        vec![
            "ur.o79g.0.a.1",
            "ur.o79g.0.a.2",
            "ur.o79g.0.b.0",
            "ur.o79g.0.b.1"
        ]
    );
}

#[test]
fn transitive_dependents_leaf() {
    let qm = query_mgr();

    let deps = qm.transitive_dependents("ur.o79g.0.b.1").unwrap();
    assert!(deps.is_empty());
}

#[test]
fn epic_rollup_not_all_closed() {
    let qm = query_mgr();

    assert!(!qm.epic_all_children_closed("ur.o79g.0.a").unwrap());
}

#[test]
fn epic_rollup_all_closed() {
    let qm = query_mgr();

    qm.db()
        .run(
            r#"
        ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [
            ["ur.o79g.0.a.0", "task", "closed", 2, "ur.o79g.0.a",
             "Define ticket relation", "Create the ticket stored relation.",
             "2026-03-11T09:30:00Z", "2026-03-12T08:00:00Z"],
            ["ur.o79g.0.a.1", "task", "closed", 2, "ur.o79g.0.a",
             "Define metadata relations", "Create ticket_meta and activity_meta.",
             "2026-03-11T09:35:00Z", "2026-03-12T09:00:00Z"],
            ["ur.o79g.0.a.2", "task", "closed", 3, "ur.o79g.0.a",
             "Define dependency relations", "Create blocks and relates_to.",
             "2026-03-11T09:40:00Z", "2026-03-11T09:40:00Z"],
            ["ur.o79g.0.a.3", "bug", "closed", 2, "ur.o79g.0.a",
             "Fix nullable parent_id handling", "Empty string workaround needs validation.",
             "2026-03-12T10:00:00Z", "2026-03-12T10:00:00Z"]
        ]
        :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
    "#,
        )
        .unwrap();

    assert!(qm.epic_all_children_closed("ur.o79g.0.a").unwrap());
}

#[test]
fn epic_rollup_no_children() {
    let qm = query_mgr();

    assert!(qm.epic_all_children_closed("ur.o79g.0.a.0").unwrap());
}

#[test]
fn cycle_detection_no_cycle() {
    let qm = query_mgr();

    assert!(
        !qm.would_create_cycle("ur.o79g.0.a.3", "ur.o79g.0.b.0")
            .unwrap()
    );
}

#[test]
fn cycle_detection_direct_reverse() {
    let qm = query_mgr();

    assert!(
        qm.would_create_cycle("ur.o79g.0.a.1", "ur.o79g.0.a.0")
            .unwrap()
    );
}

#[test]
fn cycle_detection_transitive() {
    let qm = query_mgr();

    assert!(
        qm.would_create_cycle("ur.o79g.0.a.2", "ur.o79g.0.a.0")
            .unwrap()
    );
}

#[test]
fn cycle_detection_cross_epic_transitive() {
    let qm = query_mgr();

    assert!(
        qm.would_create_cycle("ur.o79g.0.b.1", "ur.o79g.0.a.0")
            .unwrap()
    );
}

#[test]
fn cycle_detection_self_loop() {
    let qm = query_mgr();

    assert!(
        qm.would_create_cycle("ur.o79g.0.a.0", "ur.o79g.0.a.0")
            .unwrap()
    );
}

#[test]
fn metadata_query_by_assignee() {
    let qm = query_mgr();

    let tickets = qm.tickets_by_metadata("assignee", "agent-1").unwrap();
    assert_eq!(tickets.len(), 2);

    let ids: Vec<&str> = tickets.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"ur.o79g.0.a.0"));
    assert!(ids.contains(&"ur.o79g.0.b"));
}

#[test]
fn metadata_query_by_tag() {
    let qm = query_mgr();

    let tickets = qm.tickets_by_metadata("tag", "schema").unwrap();
    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].id, "ur.o79g.0.a");
}

#[test]
fn metadata_query_all_tagged_tickets() {
    let qm = query_mgr();

    let tickets = qm.tickets_with_metadata_key("tag").unwrap();
    assert_eq!(tickets.len(), 4);

    let ids: Vec<&str> = tickets.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"ur.o79g.0.a"));
    assert!(ids.contains(&"ur.o79g.0.b"));
    assert!(ids.contains(&"ur.o79g.0.b.0"));
    assert!(ids.contains(&"ur.o79g.0.b.1"));
}

#[test]
fn metadata_query_no_matches() {
    let qm = query_mgr();

    let tickets = qm
        .tickets_by_metadata("assignee", "nonexistent-user")
        .unwrap();
    assert!(tickets.is_empty());
}

/// Verify that dispatchable tickets update correctly when blockers are resolved.
#[test]
fn dispatchable_tickets_update_when_blockers_resolve() {
    let qm = query_mgr();

    let before = qm.dispatchable_tickets("ur.o79g.0.b").unwrap();
    assert_eq!(before.len(), 1);

    qm.db()
        .run(
            r#"
        ?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
            "ur.o79g.0.a.2", "task", "closed", 3, "ur.o79g.0.a",
            "Define dependency relations", "Create blocks and relates_to.",
            "2026-03-11T09:40:00Z", "2026-03-12T12:00:00Z"
        ]]
        :put ticket {id => type, status, priority, parent_id, title, body, created_at, updated_at}
    "#,
        )
        .unwrap();

    let after = qm.dispatchable_tickets("ur.o79g.0.b").unwrap();
    assert_eq!(after.len(), 2);
    let ids: Vec<&str> = after.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"ur.o79g.0.b.0"));
    assert!(ids.contains(&"ur.o79g.0.b.1"));
}
