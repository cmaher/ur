//! Template generation and front-matter parsing for ticket creation via an external editor.
//!
//! The editor receives a temp file with front matter (title, priority) delimited by `---`
//! from the body. This module generates that template and parses the result back.

use std::collections::BTreeMap;

/// A ticket parsed from editor output, ready to be sent to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTicket {
    pub project: String,
    pub title: String,
    pub ticket_type: String,
    pub priority: i64,
    pub branch: Option<String>,
    pub body: String,
    /// Arbitrary metadata key-value pairs parsed from the `meta:` block.
    pub meta: BTreeMap<String, String>,
}

const TITLE_PLACEHOLDER: &str = "<summarize>";
pub const BRANCH_PLACEHOLDER: &str = "<ticket-id>";
const DEFAULT_TICKET_TYPE: &str = "design";

/// Metadata keys that the editor is allowed to set or delete.
///
/// Workflow-managed keys (e.g. `autoapprove`, `pr_number`, `gh_repo`,
/// `feedback_mode`) are intentionally excluded so the edit flow cannot
/// clobber running workflow state.
pub const EDITABLE_META_KEYS: &[&str] = &[ur_rpc::ticket_meta::REF];

/// Compute the meta diff between server state and editor output.
///
/// For each key in the allowlist:
/// - Editor has a non-empty value that differs from server → include in `meta_set`.
/// - Server has the key but editor omits or empties it → include in `meta_delete`.
/// - Value unchanged → skip.
///
/// Keys not in the allowlist are never touched.
pub fn compute_meta_diff(
    server: &BTreeMap<String, String>,
    editor: &BTreeMap<String, String>,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut meta_set: Vec<(String, String)> = Vec::new();
    let mut meta_delete: Vec<String> = Vec::new();

    for &key in EDITABLE_META_KEYS {
        let server_val = server.get(key).map(|s| s.as_str()).unwrap_or("");
        let editor_val = editor.get(key).map(|s| s.as_str()).unwrap_or("");

        match (server_val.is_empty(), editor_val.is_empty()) {
            // Both empty — no change.
            (true, true) => {}
            // Server empty, editor has value → set.
            (true, false) => meta_set.push((key.to_owned(), editor_val.to_owned())),
            // Server has value, editor empty → delete.
            (false, true) => meta_delete.push(key.to_owned()),
            // Both non-empty — set only if changed.
            (false, false) => {
                if editor_val != server_val {
                    meta_set.push((key.to_owned(), editor_val.to_owned()));
                }
            }
        }
    }

    (meta_set, meta_delete)
}

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
        "title: {TITLE_PLACEHOLDER}\ntype: {DEFAULT_TICKET_TYPE}\npriority: 0\nbranch: {BRANCH_PLACEHOLDER}\nmeta:\n    ref:\n---\n\n"
    )
}

/// Serialize ticket fields into the frontmatter markdown format used by the editor.
///
/// This is the inverse of [`parse_ticket_file`]: given the individual fields, it
/// produces the same `title: …\npriority: …\nbranch: …\nmeta:\n    ref:\n---\n…`
/// format that the editor template uses. A `None` branch is rendered as the literal
/// `<ticket-id>` placeholder.
///
/// The `meta` block is always emitted:
/// - If `meta` is empty: `meta:\n    ref:\n` (so the field is visible to the user).
/// - If `meta` has entries: `meta:\n` followed by `    <key>: <value>\n` for each,
///   sorted by key (BTreeMap order).
pub fn serialize_to_template(
    project: &str,
    title: &str,
    ticket_type: &str,
    priority: i64,
    branch: Option<&str>,
    meta: &BTreeMap<String, String>,
    body: &str,
) -> String {
    let _ = project; // reserved for future use in the template
    let branch_value = branch.unwrap_or(BRANCH_PLACEHOLDER);
    let meta_block = serialize_meta_block(meta);
    let trimmed_body = body.trim();
    if trimmed_body.is_empty() {
        format!(
            "title: {title}\ntype: {ticket_type}\npriority: {priority}\nbranch: {branch_value}\n{meta_block}---\n\n"
        )
    } else {
        format!(
            "title: {title}\ntype: {ticket_type}\npriority: {priority}\nbranch: {branch_value}\n{meta_block}---\n{trimmed_body}\n"
        )
    }
}

