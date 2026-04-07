//! Template generation and front-matter parsing for ticket creation via an external editor.
//!
//! The editor receives a temp file with front matter (title, priority) delimited by `---`
//! from the body. This module generates that template and parses the result back.

/// A ticket parsed from editor output, ready to be sent to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTicket {
    pub project: String,
    pub title: String,
    pub ticket_type: String,
    pub priority: i64,
    pub branch: Option<String>,
    pub body: String,
}

const TITLE_PLACEHOLDER: &str = "<summarize>";
pub const BRANCH_PLACEHOLDER: &str = "<ticket-id>";
const DEFAULT_TICKET_TYPE: &str = "design";

/// Normalize a ticket type string: maps aliases to their canonical form.
///
/// Maps "task" → "code", "epic" → "code", "c" → "code", "d" → "design".
/// All other values pass through unchanged.
pub fn normalize_ticket_type(s: &str) -> String {
    match s.trim() {
        "task" | "epic" | "c" => "code".to_owned(),
        "d" => "design".to_owned(),
        other => other.to_owned(),
    }
}

/// Generate the default template content shown in the editor.
pub fn generate_template() -> String {
    format!(
        "title: {TITLE_PLACEHOLDER}\ntype: {DEFAULT_TICKET_TYPE}\npriority: 0\nbranch: {BRANCH_PLACEHOLDER}\n---\n\n"
    )
}

/// Serialize ticket fields into the frontmatter markdown format used by the editor.
///
/// This is the inverse of [`parse_ticket_file`]: given the individual fields, it
/// produces the same `title: …\npriority: …\nbranch: …\n---\n…` format that
/// the editor template uses. A `None` branch is rendered as the literal
/// `<ticket-id>` placeholder.
pub fn serialize_to_template(
    project: &str,
    title: &str,
    ticket_type: &str,
    priority: i64,
    branch: Option<&str>,
    body: &str,
) -> String {
    let _ = project; // reserved for future use in the template
    let branch_value = branch.unwrap_or(BRANCH_PLACEHOLDER);
    let trimmed_body = body.trim();
    if trimmed_body.is_empty() {
        format!(
            "title: {title}\ntype: {ticket_type}\npriority: {priority}\nbranch: {branch_value}\n---\n\n"
        )
    } else {
        format!(
            "title: {title}\ntype: {ticket_type}\npriority: {priority}\nbranch: {branch_value}\n---\n{trimmed_body}\n"
        )
    }
}

/// Returns `true` if the title is the placeholder or empty.
#[allow(dead_code)]
pub fn is_title_placeholder(title: &str) -> bool {
    let trimmed = title.trim();
    trimmed.is_empty() || trimmed == TITLE_PLACEHOLDER
}

