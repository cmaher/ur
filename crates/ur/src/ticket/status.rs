use std::collections::HashMap;
use std::fmt::Write;

use ur_rpc::proto::ticket::Ticket;

struct Counts {
    open: usize,
    in_progress: usize,
    closed: usize,
}

struct ReportData<'a> {
    children: HashMap<&'a str, Vec<&'a Ticket>>,
    ticket_map: HashMap<&'a str, &'a Ticket>,
}

/// Build a project status report showing ticket trees and work overview.
///
/// If `project` is provided, only tickets whose ID starts with `{project}-` are included.
pub fn build_status_report(tickets: &[Ticket], today: &str, project: Option<&str>) -> String {
    let items: Vec<&Ticket> = match project {
        Some(p) => {
            let prefix = format!("{p}-");
            tickets
                .iter()
                .filter(|t| t.id.starts_with(&prefix))
                .collect()
        }
        None => tickets.iter().collect(),
    };

    let data = build_report_data(&items);

    let total_open = items.iter().filter(|t| t.status != "closed").count();
    let total = items.len();

    let mut out = String::new();
    writeln!(
        out,
        "Project Status — {today}  ({total_open} open / {total} total)"
    )
    .unwrap();
    writeln!(out).unwrap();

    render_tree(&mut out, &items, &data);
    render_orphans(&mut out, &items, &data);

    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn build_report_data<'a>(items: &[&'a Ticket]) -> ReportData<'a> {
    let mut children: HashMap<&str, Vec<&Ticket>> = HashMap::new();
    let mut ticket_map: HashMap<&str, &Ticket> = HashMap::new();
    for t in items {
        ticket_map.insert(t.id.as_str(), t);
        if !t.parent_id.is_empty() {
            children.entry(t.parent_id.as_str()).or_default().push(t);
        }
    }

    ReportData {
        children,
        ticket_map,
    }
}

/// Identify root tickets: tickets that have children and are not themselves children
/// of another ticket with children, or tickets with children whose parent is not in
/// the current item set.
fn find_root_parents<'a>(items: &[&'a Ticket], data: &ReportData<'a>) -> Vec<&'a Ticket> {
    let has_children: HashMap<&str, bool> = data
        .children
        .keys()
        .filter(|id| data.ticket_map.contains_key(*id))
        .map(|&id| (id, true))
        .collect();

    let mut roots: Vec<&Ticket> = items
        .iter()
        .filter(|t| {
            // Must have children
            if !has_children.contains_key(t.id.as_str()) {
                return false;
            }
            // Must not be closed
            if t.status == "closed" {
                return false;
            }
            // Is a root if it has no parent, or its parent is not in the item set
            t.parent_id.is_empty() || !data.ticket_map.contains_key(t.parent_id.as_str())
        })
        .copied()
        .collect();
    roots.sort_by_key(|t| &t.title);
    roots
}

fn render_tree(out: &mut String, items: &[&Ticket], data: &ReportData<'_>) {
    let roots = find_root_parents(items, data);

    let mut roots_by_pri: HashMap<i64, Vec<&Ticket>> = HashMap::new();
    for t in &roots {
        roots_by_pri.entry(t.priority).or_default().push(t);
    }
    let mut pris: Vec<i64> = roots_by_pri.keys().copied().collect();
    pris.sort();

    for p in &pris {
        let group = roots_by_pri.get(p).unwrap();
        let mut group: Vec<&Ticket> = group.to_vec();
        group.sort_by_key(|t| &t.title);

        writeln!(out, "[P{p}]").unwrap();

        for root in &group {
            render_tree_node(out, root, data, 1);
        }
        writeln!(out).unwrap();
    }
}

/// Render a single tree node with its descendant counts and recurse into sub-parents.
fn render_tree_node(out: &mut String, ticket: &Ticket, data: &ReportData<'_>, depth: usize) {
    let c = descendant_counts(&data.children, &ticket.id);
    let total = c.open + c.in_progress + c.closed;
    let counts = format!("{}/{}", c.open + c.in_progress, total);
    let indent: String = std::iter::repeat_n(' ', depth * 2).collect();
    writeln!(
        out,
        "{indent}{:<12} {:<7} {}",
        ticket.id, counts, ticket.title
    )
    .unwrap();

    // Render child tickets that themselves have children (sub-trees)
    if let Some(kids) = data.children.get(ticket.id.as_str()) {
        let mut sub_parents: Vec<&Ticket> = kids
            .iter()
            .filter(|k| data.children.contains_key(k.id.as_str()) && k.status != "closed")
            .copied()
            .collect();
        sub_parents.sort_by_key(|t| &t.title);
        for sub in sub_parents {
            render_tree_node(out, sub, data, depth + 1);
        }
    }
}

fn render_orphans(out: &mut String, items: &[&Ticket], data: &ReportData<'_>) {
    // A ticket is orphaned if it:
    // - is not closed
    // - has no children (i.e., it's a leaf)
    // - has no parent, OR its parent is not a root/sub-parent in the tree
    let has_children: HashMap<&str, bool> = data
        .children
        .keys()
        .filter(|id| data.ticket_map.contains_key(*id))
        .map(|&id| (id, true))
        .collect();

    let mut orphans: Vec<&Ticket> = items
        .iter()
        .filter(|t| {
            if t.status == "closed" {
                return false;
            }
            // If this ticket has children, it's rendered as a tree root
            if has_children.contains_key(t.id.as_str()) {
                return false;
            }
            // If it has a parent that is in the ticket set (and that parent has children,
            // which it does by definition since this ticket is a child), it's not orphaned
            if !t.parent_id.is_empty() && data.ticket_map.contains_key(t.parent_id.as_str()) {
                return false;
            }
            true
        })
        .copied()
        .collect();
    orphans.sort_by_key(|t| &t.title);

    if !orphans.is_empty() {
        writeln!(out, "[Unparented]").unwrap();
        for o in &orphans {
            writeln!(out, "  {:<12} {:<7} {}", o.id, o.status, o.title).unwrap();
        }
        writeln!(out).unwrap();
    }
}

