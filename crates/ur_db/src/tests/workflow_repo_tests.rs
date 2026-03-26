// Tests for WorkflowRepo.

use crate::graph::GraphManager;
use crate::model::{LifecycleStatus, NewTicket};
use crate::tests::TestDb;
use crate::ticket_repo::TicketRepo;
use crate::workflow_repo::WorkflowRepo;

/// Build a TicketRepo from a TestDb (used only to create test tickets).
fn ticket_repo(db: &TestDb) -> TicketRepo {
    let pool = db.db().pool().clone();
    let graph_manager = GraphManager::new(pool.clone());
    TicketRepo::new(pool, graph_manager)
}

/// Build a WorkflowRepo from a TestDb.
fn wf_repo(db: &TestDb) -> WorkflowRepo {
    WorkflowRepo::new(db.db().pool().clone())
}

// ============================================================
// Workflow CRUD tests
// ============================================================

#[tokio::test]
async fn create_and_get_workflow() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-t1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Workflow test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let created = wf
        .create_workflow("wf-t1", LifecycleStatus::Open)
        .await
        .unwrap();
    assert_eq!(created.ticket_id, "wf-t1");
    assert_eq!(created.status, LifecycleStatus::Open);
    assert!(!created.id.is_empty());

    let fetched = wf.get_workflow_by_ticket("wf-t1").await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.status, LifecycleStatus::Open);

    db.cleanup().await;
}

#[tokio::test]
async fn get_workflow_returns_none_when_missing() {
    let db = TestDb::new().await;
    let wf = wf_repo(&db);

    let result = wf.get_workflow_by_ticket("no-such").await.unwrap();
    assert!(result.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn create_workflow_allows_multiple_per_ticket() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-dup".into()),
        type_: "task".into(),
        priority: 1,
        title: "Dup test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // First workflow — mark it done (terminal).
    wf.create_workflow("wf-dup", LifecycleStatus::Open)
        .await
        .unwrap();
    wf.update_workflow_status("wf-dup", LifecycleStatus::Done)
        .await
        .unwrap();

    // Second workflow for the same ticket should succeed.
    let wf2 = wf
        .create_workflow("wf-dup", LifecycleStatus::Implementing)
        .await
        .unwrap();
    assert_eq!(wf2.status, LifecycleStatus::Implementing);

    // Active-only query should return the new (non-terminal) workflow.
    let active = wf.get_workflow_by_ticket("wf-dup").await.unwrap().unwrap();
    assert_eq!(active.id, wf2.id);

    db.cleanup().await;
}

#[tokio::test]
async fn update_workflow_status() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-upd".into()),
        type_: "task".into(),
        priority: 1,
        title: "Update wf".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.create_workflow("wf-upd", LifecycleStatus::Open)
        .await
        .unwrap();

    wf.update_workflow_status("wf-upd", LifecycleStatus::Implementing)
        .await
        .unwrap();

    let fetched = wf.get_workflow_by_ticket("wf-upd").await.unwrap().unwrap();
    assert_eq!(fetched.status, LifecycleStatus::Implementing);

    db.cleanup().await;
}

#[tokio::test]
async fn mark_workflow_done() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-done".into()),
        type_: "task".into(),
        priority: 1,
        title: "Done wf".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.create_workflow("wf-done", LifecycleStatus::Open)
        .await
        .unwrap();

    wf.update_workflow_status("wf-done", LifecycleStatus::Done)
        .await
        .unwrap();

    // Active-only query should return None for terminal workflows.
    let active = wf.get_workflow_by_ticket("wf-done").await.unwrap();
    assert!(active.is_none());

    // Latest query should still return the Done workflow.
    let latest = wf
        .get_latest_workflow_by_ticket("wf-done")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.status, LifecycleStatus::Done);

    db.cleanup().await;
}

// ============================================================
// WorkflowIntent CRUD tests
// ============================================================

