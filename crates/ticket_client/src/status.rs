use std::collections::HashMap;
use std::fmt::Write;

use ur_rpc::proto::ticket::Ticket;

struct Counts {
    open: usize,
    closed: usize,
}

/// Build a project status report matching the repotools `ticket-status` format.
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

    // parent_id → children
    let mut children: HashMap<&str, Vec<&Ticket>> = HashMap::new();
    for t in &items {
        if !t.parent_id.is_empty() {
            children.entry(t.parent_id.as_str()).or_default().push(t);
        }
    }

    // Open epics by ID
    let mut epics: HashMap<&str, &Ticket> = HashMap::new();
    for t in &items {
        if t.ticket_type == "epic" && t.status != "closed" {
            epics.insert(t.id.as_str(), t);
        }
    }

    // Find top-level epics (not a child of another open epic)
    let mut child_epic_ids: HashMap<&str, bool> = HashMap::new();
    for &pid in epics.keys() {
        let Some(kids) = children.get(pid) else {
            continue;
        };
        for kid in kids.iter().filter(|k| k.ticket_type == "epic") {
            child_epic_ids.insert(kid.id.as_str(), true);
        }
    }
    let mut top_epics: Vec<&Ticket> = epics
        .iter()
        .filter(|(id, _)| !child_epic_ids.contains_key(*id))
        .map(|(_, t)| *t)
        .collect();
    top_epics.sort_by_key(|t| &t.title);

    // Direct sub-epics of a given epic
    let sub_epics_of = |eid: &str| -> Vec<&Ticket> {
        let mut subs: Vec<&Ticket> = children
            .get(eid)
            .map(|kids| {
                kids.iter()
                    .filter(|t| t.ticket_type == "epic" && t.status != "closed")
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        subs.sort_by_key(|t| &t.title);
        subs
    };

    // Count open/closed non-epic descendants (recursive via stack)
    let descendant_counts = |eid: &str| -> Counts {
        let mut c = Counts { open: 0, closed: 0 };
        let mut stack = vec![eid.to_owned()];
        while let Some(id) = stack.pop() {
            let Some(kids) = children.get(id.as_str()) else {
                continue;
            };
            for kid in kids {
                match kid.ticket_type.as_str() {
                    "epic" => stack.push(kid.id.clone()),
                    _ if kid.status == "closed" => c.closed += 1,
                    _ => c.open += 1,
                }
            }
        }
        c
    };

    // Global totals
    let total_open = items.iter().filter(|t| t.status != "closed").count();
    let total = items.len();

    let mut out = String::new();
    writeln!(
        out,
        "Project Status — {today}  ({total_open} open / {total} total)"
    )
    .unwrap();
    writeln!(out).unwrap();

    // Group top-level epics by priority
    let mut epics_by_pri: HashMap<i64, Vec<&Ticket>> = HashMap::new();
    for t in &top_epics {
        epics_by_pri.entry(t.priority).or_default().push(t);
    }
    let mut pris: Vec<i64> = epics_by_pri.keys().copied().collect();
    pris.sort();

    for p in &pris {
        let group = epics_by_pri.get(p).unwrap();
        let mut group: Vec<&Ticket> = group.to_vec();
        group.sort_by_key(|t| &t.title);

        writeln!(out, "[P{p}]").unwrap();

        for ep in &group {
            let c = descendant_counts(&ep.id);
            let counts = format!("{}/{}", c.open, c.open + c.closed);
            writeln!(out, "  {:<12} {:<7} {}", ep.id, counts, ep.title).unwrap();

            for sub in sub_epics_of(&ep.id) {
                let sc = descendant_counts(&sub.id);
                let sub_counts = format!("{}/{}", sc.open, sc.open + sc.closed);
                writeln!(out, "    {:<12} {:<7} {}", sub.id, sub_counts, sub.title).unwrap();
            }
        }
        writeln!(out).unwrap();
    }

    // Orphaned tickets: non-epic, non-closed, with no parent or parent not a known open epic
    let mut orphans: Vec<&Ticket> = items
        .iter()
        .filter(|t| {
            if t.ticket_type == "epic" || t.status == "closed" {
                return false;
            }
            t.parent_id.is_empty() || !epics.contains_key(t.parent_id.as_str())
        })
        .copied()
        .collect();
    orphans.sort_by_key(|t| &t.title);

    if !orphans.is_empty() {
        writeln!(out, "[Orphaned]").unwrap();
        for o in &orphans {
            writeln!(out, "  {:<12} {:<7} {}", o.id, o.status, o.title).unwrap();
        }
        writeln!(out).unwrap();
    }

    // Remove trailing newline
    while out.ends_with('\n') {
        out.pop();
    }
    out
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
        }
    }

    #[test]
    fn empty_tickets() {
        let report = build_status_report(&[], "2026-03-15", None);
        assert_eq!(report, "Project Status — 2026-03-15  (0 open / 0 total)");
    }

    #[test]
    fn epic_with_children() {
        let tickets = vec![
            ticket("ur-e1", "Epic One", "epic", "open", 1, ""),
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
    fn nested_epics() {
        let tickets = vec![
            ticket("ur-e1", "Parent Epic", "epic", "open", 0, ""),
            ticket("ur-e2", "Child Epic", "epic", "open", 0, "ur-e1"),
            ticket("ur-t1", "Task", "task", "open", 0, "ur-e2"),
        ];
        let report = build_status_report(&tickets, "2026-03-15", None);
        // ur-e1 is top-level, ur-e2 is sub-epic
        assert!(report.contains("  ur-e1"));
        assert!(report.contains("    ur-e2"));
        // ur-e1 descendant count includes ur-t1 (through ur-e2)
        assert!(report.contains("1/1")); // 1 open descendant, 1 total
    }

    #[test]
    fn orphaned_tickets() {
        let tickets = vec![ticket("ur-t1", "Orphan Task", "task", "open", 0, "")];
        let report = build_status_report(&tickets, "2026-03-15", None);
        assert!(report.contains("[Orphaned]"));
        assert!(report.contains("ur-t1"));
        assert!(report.contains("open"));
    }

    #[test]
    fn project_filter() {
        let tickets = vec![
            ticket("ur-e1", "Ur Epic", "epic", "open", 1, ""),
            ticket("ur-t1", "Ur Task", "task", "open", 0, "ur-e1"),
            ticket("foo-e1", "Foo Epic", "epic", "open", 1, ""),
            ticket("foo-t1", "Foo Task", "task", "open", 0, "foo-e1"),
        ];
        let report = build_status_report(&tickets, "2026-03-15", Some("ur"));
        assert!(report.contains("ur-e1"));
        assert!(!report.contains("foo-e1"));
        assert!(report.contains("(2 open / 2 total)"));
    }

    #[test]
    fn closed_epics_excluded() {
        let tickets = vec![
            ticket("ur-e1", "Closed Epic", "epic", "closed", 1, ""),
            ticket("ur-e2", "Open Epic", "epic", "open", 1, ""),
        ];
        let report = build_status_report(&tickets, "2026-03-15", None);
        // Closed epics don't appear in the tree
        assert!(!report.contains("Closed Epic"));
        assert!(report.contains("Open Epic"));
    }
}