/// Serialize the `meta:` block for use in the frontmatter template.
///
/// If `meta` is empty, emits `meta:\n    ref:\n`.
/// Otherwise, emits `meta:\n` followed by sorted `    <key>: <value>\n` entries.
fn serialize_meta_block(meta: &BTreeMap<String, String>) -> String {
    if meta.is_empty() {
        format!("meta:\n    {}:\n", ur_rpc::ticket_meta::REF)
    } else {
        let mut s = "meta:\n".to_owned();
        for (key, value) in meta {
            s.push_str(&format!("    {key}: {value}\n"));
        }
        s
    }
}

/// Returns `true` if the title is the placeholder or empty.
pub fn is_title_placeholder(title: &str) -> bool {
    let trimmed = title.trim();
    trimmed.is_empty() || trimmed == TITLE_PLACEHOLDER
}

/// Resolve a ticket title from the body by running `claude --model haiku --print`.
///
/// Falls back to a truncated body (first line, max 80 chars) if the command fails.
/// Returns an error only if the body is also empty.
pub async fn resolve_title(body: &str) -> anyhow::Result<String> {
    let prompt = format!(
        "Generate a concise ticket title (under 80 chars) for this description. \
         Output ONLY the title, nothing else:\n\n{body}"
    );
    let output = tokio::process::Command::new("claude")
        .args(["--model", "haiku", "--print", "-p", &prompt])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let title = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !title.is_empty() {
                return Ok(title);
            }
        }
        Ok(o) => {
            tracing::debug!(
                status = %o.status,
                stderr = %String::from_utf8_lossy(&o.stderr),
                "claude title generation failed, falling back to truncation"
            );
        }
        Err(e) => {
            tracing::debug!(error = %e, "claude command not available, falling back to truncation");
        }
    }

    fallback_title(body)
}

/// Truncate the body to produce a fallback title (first line, max 80 chars).
///
/// Returns an error if the body is empty.
fn fallback_title(body: &str) -> anyhow::Result<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Cannot resolve title: body is empty and claude command failed");
    }
    let first_line = trimmed.lines().next().unwrap_or(trimmed);
    if first_line.len() <= 80 {
        Ok(first_line.to_string())
    } else {
        Ok(format!("{}...", &first_line[..77]))
    }
}