#[tokio::test]
async fn create_and_poll_intent() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("int-t1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Intent test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let intent = wf
        .create_intent("int-t1", LifecycleStatus::Implementing)
        .await
        .unwrap();
    assert_eq!(intent.ticket_id, "int-t1");
    assert_eq!(intent.target_status, LifecycleStatus::Implementing);

    let polled = wf.poll_intent().await.unwrap().unwrap();
    assert_eq!(polled.id, intent.id);
    assert_eq!(polled.target_status, LifecycleStatus::Implementing);

    db.cleanup().await;
}

#[tokio::test]
async fn poll_intent_returns_none_when_empty() {
    let db = TestDb::new().await;
    let wf = wf_repo(&db);

    let result = wf.poll_intent().await.unwrap();
    assert!(result.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn delete_intent() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("int-del".into()),
        type_: "task".into(),
        priority: 1,
        title: "Del intent".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let intent = wf
        .create_intent("int-del", LifecycleStatus::Pushing)
        .await
        .unwrap();

    wf.delete_intent(&intent.id).await.unwrap();

    let polled = wf.poll_intent().await.unwrap();
    assert!(polled.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn poll_intent_returns_oldest_first() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("int-ord1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Order 1".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("int-ord2".into()),
        type_: "task".into(),
        priority: 1,
        title: "Order 2".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let first = wf
        .create_intent("int-ord1", LifecycleStatus::Implementing)
        .await
        .unwrap();
    wf.create_intent("int-ord2", LifecycleStatus::Pushing)
        .await
        .unwrap();

    let polled = wf.poll_intent().await.unwrap().unwrap();
    assert_eq!(polled.id, first.id);
    assert_eq!(polled.ticket_id, "int-ord1");

    // Delete first, poll should return second
    wf.delete_intent(&first.id).await.unwrap();
    let polled2 = wf.poll_intent().await.unwrap().unwrap();
    assert_eq!(polled2.ticket_id, "int-ord2");

    db.cleanup().await;
}

// ============================================================
// Workflow stall and lifecycle column tests
// ============================================================

#[tokio::test]
async fn workflow_new_has_default_stall_fields() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-stall1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Stall test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let created = wf
        .create_workflow("wf-stall1", LifecycleStatus::Open)
        .await
        .unwrap();

    assert!(!created.stalled);
    assert_eq!(created.stall_reason, "");
    assert_eq!(created.implement_cycles, 0);
    assert_eq!(created.worker_id, "");
    assert!(!created.noverify);
    assert_eq!(created.feedback_mode, "");

    // Verify defaults are returned by get_workflow_by_ticket too.
    let fetched = wf
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
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-stall2".into()),
        type_: "task".into(),
        priority: 1,
        title: "Stall set/clear".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.create_workflow("wf-stall2", LifecycleStatus::Implementing)
        .await
        .unwrap();

    wf.set_workflow_stalled("wf-stall2", "handler failed: timeout")
        .await
        .unwrap();

    let stalled = wf
        .get_workflow_by_ticket("wf-stall2")
        .await
        .unwrap()
        .unwrap();
    assert!(stalled.stalled);
    assert_eq!(stalled.stall_reason, "handler failed: timeout");

    wf.clear_workflow_stall("wf-stall2").await.unwrap();

    let cleared = wf
        .get_workflow_by_ticket("wf-stall2")
        .await
        .unwrap()
        .unwrap();
    assert!(!cleared.stalled);
    assert_eq!(cleared.stall_reason, "");

    db.cleanup().await;
}

#[tokio::test]
async fn increment_implement_cycles() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-cyc1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Cycle test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.create_workflow("wf-cyc1", LifecycleStatus::Implementing)
        .await
        .unwrap();

    wf.increment_implement_cycles("wf-cyc1").await.unwrap();
    wf.increment_implement_cycles("wf-cyc1").await.unwrap();
    wf.increment_implement_cycles("wf-cyc1").await.unwrap();

    let fetched = wf.get_workflow_by_ticket("wf-cyc1").await.unwrap().unwrap();
    assert_eq!(fetched.implement_cycles, 3);

    db.cleanup().await;
}

#[tokio::test]
async fn set_workflow_worker_id() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-wid1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Worker id test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.create_workflow("wf-wid1", LifecycleStatus::Implementing)
        .await
        .unwrap();

    wf.set_workflow_worker_id("wf-wid1", "worker-abc123")
        .await
        .unwrap();

    let fetched = wf.get_workflow_by_ticket("wf-wid1").await.unwrap().unwrap();
    assert_eq!(fetched.worker_id, "worker-abc123");

    db.cleanup().await;
}