fn descendant_counts(children: &HashMap<&str, Vec<&Ticket>>, parent_id: &str) -> Counts {
    let mut c = Counts {
        open: 0,
        in_progress: 0,
        closed: 0,
    };
    let mut stack = vec![parent_id.to_owned()];
    while let Some(id) = stack.pop() {
        let Some(kids) = children.get(id.as_str()) else {
            continue;
        };
        for kid in kids {
            if children.contains_key(kid.id.as_str()) {
                // This child is itself a parent; recurse into it but don't count it directly
                stack.push(kid.id.clone());
            } else {
                match kid.status.as_str() {
                    "closed" => c.closed += 1,
                    "in_progress" => c.in_progress += 1,
                    _ => c.open += 1,
                }
            }
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(
        id: &str,
        title: &str,
        ticket_type: &str,
        status: &str,
        priority: i64,
        parent_id: &str,
    ) -> Ticket {
        Ticket {
            id: id.to_owned(),
            title: title.to_owned(),
            ticket_type: ticket_type.to_owned(),
            status: status.to_owned(),
            priority,
            parent_id: parent_id.to_owned(),
            body: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            project: "test".to_owned(),
            branch: String::new(),
            depth: 0,
        }
    }

    #[test]
    fn empty_tickets() {
        let report = build_status_report(&[], "2026-03-15", None);
        assert_eq!(report, "Project Status — 2026-03-15  (0 open / 0 total)");
    }

    #[test]
    fn parent_with_children() {
        let tickets = vec![
            ticket("ur-e1", "Parent One", "task", "open", 1, ""),
            ticket("ur-t1", "Task One", "task", "open", 0, "ur-e1"),
            ticket("ur-t2", "Task Two", "task", "closed", 0, "ur-e1"),
        ];
        let report = build_status_report(&tickets, "2026-03-15", None);
        assert!(report.contains("(2 open / 3 total)"));
        assert!(report.contains("[P1]"));
        assert!(report.contains("ur-e1"));
        assert!(report.contains("1/2")); // 1 open, 2 total children
    }

    #[test]
    fn nested_parents() {
        let tickets = vec![
            ticket("ur-e1", "Parent", "task", "open", 0, ""),
            ticket("ur-e2", "Child Parent", "task", "open", 0, "ur-e1"),
            ticket("ur-t1", "Task", "task", "open", 0, "ur-e2"),
        ];
        let report = build_status_report(&tickets, "2026-03-15", None);
        // ur-e1 is top-level, ur-e2 is sub-parent
        assert!(report.contains("  ur-e1"));
        assert!(report.contains("    ur-e2"));
        // ur-e1 descendant count includes ur-t1 (through ur-e2)
        assert!(report.contains("1/1")); // 1 open descendant, 1 total
    }

    #[test]
    fn orphaned_tickets() {
        let tickets = vec![ticket("ur-t1", "Orphan Task", "task", "open", 0, "")];
        let report = build_status_report(&tickets, "2026-03-15", None);
        assert!(report.contains("[Unparented]"));
        assert!(report.contains("ur-t1"));
        assert!(report.contains("open"));
    }

    #[test]
    fn project_filter() {
        let tickets = vec![
            ticket("ur-e1", "Ur Parent", "task", "open", 1, ""),
            ticket("ur-t1", "Ur Task", "task", "open", 0, "ur-e1"),
            ticket("foo-e1", "Foo Parent", "task", "open", 1, ""),
            ticket("foo-t1", "Foo Task", "task", "open", 0, "foo-e1"),
        ];
        let report = build_status_report(&tickets, "2026-03-15", Some("ur"));
        assert!(report.contains("ur-e1"));
        assert!(!report.contains("foo-e1"));
        assert!(report.contains("(2 open / 2 total)"));
    }

    #[test]
    fn closed_parents_excluded() {
        let tickets = vec![
            ticket("ur-e1", "Closed Parent", "task", "closed", 1, ""),
            ticket("ur-t1", "Child", "task", "open", 0, "ur-e1"),
            ticket("ur-e2", "Open Parent", "task", "open", 1, ""),
            ticket("ur-t2", "Child 2", "task", "open", 0, "ur-e2"),
        ];
        let report = build_status_report(&tickets, "2026-03-15", None);
        // Closed parents don't appear as tree roots
        assert!(!report.contains("Closed Parent"));
        assert!(report.contains("Open Parent"));
    }

    #[test]
    fn in_progress_counted_as_open() {
        let tickets = vec![
            ticket("ur-e1", "Parent", "task", "open", 1, ""),
            ticket("ur-t1", "In Progress", "task", "in_progress", 0, "ur-e1"),
            ticket("ur-t2", "Open", "task", "open", 0, "ur-e1"),
            ticket("ur-t3", "Closed", "task", "closed", 0, "ur-e1"),
        ];
        let report = build_status_report(&tickets, "2026-03-15", None);
        // 2 open+in_progress / 3 total children
        assert!(report.contains("2/3"));
    }

    #[test]
    fn no_epic_type_references() {
        let tickets = vec![
            ticket("ur-e1", "Parent", "task", "open", 1, ""),
            ticket("ur-t1", "Task", "task", "open", 0, "ur-e1"),
        ];
        let report = build_status_report(&tickets, "2026-03-15", None);
        // Should not contain "epic" anywhere in output
        assert!(
            !report.to_lowercase().contains("epic"),
            "Report should not reference epic type: {report}"
        );
    }
}
