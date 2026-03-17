use std::fmt::Write;

use ur_rpc::proto::ticket::{
    ActivityDetail, ActivityEntry, DispatchableTicket, MetadataEntry, Ticket,
};

/// Format a single ticket's full detail view (used by `show`).
pub fn format_ticket_detail(
    ticket: &Ticket,
    metadata: &[MetadataEntry],
    activities: &[ActivityEntry],
) -> String {
    let mut out = String::new();
    writeln!(out, "ID:       {}", ticket.id).unwrap();
    if !ticket.project.is_empty() {
        writeln!(out, "Project:  {}", ticket.project).unwrap();
    }
    writeln!(out, "Title:    {}", ticket.title).unwrap();
    writeln!(out, "Type:     {}", ticket.ticket_type).unwrap();
    writeln!(out, "Status:   {}", ticket.status).unwrap();
    if !ticket.lifecycle_status.is_empty() {
        writeln!(out, "Lifecycle: {}", ticket.lifecycle_status).unwrap();
    }
    writeln!(out, "Priority: {}", ticket.priority).unwrap();
    if !ticket.parent_id.is_empty() {
        writeln!(out, "Parent:   {}", ticket.parent_id).unwrap();
    }
    if !ticket.branch.is_empty() {
        writeln!(out, "Branch:   {}", ticket.branch).unwrap();
    }
    writeln!(out, "Created:  {}", ticket.created_at).unwrap();
    writeln!(out, "Updated:  {}", ticket.updated_at).unwrap();
    if !ticket.body.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "{}", ticket.body).unwrap();
    }
    if !metadata.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "Metadata:").unwrap();
        for m in metadata {
            writeln!(out, "  {}: {}", m.key, m.value).unwrap();
        }
    }
    if !activities.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "Activity:").unwrap();
        for a in activities {
            writeln!(out, "  [{}] {}: {}", a.timestamp, a.author, a.message).unwrap();
        }
    }
    // Remove the trailing newline that writeln always adds
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Format a table of tickets (used by `list`).
pub fn format_ticket_list(tickets: &[Ticket]) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "{:<20} {:<10} {:<14} {:<4} TITLE",
        "ID", "TYPE", "STATUS", "PRI"
    )
    .unwrap();
    let separator: String = std::iter::repeat_n('-', 72).collect();
    writeln!(out, "{separator}").unwrap();
    for t in tickets {
        writeln!(
            out,
            "{:<20} {:<10} {:<14} {:<4} {}",
            t.id, t.ticket_type, t.status, t.priority, t.title
        )
        .unwrap();
    }
    write!(out, "\n{} ticket(s)", tickets.len()).unwrap();
    out
}

/// Format activity details (used by `list-activities`).
pub fn format_activities(activities: &[ActivityDetail]) -> String {
    let mut out = String::new();
    for a in activities {
        let Some(entry) = &a.entry else {
            continue;
        };
        write!(
            out,
            "[{}] {}: {}",
            entry.timestamp, entry.author, entry.message
        )
        .unwrap();
        for m in &a.metadata {
            write!(out, "\n  {}: {}", m.key, m.value).unwrap();
        }
        writeln!(out).unwrap();
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Format dispatchable tickets table.
pub fn format_dispatchable(tickets: &[DispatchableTicket]) -> String {
    let mut out = String::new();
    writeln!(out, "{:<20} {:<4} TITLE", "ID", "PRI").unwrap();
    let separator: String = std::iter::repeat_n('-', 48).collect();
    writeln!(out, "{separator}").unwrap();
    for t in tickets {
        writeln!(out, "{:<20} {:<4} {}", t.id, t.priority, t.title).unwrap();
    }
    write!(out, "\n{} dispatchable ticket(s)", tickets.len()).unwrap();
    out
}