#[tokio::test]
async fn set_workflow_noverify() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-nv1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Noverify test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.create_workflow("wf-nv1", LifecycleStatus::Implementing)
        .await
        .unwrap();

    wf.set_workflow_noverify("wf-nv1", true).await.unwrap();

    let fetched = wf.get_workflow_by_ticket("wf-nv1").await.unwrap().unwrap();
    assert!(fetched.noverify);

    wf.set_workflow_noverify("wf-nv1", false).await.unwrap();

    let fetched2 = wf.get_workflow_by_ticket("wf-nv1").await.unwrap().unwrap();
    assert!(!fetched2.noverify);

    db.cleanup().await;
}

#[tokio::test]
async fn set_workflow_feedback_mode() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-fb1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Feedback mode test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.create_workflow("wf-fb1", LifecycleStatus::Implementing)
        .await
        .unwrap();

    wf.set_workflow_feedback_mode("wf-fb1", "inline")
        .await
        .unwrap();

    let fetched = wf.get_workflow_by_ticket("wf-fb1").await.unwrap().unwrap();
    assert_eq!(fetched.feedback_mode, "inline");

    db.cleanup().await;
}

// ============================================================
// Workflow events query tests
// ============================================================

#[tokio::test]
async fn get_workflow_events_returns_ordered_events() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-evt1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Events test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let created = wf
        .create_workflow("wf-evt1", LifecycleStatus::Open)
        .await
        .unwrap();

    // Insert events with explicit timestamps to verify ordering.
    wf.insert_workflow_event_at(
        &created.id,
        ur_rpc::workflow_event::WorkflowEvent::Implementing,
        "2025-01-01T00:00:01Z",
    )
    .await
    .unwrap();

    wf.insert_workflow_event_at(
        &created.id,
        ur_rpc::workflow_event::WorkflowEvent::Pushing,
        "2025-01-01T00:00:02Z",
    )
    .await
    .unwrap();

    wf.insert_workflow_event_at(
        &created.id,
        ur_rpc::workflow_event::WorkflowEvent::InReview,
        "2025-01-01T00:00:03Z",
    )
    .await
    .unwrap();

    let events = wf.get_workflow_events(&created.id).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].event, "implementing");
    assert_eq!(events[0].created_at, "2025-01-01T00:00:01Z");
    assert_eq!(events[1].event, "pushing");
    assert_eq!(events[2].event, "in_review");

    db.cleanup().await;
}

#[tokio::test]
async fn get_workflow_events_returns_empty_for_no_events() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("wf-evt2".into()),
        type_: "task".into(),
        priority: 1,
        title: "No events".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let created = wf
        .create_workflow("wf-evt2", LifecycleStatus::Open)
        .await
        .unwrap();

    let events = wf.get_workflow_events(&created.id).await.unwrap();
    assert!(events.is_empty());

    db.cleanup().await;
}

// ============================================================
// Ticket children counts tests
// ============================================================

