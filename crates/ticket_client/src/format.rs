use std::fmt::Write;

use ur_rpc::proto::ticket::{ActivityEntry, MetadataEntry, Ticket};

/// Format a single ticket's full detail view (used by `show`).
pub fn format_ticket_detail(
    ticket: &Ticket,
    metadata: &[MetadataEntry],
    activities: &[ActivityEntry],
) -> String {
    let mut out = String::new();
    writeln!(out, "ID:       {}", ticket.id).unwrap();
    writeln!(out, "Title:    {}", ticket.title).unwrap();
    writeln!(out, "Type:     {}", ticket.ticket_type).unwrap();
    writeln!(out, "Status:   {}", ticket.status).unwrap();
    writeln!(out, "Priority: {}", ticket.priority).unwrap();
    if !ticket.parent_id.is_empty() {
        writeln!(out, "Parent:   {}", ticket.parent_id).unwrap();
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
