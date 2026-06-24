use std::fmt::Write;

use ur_rpc::proto::ticket::{
    ActivityDetail, ActivityEntry, DispatchableTicket, MetadataEntry, Ticket,
};
use ur_rpc::ticket_meta;

use super::EdgeEntry;

/// Format a single ticket's full detail view (used by `show`).
pub fn format_ticket_detail(
    ticket: &Ticket,
    metadata: &[MetadataEntry],
    activities: &[ActivityEntry],
    edges: &[EdgeEntry],
) -> String {
    let mut out = String::new();
    writeln!(out, "ID:       {}", ticket.id).unwrap();
    if !ticket.project.is_empty() {
        writeln!(out, "Project:  {}", ticket.project).unwrap();
    }
    writeln!(out, "Title:    {}", ticket.title).unwrap();
    writeln!(out, "Type:     {}", ticket.ticket_type).unwrap();
    writeln!(out, "Status:   {}", ticket.status).unwrap();
    writeln!(out, "Priority: {}", ticket.priority).unwrap();
    if let Some(ref_val) = metadata
        .iter()
        .find(|m| m.key == ticket_meta::REF)
        .map(|m| m.value.trim())
        .filter(|v| !v.is_empty())
    {
        writeln!(out, "Ref:      {ref_val}").unwrap();
    }
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
    format_edges(&mut out, edges);
    // Remove the trailing newline that writeln always adds
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

fn format_edges(out: &mut String, edges: &[EdgeEntry]) {
    let blocks: Vec<_> = edges
        .iter()
        .filter(|e| e.relation == ur_rpc::edge_relation::BLOCKS)
        .collect();
    let blocked_by: Vec<_> = edges
        .iter()
        .filter(|e| e.relation == ur_rpc::edge_relation::BLOCKED_BY)
        .collect();
    let relates_to: Vec<_> = edges
        .iter()
        .filter(|e| e.relation == ur_rpc::edge_relation::RELATES_TO)
        .collect();

    if !blocks.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "Blocks:").unwrap();
        for e in blocks {
            writeln!(out, "  {}", e.other_id).unwrap();
        }
    }
    if !blocked_by.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "Blocked by:").unwrap();
        for e in blocked_by {
            writeln!(out, "  {}", e.other_id).unwrap();
        }
    }
    if !relates_to.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "Related to:").unwrap();
        for e in relates_to {
            writeln!(out, "  {}", e.other_id).unwrap();
        }
    }
}

/// Format a table of tickets (used by `list`).
pub fn format_ticket_list(tickets: &[Ticket]) -> String {
    // If any ticket has depth > 0, render as a tree with indentation.
    let is_tree = tickets.iter().any(|t| t.depth > 0);
    if is_tree {
        return format_ticket_tree(tickets);
    }

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

/// Format tickets as an indented tree (used by `list --tree`).
fn format_ticket_tree(tickets: &[Ticket]) -> String {
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
        let indent = "  ".repeat(t.depth as usize);
        writeln!(
            out,
            "{:<20} {:<10} {:<14} {:<4} {}{}",
            t.id, t.ticket_type, t.status, t.priority, indent, t.title
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ticket::EdgeEntry;

    fn bare_ticket() -> Ticket {
        Ticket {
            id: "t-1".into(),
            ticket_type: "code".into(),
            status: "open".into(),
            priority: 0,
            parent_id: String::new(),
            title: "Test ticket".into(),
            body: String::new(),
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: "2024-01-01T00:00:00Z".into(),
            project: "test".into(),
            branch: String::new(),
            depth: 0,
            children_completed: 0,
            children_total: 0,
            dispatch_status: String::new(),
        }
    }

    #[test]
    fn format_edges_all_groups() {
        let edges = vec![
            EdgeEntry {
                other_id: "t-blocked".into(),
                relation: "blocks".into(),
            },
            EdgeEntry {
                other_id: "t-blocker".into(),
                relation: "blocked_by".into(),
            },
            EdgeEntry {
                other_id: "t-related".into(),
                relation: "relates_to".into(),
            },
        ];
        let out = format_ticket_detail(&bare_ticket(), &[], &[], &edges);
        assert!(
            out.contains("Blocks:\n  t-blocked"),
            "missing Blocks section"
        );
        assert!(
            out.contains("Blocked by:\n  t-blocker"),
            "missing Blocked by section"
        );
        assert!(
            out.contains("Related to:\n  t-related"),
            "missing Related to section"
        );
    }

    #[test]
    fn format_edges_empty_groups_omitted() {
        let edges = vec![EdgeEntry {
            other_id: "t-blocked".into(),
            relation: "blocks".into(),
        }];
        let out = format_ticket_detail(&bare_ticket(), &[], &[], &edges);
        assert!(out.contains("Blocks:"), "Blocks section missing");
        assert!(!out.contains("Blocked by:"), "Blocked by should be omitted");
        assert!(!out.contains("Related to:"), "Related to should be omitted");
    }

    #[test]
    fn format_no_edges() {
        let out = format_ticket_detail(&bare_ticket(), &[], &[], &[]);
        assert!(!out.contains("Blocks:"));
        assert!(!out.contains("Blocked by:"));
        assert!(!out.contains("Related to:"));
    }
}