#[tokio::test]
async fn get_ticket_children_counts_returns_correct_counts() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    // Create a parent ticket.
    repo.create_ticket(&NewTicket {
        id: Some("wf-parent1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Parent".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // Create children: 2 open, 1 closed.
    repo.create_ticket(&NewTicket {
        id: Some("wf-child1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Child 1".into(),
        project: "test".into(),
        parent_id: Some("wf-parent1".into()),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("wf-child2".into()),
        type_: "task".into(),
        priority: 1,
        title: "Child 2".into(),
        project: "test".into(),
        parent_id: Some("wf-parent1".into()),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("wf-child3".into()),
        type_: "task".into(),
        priority: 1,
        title: "Child 3".into(),
        project: "test".into(),
        parent_id: Some("wf-parent1".into()),
        ..Default::default()
    })
    .await
    .unwrap();

    // Close one child.
    repo.update_ticket(
        "wf-child3",
        &crate::TicketUpdate {
            status: Some("closed".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (open, closed) = wf.get_ticket_children_counts("wf-parent1").await.unwrap();
    assert_eq!(open, 2);
    assert_eq!(closed, 1);

    // A ticket with no children should return (0, 0).
    let (open_none, closed_none) = wf.get_ticket_children_counts("wf-child1").await.unwrap();
    assert_eq!(open_none, 0);
    assert_eq!(closed_none, 0);

    db.cleanup().await;
}

// ============================================================
// TicketComments CRUD tests
// ============================================================

#[tokio::test]
async fn insert_ticket_comment_writes_row() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("tc-t1".into()),
        type_: "task".into(),
        priority: 1,
        title: "Ticket comment test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.insert_ticket_comment("comment-1", "tc-t1", 42, "owner/repo")
        .await
        .unwrap();

    let pending = wf.get_pending_replies().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].comment_id, "comment-1");
    assert_eq!(pending[0].ticket_id, "tc-t1");
    assert_eq!(pending[0].pr_number, 42);
    assert_eq!(pending[0].gh_repo, "owner/repo");
    assert!(!pending[0].reply_posted);

    db.cleanup().await;
}

#[tokio::test]
async fn get_pending_replies_excludes_posted() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("tc-t2".into()),
        type_: "task".into(),
        priority: 1,
        title: "Pending replies test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.insert_ticket_comment("comment-a", "tc-t2", 10, "owner/repo")
        .await
        .unwrap();
    wf.insert_ticket_comment("comment-b", "tc-t2", 10, "owner/repo")
        .await
        .unwrap();

    // Mark one as posted.
    wf.mark_reply_posted("comment-a", "tc-t2").await.unwrap();

    let pending = wf.get_pending_replies().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].comment_id, "comment-b");

    db.cleanup().await;
}

#[tokio::test]
async fn mark_reply_posted_flips_flag() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("tc-t3".into()),
        type_: "task".into(),
        priority: 1,
        title: "Mark posted test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    wf.insert_ticket_comment("comment-x", "tc-t3", 7, "owner/repo")
        .await
        .unwrap();

    // Before marking: pending.
    let before = wf.get_pending_replies().await.unwrap();
    assert_eq!(before.len(), 1);

    wf.mark_reply_posted("comment-x", "tc-t3").await.unwrap();

    // After marking: no pending.
    let after = wf.get_pending_replies().await.unwrap();
    assert!(after.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn get_pending_replies_empty_when_none() {
    let db = TestDb::new().await;
    let wf = wf_repo(&db);

    let pending = wf.get_pending_replies().await.unwrap();
    assert!(pending.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn insert_ticket_comment_composite_pk() {
    let db = TestDb::new().await;
    let repo = ticket_repo(&db);
    let wf = wf_repo(&db);

    repo.create_ticket(&NewTicket {
        id: Some("tc-t4".into()),
        type_: "task".into(),
        priority: 1,
        title: "Composite PK test".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    repo.create_ticket(&NewTicket {
        id: Some("tc-t5".into()),
        type_: "task".into(),
        priority: 1,
        title: "Composite PK test 2".into(),
        project: "test".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    // Same comment_id can link to different tickets.
    wf.insert_ticket_comment("comment-shared", "tc-t4", 1, "owner/repo")
        .await
        .unwrap();
    wf.insert_ticket_comment("comment-shared", "tc-t5", 1, "owner/repo")
        .await
        .unwrap();

    let pending = wf.get_pending_replies().await.unwrap();
    assert_eq!(pending.len(), 2);

    db.cleanup().await;
}