/// Parse editor output into a `PendingTicket`.
///
/// The `project` field is left empty — the caller fills it in.
///
/// Returns `None` if the content matches the default template or is effectively empty.
pub fn parse_ticket_file(content: &str) -> Option<PendingTicket> {
    if content.trim().is_empty() {
        return None;
    }

    if content == generate_template() {
        return None;
    }

    let (front_matter, body) = match content.split_once("\n---") {
        Some((fm, b)) => {
            // Strip the leading newline (or \n) after the --- delimiter line
            let body_after = b.strip_prefix('\n').unwrap_or(b);
            (fm, body_after.trim().to_string())
        }
        None => (content, String::new()),
    };

    let mut title = String::new();
    let mut ticket_type = DEFAULT_TICKET_TYPE.to_owned();
    let mut priority: i64 = 0;
    let mut branch: Option<String> = None;

    for line in front_matter.lines() {
        if let Some(val) = line.strip_prefix("title:") {
            title = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("type:") {
            ticket_type = normalize_ticket_type(val);
        } else if let Some(val) = line.strip_prefix("priority:") {
            if let Ok(p) = val.trim().parse::<i64>() {
                priority = p;
            }
        } else if let Some(val) = line.strip_prefix("branch:") {
            let trimmed = val.trim();
            branch = if trimmed.is_empty() || trimmed == BRANCH_PLACEHOLDER {
                None
            } else {
                Some(trimmed.to_string())
            };
        }
    }

    Some(PendingTicket {
        project: String::new(),
        title,
        ticket_type,
        priority,
        branch,
        body,
    })
}

// --- Flow orchestration (formerly v2/create_ticket.rs) ---

use crate::cmd::Cmd;
use crate::model::Model;

/// Start the create ticket flow (top-level ticket, no parent).
///
/// Emits `Cmd::SpawnEditor` which causes the TEA loop to break out, run
/// the editor, and re-enter with the parsed result.
pub fn start_create_flow(model: Model) -> (Model, Vec<Cmd>) {
    (
        model,
        vec![Cmd::SpawnEditor {
            parent_id: None,
            project: None,
            content: None,
        }],
    )
}

/// Start the create child ticket flow (child of the ticket on the detail page).
///
/// Emits `Cmd::SpawnEditor` with the parent's ID and project pre-filled.
pub fn start_create_child_flow(model: Model) -> (Model, Vec<Cmd>) {
    let (parent_id, project) = match &model.ticket_detail {
        Some(detail) => {
            let parent_id = detail.ticket_id.clone();
            let project = detail
                .data
                .data()
                .and_then(|d| d.detail.ticket.as_ref().map(|t| t.project.clone()))
                .unwrap_or_default();
            (parent_id, project)
        }
        None => return (model, vec![]),
    };

    let project_opt = if project.is_empty() {
        None
    } else {
        Some(project)
    };

    (
        model,
        vec![Cmd::SpawnEditor {
            parent_id: Some(parent_id),
            project: project_opt,
            content: None,
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LoadState, TicketDetailData, TicketDetailModel, TicketTableModel};

    #[test]
    fn valid_parse() {
        let content = "title: Fix the bug\ntype: code\npriority: 2\n---\nThis is the body.\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.title, "Fix the bug");
        assert_eq!(ticket.ticket_type, "code");
        assert_eq!(ticket.priority, 2);
        assert_eq!(ticket.body, "This is the body.");
        assert_eq!(ticket.project, "");
    }

    #[test]
    fn missing_type_defaults_to_design() {
        let content = "title: No type\npriority: 1\n---\nBody.\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.ticket_type, "design");
    }

    #[test]
    fn type_aliases_normalize() {
        for (alias, expected) in [
            ("c", "code"),
            ("task", "code"),
            ("epic", "code"),
            ("code", "code"),
            ("d", "design"),
            ("design", "design"),
        ] {
            let content = format!("title: test\ntype: {alias}\npriority: 0\n---\n\n");
            let ticket = parse_ticket_file(&content).unwrap();
            assert_eq!(
                ticket.ticket_type, expected,
                "alias '{alias}' should normalize to '{expected}'"
            );
        }
    }

    #[test]
    fn empty_file_returns_none() {
        assert!(parse_ticket_file("").is_none());
        assert!(parse_ticket_file("   \n  \n").is_none());
    }

    #[test]
    fn unchanged_template_returns_none() {
        let template = generate_template();
        assert!(parse_ticket_file(&template).is_none());
    }

    #[test]
    fn missing_title_defaults_to_empty() {
        let content = "priority: 1\n---\nSome body text\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.title, "");
        assert_eq!(ticket.priority, 1);
        assert!(is_title_placeholder(&ticket.title));
    }

    #[test]
    fn whitespace_trimming() {
        let content = "title:   spaced out   \npriority:  3  \n---\n  body with spaces  \n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.title, "spaced out");
        assert_eq!(ticket.priority, 3);
        assert_eq!(ticket.body, "body with spaces");
    }

    #[test]
    fn various_priority_values() {
        for (input, expected) in [("0", 0), ("1", 1), ("-1", -1), ("100", 100)] {
            let content = format!("title: test\npriority: {input}\n---\n\n");
            let ticket = parse_ticket_file(&content).unwrap();
            assert_eq!(ticket.priority, expected, "failed for input {input}");
        }
    }

    #[test]
    fn is_title_placeholder_checks() {
        assert!(is_title_placeholder("<summarize>"));
        assert!(is_title_placeholder("  <summarize>  "));
        assert!(is_title_placeholder(""));
        assert!(is_title_placeholder("   "));
        assert!(!is_title_placeholder("A real title"));
    }

    #[test]
    fn template_has_expected_format() {
        let t = generate_template();
        assert_eq!(
            t,
            "title: <summarize>\ntype: design\npriority: 0\nbranch: <ticket-id>\n---\n\n"
        );
    }

    #[test]
    fn template_ends_with_branch_placeholder_before_delimiter() {
        let t = generate_template();
        assert!(t.contains("\nbranch: <ticket-id>\n---\n"));
    }

    #[test]
    fn serialize_basic() {
        let output =
            serialize_to_template("ur", "Fix the bug", "code", 2, None, "This is the body.");
        assert_eq!(
            output,
            "title: Fix the bug\ntype: code\npriority: 2\nbranch: <ticket-id>\n---\nThis is the body.\n"
        );
    }

    #[test]
    fn serialize_empty_body() {
        let output = serialize_to_template("ur", "A title", "code", 0, None, "");
        assert_eq!(
            output,
            "title: A title\ntype: code\npriority: 0\nbranch: <ticket-id>\n---\n\n"
        );
    }

    #[test]
    fn serialize_whitespace_only_body() {
        let output = serialize_to_template("ur", "A title", "code", 1, None, "   \n  ");
        assert_eq!(
            output,
            "title: A title\ntype: code\npriority: 1\nbranch: <ticket-id>\n---\n\n"
        );
    }

    #[test]
    fn serialize_with_branch_some() {
        let output =
            serialize_to_template("ur", "A title", "code", 1, Some("feature/foo"), "body text");
        assert_eq!(
            output,
            "title: A title\ntype: code\npriority: 1\nbranch: feature/foo\n---\nbody text\n"
        );
    }

    #[test]
    fn serialize_branch_none_emits_placeholder() {
        let output = serialize_to_template("ur", "T", "code", 0, None, "");
        assert!(output.contains("branch: <ticket-id>"));
    }

    #[test]
    fn parse_branch_absent_is_none() {
        let content = "title: T\ntype: code\npriority: 0\n---\nbody\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.branch, None);
    }

    #[test]
    fn parse_branch_placeholder_is_none() {
        let content = "title: T\ntype: code\npriority: 0\nbranch: <ticket-id>\n---\nbody\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.branch, None);
    }

    #[test]
    fn parse_branch_empty_is_none() {
        let content = "title: T\ntype: code\npriority: 0\nbranch: \n---\nbody\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.branch, None);
    }

    #[test]
    fn parse_branch_whitespace_is_none() {
        let content = "title: T\ntype: code\npriority: 0\nbranch:    \n---\nbody\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.branch, None);
    }

    #[test]
    fn parse_branch_value_is_some() {
        let content = "title: T\ntype: code\npriority: 0\nbranch: feature/foo\n---\nbody\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.branch.as_deref(), Some("feature/foo"));
    }

    #[test]
    fn round_trip_basic() {
        let project = "ur";
        let title = "Fix the bug";
        let ticket_type = "code";
        let priority = 2;
        let body = "This is the body.";

        let serialized = serialize_to_template(project, title, ticket_type, priority, None, body);
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, title);
        assert_eq!(parsed.ticket_type, ticket_type);
        assert_eq!(parsed.priority, priority);
        assert_eq!(parsed.body, body);
        assert_eq!(parsed.branch, None);
    }

    #[test]
    fn round_trip_empty_body() {
        let serialized = serialize_to_template("proj", "Empty body ticket", "design", 0, None, "");
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Empty body ticket");
        assert_eq!(parsed.ticket_type, "design");
        assert_eq!(parsed.priority, 0);
        assert_eq!(parsed.body, "");
        assert_eq!(parsed.branch, None);
    }

    #[test]
    fn round_trip_special_characters() {
        let body = "Some **markdown** with `code`\n\n---\n\nAnother section after delimiter";
        let serialized = serialize_to_template("ur", "Special chars", "code", 3, None, body);
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Special chars");
        assert_eq!(parsed.priority, 3);
        // The parser splits on the first \n--- so subsequent --- are part of the body
        assert_eq!(parsed.body, body.trim());
    }

    #[test]
    fn round_trip_multiline_body() {
        let body = "Line 1\nLine 2\nLine 3";
        let serialized = serialize_to_template("ur", "Multi-line", "code", 1, None, body);
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Multi-line");
        assert_eq!(parsed.priority, 1);
        assert_eq!(parsed.body, body);
    }

    #[test]
    fn round_trip_negative_priority() {
        let serialized =
            serialize_to_template("ur", "Negative prio", "design", -5, None, "body text");
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Negative prio");
        assert_eq!(parsed.ticket_type, "design");
        assert_eq!(parsed.priority, -5);
        assert_eq!(parsed.body, "body text");
    }

    #[test]
    fn round_trip_branch_some() {
        let serialized =
            serialize_to_template("ur", "With branch", "code", 0, Some("feature/foo"), "body");
        let parsed = parse_ticket_file(&serialized).unwrap();
        assert_eq!(parsed.branch.as_deref(), Some("feature/foo"));
    }

    #[test]
    fn round_trip_branch_none() {
        let serialized = serialize_to_template("ur", "No branch", "code", 0, None, "body");
        let parsed = parse_ticket_file(&serialized).unwrap();
        assert_eq!(parsed.branch, None);
    }

    #[test]
    fn start_create_flow_emits_spawn_editor() {
        let model = Model::initial();
        let (_, cmds) = start_create_flow(model);
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            Cmd::SpawnEditor {
                parent_id, project, ..
            } => {
                assert!(parent_id.is_none());
                assert!(project.is_none());
            }
            other => panic!("expected SpawnEditor, got {other:?}"),
        }
    }

    fn make_detail_model(ticket_id: &str, project: &str) -> TicketDetailModel {
        use ur_rpc::proto::ticket::{GetTicketResponse, Ticket};
        TicketDetailModel {
            ticket_id: ticket_id.to_string(),
            data: LoadState::Loaded(TicketDetailData {
                detail: GetTicketResponse {
                    ticket: Some(Ticket {
                        id: ticket_id.to_string(),
                        title: "Parent".to_string(),
                        body: String::new(),
                        created_at: String::new(),
                        updated_at: String::new(),
                        project: project.to_string(),
                        status: "open".to_string(),
                        priority: 0,
                        parent_id: String::new(),
                        ticket_type: "task".to_string(),
                        children_completed: 0,
                        children_total: 0,
                        depth: 0,
                        branch: String::new(),
                        dispatch_status: String::new(),
                    }),
                    activities: vec![],
                    metadata: vec![],
                },
                children: vec![],
                total_children: 0,
            }),
            activities: LoadState::NotLoaded,
            children_table: TicketTableModel::empty(),
            show_closed: false,
        }
    }

    #[test]
    fn start_create_child_flow_with_project() {
        let mut model = Model::initial();
        model.ticket_detail = Some(make_detail_model("ur-abc", "ur"));
        let (_, cmds) = start_create_child_flow(model);
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            Cmd::SpawnEditor {
                parent_id, project, ..
            } => {
                assert_eq!(parent_id.as_deref(), Some("ur-abc"));
                assert_eq!(project.as_deref(), Some("ur"));
            }
            other => panic!("expected SpawnEditor, got {other:?}"),
        }
    }

    #[test]
    fn start_create_child_flow_without_detail() {
        let model = Model::initial();
        let (_, cmds) = start_create_child_flow(model);
        assert!(cmds.is_empty());
    }
}