fn parse_meta_entry(line: &str) -> Option<(String, String)> {
    let (k, v) = line.trim().split_once(':')?;
    let key = k.trim().to_string();
    let value = v.trim().to_string();
    if key.is_empty() || value.is_empty() {
        return None;
    }
    Some((key, value))
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
    let mut meta: BTreeMap<String, String> = BTreeMap::new();

    let mut in_meta = false;
    for line in front_matter.lines() {
        if in_meta && (line.starts_with("    ") || line.starts_with('\t')) {
            if let Some((k, v)) = parse_meta_entry(line) {
                meta.insert(k, v);
            }
            continue;
        }
        if in_meta {
            in_meta = false;
        }

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
        } else if line.strip_prefix("meta:").is_some() {
            in_meta = true;
        }
    }

    Some(PendingTicket {
        project: String::new(),
        title,
        ticket_type,
        priority,
        branch,
        body,
        meta,
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
    fn fallback_title_short_body_passthrough() {
        let title = fallback_title("This is a short body").unwrap();
        assert_eq!(title, "This is a short body");
    }

    #[test]
    fn fallback_title_long_body_truncates_with_ellipsis() {
        let body = "A".repeat(200);
        let title = fallback_title(&body).unwrap();
        assert_eq!(title.len(), 80);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn fallback_title_uses_first_line_only() {
        let body = "First line title\nSecond line detail\nMore detail";
        let title = fallback_title(body).unwrap();
        assert_eq!(title, "First line title");
    }

    #[test]
    fn fallback_title_empty_body_errors() {
        assert!(fallback_title("").is_err());
        assert!(fallback_title("   \n  ").is_err());
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
            "title: <summarize>\ntype: design\npriority: 0\nbranch: <ticket-id>\nmeta:\n    ref:\n---\n\n"
        );
    }

    #[test]
    fn template_ends_with_branch_placeholder_before_delimiter() {
        let t = generate_template();
        assert!(t.contains("\nbranch: <ticket-id>\nmeta:\n    ref:\n---\n"));
    }

    #[test]
    fn serialize_basic() {
        let output = serialize_to_template(
            "ur",
            "Fix the bug",
            "code",
            2,
            None,
            &Default::default(),
            "This is the body.",
        );
        assert_eq!(
            output,
            "title: Fix the bug\ntype: code\npriority: 2\nbranch: <ticket-id>\nmeta:\n    ref:\n---\nThis is the body.\n"
        );
    }

    #[test]
    fn serialize_empty_body() {
        let output =
            serialize_to_template("ur", "A title", "code", 0, None, &Default::default(), "");
        assert_eq!(
            output,
            "title: A title\ntype: code\npriority: 0\nbranch: <ticket-id>\nmeta:\n    ref:\n---\n\n"
        );
    }

    #[test]
    fn serialize_whitespace_only_body() {
        let output = serialize_to_template(
            "ur",
            "A title",
            "code",
            1,
            None,
            &Default::default(),
            "   \n  ",
        );
        assert_eq!(
            output,
            "title: A title\ntype: code\npriority: 1\nbranch: <ticket-id>\nmeta:\n    ref:\n---\n\n"
        );
    }

    #[test]
    fn serialize_with_branch_some() {
        let output = serialize_to_template(
            "ur",
            "A title",
            "code",
            1,
            Some("feature/foo"),
            &Default::default(),
            "body text",
        );
        assert_eq!(
            output,
            "title: A title\ntype: code\npriority: 1\nbranch: feature/foo\nmeta:\n    ref:\n---\nbody text\n"
        );
    }

    #[test]
    fn serialize_branch_none_emits_placeholder() {
        let output = serialize_to_template("ur", "T", "code", 0, None, &Default::default(), "");
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

        let serialized = serialize_to_template(
            project,
            title,
            ticket_type,
            priority,
            None,
            &Default::default(),
            body,
        );
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, title);
        assert_eq!(parsed.ticket_type, ticket_type);
        assert_eq!(parsed.priority, priority);
        assert_eq!(parsed.body, body);
        assert_eq!(parsed.branch, None);
        assert!(parsed.meta.is_empty());
    }

    #[test]
    fn round_trip_empty_body() {
        let serialized = serialize_to_template(
            "proj",
            "Empty body ticket",
            "design",
            0,
            None,
            &Default::default(),
            "",
        );
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Empty body ticket");
        assert_eq!(parsed.ticket_type, "design");
        assert_eq!(parsed.priority, 0);
        assert_eq!(parsed.body, "");
        assert_eq!(parsed.branch, None);
        assert!(parsed.meta.is_empty());
    }

    #[test]
    fn round_trip_special_characters() {
        let body = "Some **markdown** with `code`\n\n---\n\nAnother section after delimiter";
        let serialized = serialize_to_template(
            "ur",
            "Special chars",
            "code",
            3,
            None,
            &Default::default(),
            body,
        );
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Special chars");
        assert_eq!(parsed.priority, 3);
        // The parser splits on the first \n--- so subsequent --- are part of the body
        assert_eq!(parsed.body, body.trim());
    }

    #[test]
    fn round_trip_multiline_body() {
        let body = "Line 1\nLine 2\nLine 3";
        let serialized = serialize_to_template(
            "ur",
            "Multi-line",
            "code",
            1,
            None,
            &Default::default(),
            body,
        );
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Multi-line");
        assert_eq!(parsed.priority, 1);
        assert_eq!(parsed.body, body);
    }

    #[test]
    fn round_trip_negative_priority() {
        let serialized = serialize_to_template(
            "ur",
            "Negative prio",
            "design",
            -5,
            None,
            &Default::default(),
            "body text",
        );
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Negative prio");
        assert_eq!(parsed.ticket_type, "design");
        assert_eq!(parsed.priority, -5);
        assert_eq!(parsed.body, "body text");
    }

    #[test]
    fn round_trip_branch_some() {
        let serialized = serialize_to_template(
            "ur",
            "With branch",
            "code",
            0,
            Some("feature/foo"),
            &Default::default(),
            "body",
        );
        let parsed = parse_ticket_file(&serialized).unwrap();
        assert_eq!(parsed.branch.as_deref(), Some("feature/foo"));
    }

    #[test]
    fn round_trip_branch_none() {
        let serialized = serialize_to_template(
            "ur",
            "No branch",
            "code",
            0,
            None,
            &Default::default(),
            "body",
        );
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

    // --- Meta block tests ---

    #[test]
    fn parse_meta_empty_values_are_dropped() {
        // The default template has `ref:` with no value — should yield empty meta.
        let content =
            "title: T\ntype: code\npriority: 0\nbranch: <ticket-id>\nmeta:\n    ref:\n---\nbody\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert!(ticket.meta.is_empty());
    }

    #[test]
    fn parse_meta_single_ref() {
        let content = format!(
            "title: T\ntype: code\npriority: 0\nbranch: <ticket-id>\nmeta:\n    {}: PROJ-123\n---\nbody\n",
            ur_rpc::ticket_meta::REF
        );
        let ticket = parse_ticket_file(&content).unwrap();
        assert_eq!(
            ticket
                .meta
                .get(ur_rpc::ticket_meta::REF)
                .map(|s| s.as_str()),
            Some("PROJ-123")
        );
    }

    #[test]
    fn parse_meta_ref_with_spaces_in_value() {
        let content = format!(
            "title: T\ntype: code\npriority: 0\nbranch: <ticket-id>\nmeta:\n    {}: hello world\n---\nbody\n",
            ur_rpc::ticket_meta::REF
        );
        let ticket = parse_ticket_file(&content).unwrap();
        assert_eq!(
            ticket
                .meta
                .get(ur_rpc::ticket_meta::REF)
                .map(|s| s.as_str()),
            Some("hello world")
        );
    }

    #[test]
    fn parse_meta_ref_with_surrounding_whitespace() {
        let content = format!(
            "title: T\ntype: code\npriority: 0\nbranch: <ticket-id>\nmeta:\n    {}:   PROJ-123   \n---\nbody\n",
            ur_rpc::ticket_meta::REF
        );
        let ticket = parse_ticket_file(&content).unwrap();
        assert_eq!(
            ticket
                .meta
                .get(ur_rpc::ticket_meta::REF)
                .map(|s| s.as_str()),
            Some("PROJ-123")
        );
    }

    #[test]
    fn parse_meta_multiple_keys() {
        let content = "title: T\ntype: code\npriority: 0\nbranch: <ticket-id>\nmeta:\n    alpha: val1\n    beta: val2\n    gamma: val3\n---\nbody\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.meta.get("alpha").map(|s| s.as_str()), Some("val1"));
        assert_eq!(ticket.meta.get("beta").map(|s| s.as_str()), Some("val2"));
        assert_eq!(ticket.meta.get("gamma").map(|s| s.as_str()), Some("val3"));
    }

    #[test]
    fn serialize_meta_empty_emits_ref_placeholder() {
        let output = serialize_to_template("ur", "T", "code", 0, None, &Default::default(), "body");
        assert!(output.contains(&format!("meta:\n    {}:\n", ur_rpc::ticket_meta::REF)));
    }

    #[test]
    fn serialize_meta_with_entries_emits_sorted() {
        let mut meta = BTreeMap::new();
        meta.insert(ur_rpc::ticket_meta::REF.to_string(), "PROJ-42".to_string());
        meta.insert("zzz".to_string(), "last".to_string());
        meta.insert("aaa".to_string(), "first".to_string());
        let output = serialize_to_template("ur", "T", "code", 0, None, &meta, "body");
        // BTreeMap sorts alphabetically: aaa, ref, zzz
        assert!(output.contains("meta:\n    aaa: first\n    ref: PROJ-42\n    zzz: last\n"));
    }

    #[test]
    fn round_trip_meta_empty() {
        let serialized =
            serialize_to_template("ur", "T", "code", 0, None, &Default::default(), "body");
        let parsed = parse_ticket_file(&serialized).unwrap();
        assert!(parsed.meta.is_empty());
    }

    #[test]
    fn round_trip_meta_single_ref() {
        let mut meta = BTreeMap::new();
        meta.insert(ur_rpc::ticket_meta::REF.to_string(), "JIRA-99".to_string());
        let serialized = serialize_to_template("ur", "T", "code", 0, None, &meta, "body");
        let parsed = parse_ticket_file(&serialized).unwrap();
        assert_eq!(
            parsed
                .meta
                .get(ur_rpc::ticket_meta::REF)
                .map(|s| s.as_str()),
            Some("JIRA-99")
        );
        assert_eq!(parsed.meta.len(), 1);
    }

    #[test]
    fn round_trip_meta_multiple_keys() {
        let mut meta = BTreeMap::new();
        meta.insert(ur_rpc::ticket_meta::REF.to_string(), "PROJ-1".to_string());
        meta.insert("other".to_string(), "value".to_string());
        let serialized = serialize_to_template("ur", "T", "code", 0, None, &meta, "body");
        let parsed = parse_ticket_file(&serialized).unwrap();
        assert_eq!(parsed.meta, meta);
    }

    #[test]
    fn unchanged_template_with_meta_returns_none() {
        // The template with the meta block should still return None for unchanged content.
        let template = generate_template();
        assert!(parse_ticket_file(&template).is_none());
    }

    #[test]
    fn existing_tests_no_meta_field_still_pass() {
        // Tickets with no meta block parse fine — meta defaults to empty.
        let content = "title: Old ticket\ntype: code\npriority: 1\nbranch: feature/x\n---\nbody\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.title, "Old ticket");
        assert!(ticket.meta.is_empty());
    }

    // --- compute_meta_diff tests ---

    fn ref_key() -> String {
        ur_rpc::ticket_meta::REF.to_owned()
    }

    /// Case 1: setting a previously-empty `ref` → meta_set entry.
    #[test]
    fn meta_diff_set_new_value() {
        let server: BTreeMap<String, String> = BTreeMap::new();
        let mut editor = BTreeMap::new();
        editor.insert(ref_key(), "PROJ-42".to_owned());

        let (meta_set, meta_delete) = compute_meta_diff(&server, &editor);
        assert_eq!(meta_set, vec![(ref_key(), "PROJ-42".to_owned())]);
        assert!(meta_delete.is_empty());
    }

    /// Case 2: changing an existing `ref` → meta_set with new value.
    #[test]
    fn meta_diff_change_existing_value() {
        let mut server = BTreeMap::new();
        server.insert(ref_key(), "OLD-1".to_owned());
        let mut editor = BTreeMap::new();
        editor.insert(ref_key(), "NEW-2".to_owned());

        let (meta_set, meta_delete) = compute_meta_diff(&server, &editor);
        assert_eq!(meta_set, vec![(ref_key(), "NEW-2".to_owned())]);
        assert!(meta_delete.is_empty());
    }

    /// Case 3: clearing an existing `ref` → meta_delete entry.
    #[test]
    fn meta_diff_clear_existing_value() {
        let mut server = BTreeMap::new();
        server.insert(ref_key(), "JIRA-99".to_owned());
        let editor: BTreeMap<String, String> = BTreeMap::new();

        let (meta_set, meta_delete) = compute_meta_diff(&server, &editor);
        assert!(meta_set.is_empty());
        assert_eq!(meta_delete, vec![ref_key()]);
    }

    /// Case 4: unchanged `ref` → no diff entries.
    #[test]
    fn meta_diff_unchanged_value() {
        let mut server = BTreeMap::new();
        server.insert(ref_key(), "SAME-1".to_owned());
        let mut editor = BTreeMap::new();
        editor.insert(ref_key(), "SAME-1".to_owned());

        let (meta_set, meta_delete) = compute_meta_diff(&server, &editor);
        assert!(meta_set.is_empty());
        assert!(meta_delete.is_empty());
    }

    /// Both server and editor have no value for `ref` → no diff entries.
    #[test]
    fn meta_diff_both_empty() {
        let server: BTreeMap<String, String> = BTreeMap::new();
        let editor: BTreeMap<String, String> = BTreeMap::new();

        let (meta_set, meta_delete) = compute_meta_diff(&server, &editor);
        assert!(meta_set.is_empty());
        assert!(meta_delete.is_empty());
    }

    /// Allowlist guard: workflow-managed keys in editor are NOT included in diff.
    #[test]
    fn meta_diff_allowlist_blocks_workflow_keys() {
        let server: BTreeMap<String, String> = BTreeMap::new();
        let mut editor = BTreeMap::new();
        // These are workflow-managed keys that must never be touched.
        editor.insert("autoapprove".to_owned(), "true".to_owned());
        editor.insert("pr_number".to_owned(), "123".to_owned());
        editor.insert("gh_repo".to_owned(), "org/repo".to_owned());
        editor.insert("feedback_mode".to_owned(), "now".to_owned());
        editor.insert("noverify".to_owned(), "true".to_owned());

        let (meta_set, meta_delete) = compute_meta_diff(&server, &editor);
        assert!(
            meta_set.is_empty(),
            "workflow-managed keys must not be set via editor diff"
        );
        assert!(meta_delete.is_empty());
    }

    /// Allowlist guard: workflow-managed key on server side is not deleted even if
    /// editor omits it.
    #[test]
    fn meta_diff_allowlist_does_not_delete_workflow_keys() {
        let mut server = BTreeMap::new();
        server.insert("autoapprove".to_owned(), "true".to_owned());
        server.insert("pr_number".to_owned(), "42".to_owned());
        let editor: BTreeMap<String, String> = BTreeMap::new();

        let (meta_set, meta_delete) = compute_meta_diff(&server, &editor);
        assert!(meta_set.is_empty());
        assert!(
            meta_delete.is_empty(),
            "workflow-managed keys must not be deleted via editor diff"
        );
    }

    /// Mixed: allowlisted key changed, non-allowlisted key added → only allowlisted key in diff.
    #[test]
    fn meta_diff_mixed_keys_only_allowlisted_in_diff() {
        let mut server = BTreeMap::new();
        server.insert(ref_key(), "OLD".to_owned());
        let mut editor = BTreeMap::new();
        editor.insert(ref_key(), "NEW".to_owned());
        editor.insert("autoapprove".to_owned(), "true".to_owned());

        let (meta_set, meta_delete) = compute_meta_diff(&server, &editor);
        assert_eq!(meta_set, vec![(ref_key(), "NEW".to_owned())]);
        assert!(meta_delete.is_empty());
    }
}
