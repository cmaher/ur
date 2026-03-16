// Tests for GraphManager.

use crate::graph::GraphManager;
use crate::model::{EdgeKind, NewTicket};
use crate::tests::TestDb;
use crate::ticket_repo::TicketRepo;

/// Build a TicketRepo and GraphManager from a TestDb.
fn managers(db: &TestDb) -> (TicketRepo, GraphManager) {
    let pool = db.db().pool().clone();
    let graph = GraphManager::new(pool.clone());
    let repo = TicketRepo::new(pool, graph.clone());
    (repo, graph)
}

/// Create a minimal ticket via TicketRepo.
async fn create_ticket(repo: &TicketRepo, id: &str) {
    repo.create_ticket(&NewTicket {
        id: id.into(),
        type_: "task".into(),
        priority: 1,
        parent_id: None,
        title: format!("Ticket {id}"),
        body: String::new(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();
}

// ============================================================
// transitive_blockers
// ============================================================

#[tokio::test]
async fn transitive_blockers_single_hop() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    create_ticket(&repo, "a").await;
    create_ticket(&repo, "b").await;

    // a blocks b
    repo.add_edge("a", "b", EdgeKind::Blocks).await.unwrap();

    let blockers = graph.transitive_blockers("b").await.unwrap();
    assert_eq!(blockers, vec!["a"]);

    // a has no blockers
    let blockers_a = graph.transitive_blockers("a").await.unwrap();
    assert!(blockers_a.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn transitive_blockers_multi_hop() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    create_ticket(&repo, "a").await;
    create_ticket(&repo, "b").await;
    create_ticket(&repo, "c").await;

    // a blocks b, b blocks c => c is transitively blocked by a and b
    repo.add_edge("a", "b", EdgeKind::Blocks).await.unwrap();
    repo.add_edge("b", "c", EdgeKind::Blocks).await.unwrap();

    let mut blockers = graph.transitive_blockers("c").await.unwrap();
    blockers.sort();
    assert_eq!(blockers, vec!["a", "b"]);

    // b is only blocked by a
    let blockers_b = graph.transitive_blockers("b").await.unwrap();
    assert_eq!(blockers_b, vec!["a"]);

    db.cleanup().await;
}

#[tokio::test]
async fn transitive_blockers_cross_epic() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    // Create tickets in two different epics
    repo.create_ticket(&NewTicket {
        id: "epic-x".into(),
        type_: "epic".into(),
        priority: 1,
        parent_id: None,
        title: "Epic X".into(),
        body: String::new(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: "epic-y".into(),
        type_: "epic".into(),
        priority: 1,
        parent_id: None,
        title: "Epic Y".into(),
        body: String::new(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: "x-task".into(),
        type_: "task".into(),
        priority: 1,
        parent_id: Some("epic-x".into()),
        title: "Task in X".into(),
        body: String::new(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: "y-task".into(),
        type_: "task".into(),
        priority: 1,
        parent_id: Some("epic-y".into()),
        title: "Task in Y".into(),
        body: String::new(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // x-task blocks y-task (cross-epic)
    repo.add_edge("x-task", "y-task", EdgeKind::Blocks)
        .await
        .unwrap();

    let blockers = graph.transitive_blockers("y-task").await.unwrap();
    assert_eq!(blockers, vec!["x-task"]);

    db.cleanup().await;
}

#[tokio::test]
async fn transitive_blockers_nonexistent_ticket() {
    let db = TestDb::new().await;
    let (_repo, graph) = managers(&db);

    let blockers = graph.transitive_blockers("no-such-ticket").await.unwrap();
    assert!(blockers.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn transitive_blockers_ignores_relates_to_edges() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    create_ticket(&repo, "a").await;
    create_ticket(&repo, "b").await;

    // relates_to should not appear as a blocker
    repo.add_edge("a", "b", EdgeKind::RelatesTo).await.unwrap();

    let blockers = graph.transitive_blockers("b").await.unwrap();
    assert!(blockers.is_empty());

    db.cleanup().await;
}

// ============================================================
// transitive_dependents
// ============================================================

#[tokio::test]
async fn transitive_dependents_single_hop() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    create_ticket(&repo, "a").await;
    create_ticket(&repo, "b").await;

    // a blocks b => b depends on a => a's dependent is b
    repo.add_edge("a", "b", EdgeKind::Blocks).await.unwrap();

    let dependents = graph.transitive_dependents("a").await.unwrap();
    assert_eq!(dependents, vec!["b"]);

    // b has no dependents
    let dependents_b = graph.transitive_dependents("b").await.unwrap();
    assert!(dependents_b.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn transitive_dependents_multi_hop() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    create_ticket(&repo, "a").await;
    create_ticket(&repo, "b").await;
    create_ticket(&repo, "c").await;
    create_ticket(&repo, "d").await;

    // a blocks b, b blocks c, b blocks d
    repo.add_edge("a", "b", EdgeKind::Blocks).await.unwrap();
    repo.add_edge("b", "c", EdgeKind::Blocks).await.unwrap();
    repo.add_edge("b", "d", EdgeKind::Blocks).await.unwrap();

    let mut dependents = graph.transitive_dependents("a").await.unwrap();
    dependents.sort();
    assert_eq!(dependents, vec!["b", "c", "d"]);

    // b's dependents are c and d
    let mut dependents_b = graph.transitive_dependents("b").await.unwrap();
    dependents_b.sort();
    assert_eq!(dependents_b, vec!["c", "d"]);

    db.cleanup().await;
}

#[tokio::test]
async fn transitive_dependents_nonexistent_ticket() {
    let db = TestDb::new().await;
    let (_repo, graph) = managers(&db);

    let dependents = graph.transitive_dependents("ghost").await.unwrap();
    assert!(dependents.is_empty());

    db.cleanup().await;
}

// ============================================================
// would_create_cycle
// ============================================================

#[tokio::test]
async fn would_create_cycle_self_loop() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    create_ticket(&repo, "a").await;

    // a -> a is always a cycle
    let result = graph.would_create_cycle("a", "a").await.unwrap();
    assert!(result);

    db.cleanup().await;
}

#[tokio::test]
async fn would_create_cycle_direct_cycle() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    create_ticket(&repo, "a").await;
    create_ticket(&repo, "b").await;

    // a blocks b already exists
    repo.add_edge("a", "b", EdgeKind::Blocks).await.unwrap();

    // Adding b -> a would create a cycle
    let result = graph.would_create_cycle("b", "a").await.unwrap();
    assert!(result);

    db.cleanup().await;
}

#[tokio::test]
async fn would_create_cycle_transitive_cycle() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    create_ticket(&repo, "a").await;
    create_ticket(&repo, "b").await;
    create_ticket(&repo, "c").await;

    // a -> b -> c exists
    repo.add_edge("a", "b", EdgeKind::Blocks).await.unwrap();
    repo.add_edge("b", "c", EdgeKind::Blocks).await.unwrap();

    // Adding c -> a would create a transitive cycle
    let result = graph.would_create_cycle("c", "a").await.unwrap();
    assert!(result);

    db.cleanup().await;
}

#[tokio::test]
async fn would_create_cycle_no_cycle() {
    let db = TestDb::new().await;
    let (repo, graph) = managers(&db);

    create_ticket(&repo, "a").await;
    create_ticket(&repo, "b").await;
    create_ticket(&repo, "c").await;

    // a -> b exists
    repo.add_edge("a", "b", EdgeKind::Blocks).await.unwrap();

    // Adding a -> c does NOT create a cycle
    let result = graph.would_create_cycle("a", "c").await.unwrap();
    assert!(!result);

    // Adding c -> b does NOT create a cycle
    let result = graph.would_create_cycle("c", "b").await.unwrap();
    assert!(!result);

    db.cleanup().await;
}

#[tokio::test]
async fn would_create_cycle_nonexistent_nodes() {
    let db = TestDb::new().await;
    let (_repo, graph) = managers(&db);

    // Non-existent nodes can't form a cycle
    let result = graph.would_create_cycle("ghost1", "ghost2").await.unwrap();
    assert!(!result);

    db.cleanup().await;
}
