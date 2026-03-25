// Tests for ancestor UI event propagation triggers.

use crate::graph::GraphManager;
use crate::model::{NewTicket, TicketUpdate};
use crate::tests::TestDb;
use crate::ticket_repo::TicketRepo;
use crate::ui_event_repo::UiEventRepo;

fn repo(db: &TestDb) -> TicketRepo {
    let pool = db.db().pool().clone();
    let graph_manager = GraphManager::new(pool.clone());
    TicketRepo::new(pool, graph_manager)
}

fn ui_repo(db: &TestDb) -> UiEventRepo {
    UiEventRepo::new(db.db().pool().clone())
}

/// Drain all ui_events and return the entity_ids for entity_type = 'ticket'.
async fn drain_ticket_event_ids(ui: &UiEventRepo) -> Vec<String> {
    let events = ui.poll_ui_events().await.unwrap();
    if let Some(max_id) = events.iter().map(|e| e.id).max() {
        ui.delete_ui_events(max_id).await.unwrap();
    }
    events
        .into_iter()
        .filter(|e| e.entity_type == "ticket")
        .map(|e| e.entity_id)
        .collect()
}

#[tokio::test]
async fn update_child_emits_events_for_all_ancestors() {
    let db = TestDb::new().await;
    let tickets = repo(&db);
    let ui = ui_repo(&db);

    // Create grandparent → parent → child hierarchy.
    tickets
        .create_ticket(&NewTicket {
            id: Some("gp-1".into()),
            type_: "task".into(),
            title: "Grandparent".into(),
            project: "test".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    tickets
        .create_ticket(&NewTicket {
            id: Some("par-1".into()),
            type_: "task".into(),
            title: "Parent".into(),
            parent_id: Some("gp-1".into()),
            project: "test".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    tickets
        .create_ticket(&NewTicket {
            id: Some("ch-1".into()),
            type_: "task".into(),
            title: "Child".into(),
            parent_id: Some("par-1".into()),
            project: "test".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // Drain insert events so we start fresh.
    drain_ticket_event_ids(&ui).await;

    // Update the child ticket.
    tickets
        .update_ticket(
            "ch-1",
            &TicketUpdate {
                status: Some("in_progress".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let ids = drain_ticket_event_ids(&ui).await;

    // Should contain events for child, parent, and grandparent.
    assert!(ids.contains(&"ch-1".to_string()), "missing child event");
    assert!(ids.contains(&"par-1".to_string()), "missing parent event");
    assert!(
        ids.contains(&"gp-1".to_string()),
        "missing grandparent event"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn insert_child_emits_events_for_ancestors() {
    let db = TestDb::new().await;
    let tickets = repo(&db);
    let ui = ui_repo(&db);

    // Create parent.
    tickets
        .create_ticket(&NewTicket {
            id: Some("par-2".into()),
            type_: "task".into(),
            title: "Parent".into(),
            project: "test".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // Drain insert events.
    drain_ticket_event_ids(&ui).await;

    // Insert a child under the parent.
    tickets
        .create_ticket(&NewTicket {
            id: Some("ch-2".into()),
            type_: "task".into(),
            title: "Child".into(),
            parent_id: Some("par-2".into()),
            project: "test".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    let ids = drain_ticket_event_ids(&ui).await;

    assert!(ids.contains(&"ch-2".to_string()), "missing child event");
    assert!(ids.contains(&"par-2".to_string()), "missing parent event");

    db.cleanup().await;
}

#[tokio::test]
async fn root_ticket_update_emits_single_event() {
    let db = TestDb::new().await;
    let tickets = repo(&db);
    let ui = ui_repo(&db);

    // Create a root ticket (no parent).
    tickets
        .create_ticket(&NewTicket {
            id: Some("root-1".into()),
            type_: "task".into(),
            title: "Root".into(),
            project: "test".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // Drain insert events.
    drain_ticket_event_ids(&ui).await;

    // Update the root ticket.
    tickets
        .update_ticket(
            "root-1",
            &TicketUpdate {
                status: Some("closed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let ids = drain_ticket_event_ids(&ui).await;

    assert_eq!(ids.len(), 1, "root update should emit exactly one event");
    assert_eq!(ids[0], "root-1");

    db.cleanup().await;
}
