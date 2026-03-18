use ur_rpc::proto::ticket::{ActivityEntry, MetadataEntry, Ticket};

use super::format::{format_ticket_detail, format_ticket_list};

fn sample_ticket(id: &str, title: &str) -> Ticket {
    Ticket {
        id: id.to_owned(),
        ticket_type: "task".to_owned(),
        status: "open".to_owned(),
        priority: 1,
        parent_id: String::new(),
        title: title.to_owned(),
        body: String::new(),
        created_at: "2026-03-15T10:00:00Z".to_owned(),
        updated_at: "2026-03-15T10:00:00Z".to_owned(),
        project: "test".to_owned(),
        lifecycle_status: String::new(),
        branch: String::new(),
        lifecycle_managed: false,
    }
}

#[test]
fn format_detail_minimal() {
    let t = sample_ticket("ur-abc12", "Test ticket");
    let out = format_ticket_detail(&t, &[], &[]);
    assert!(out.contains("ID:       ur-abc12"));
    assert!(out.contains("Title:    Test ticket"));
    assert!(out.contains("Type:     task"));
    assert!(out.contains("Status:   open"));
    assert!(out.contains("Priority: 1"));
    assert!(out.contains("Created:  2026-03-15T10:00:00Z"));
    assert!(out.contains("Updated:  2026-03-15T10:00:00Z"));
    // No parent, body, metadata, or activity sections
    assert!(!out.contains("Parent:"));
    assert!(!out.contains("Metadata:"));
    assert!(!out.contains("Activity:"));
}

#[test]
fn format_detail_with_parent() {
    let t = Ticket {
        parent_id: "ur-epic1".to_owned(),
        ..sample_ticket("ur-child1", "Child ticket")
    };
    let out = format_ticket_detail(&t, &[], &[]);
    assert!(out.contains("Parent:   ur-epic1"));
}

#[test]
fn format_detail_with_body() {
    let t = Ticket {
        body: "This is the body text.".to_owned(),
        ..sample_ticket("ur-abc12", "With body")
    };
    let out = format_ticket_detail(&t, &[], &[]);
    assert!(out.contains("This is the body text."));
}

#[test]
fn format_detail_with_metadata() {
    let t = sample_ticket("ur-abc12", "With meta");
    let meta = vec![
        MetadataEntry {
            key: "env".to_owned(),
            value: "prod".to_owned(),
        },
        MetadataEntry {
            key: "team".to_owned(),
            value: "alpha".to_owned(),
        },
    ];
    let out = format_ticket_detail(&t, &meta, &[]);
    assert!(out.contains("Metadata:"));
    assert!(out.contains("  env: prod"));
    assert!(out.contains("  team: alpha"));
}

#[test]
fn format_detail_with_activities() {
    let t = sample_ticket("ur-abc12", "With activities");
    let activities = vec![ActivityEntry {
        id: "act-1".to_owned(),
        timestamp: "2026-03-15T12:00:00Z".to_owned(),
        author: "worker".to_owned(),
        message: "did some work".to_owned(),
    }];
    let out = format_ticket_detail(&t, &[], &activities);
    assert!(out.contains("Activity:"));
    assert!(out.contains("[2026-03-15T12:00:00Z] worker: did some work"));
}

#[test]
fn format_detail_full() {
    let t = Ticket {
        parent_id: "ur-epic1".to_owned(),
        body: "Full body here.".to_owned(),
        ..sample_ticket("ur-abc12", "Full ticket")
    };
    let meta = vec![MetadataEntry {
        key: "component".to_owned(),
        value: "backend".to_owned(),
    }];
    let activities = vec![ActivityEntry {
        id: "act-1".to_owned(),
        timestamp: "2026-03-15T12:00:00Z".to_owned(),
        author: "worker".to_owned(),
        message: "deployed".to_owned(),
    }];
    let out = format_ticket_detail(&t, &meta, &activities);
    assert!(out.contains("ID:       ur-abc12"));
    assert!(out.contains("Parent:   ur-epic1"));
    assert!(out.contains("Full body here."));
    assert!(out.contains("Metadata:"));
    assert!(out.contains("  component: backend"));
    assert!(out.contains("Activity:"));
    assert!(out.contains("[2026-03-15T12:00:00Z] worker: deployed"));
}

#[test]
fn format_list_empty() {
    let out = format_ticket_list(&[]);
    assert!(out.contains("ID"));
    assert!(out.contains("TYPE"));
    assert!(out.contains("STATUS"));
    assert!(out.contains("PRI"));
    assert!(out.contains("TITLE"));
    assert!(out.contains("0 ticket(s)"));
}

#[test]
fn format_list_multiple() {
    let tickets = vec![
        sample_ticket("ur-abc12", "First ticket"),
        Ticket {
            priority: 3,
            status: "closed".to_owned(),
            ticket_type: "epic".to_owned(),
            ..sample_ticket("ur-def34", "Second ticket")
        },
    ];
    let out = format_ticket_list(&tickets);
    assert!(out.contains("ur-abc12"));
    assert!(out.contains("First ticket"));
    assert!(out.contains("ur-def34"));
    assert!(out.contains("Second ticket"));
    assert!(out.contains("2 ticket(s)"));
    // Verify column values appear
    assert!(out.contains("epic"));
    assert!(out.contains("closed"));
}

#[test]
fn format_detail_no_trailing_newline() {
    let t = sample_ticket("ur-abc12", "Test");
    let out = format_ticket_detail(&t, &[], &[]);
    assert!(
        !out.ends_with('\n'),
        "format_ticket_detail output should not end with a newline"
    );
}
