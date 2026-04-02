//! Template generation and front-matter parsing for ticket creation via an external editor.
//!
//! The editor receives a temp file with front matter (title, priority) delimited by `---`
//! from the body. This module generates that template and parses the result back.

/// A ticket parsed from editor output, ready to be sent to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTicket {
    pub project: String,
    pub title: String,
    pub priority: i64,
    pub body: String,
}

const TITLE_PLACEHOLDER: &str = "<summarize>";

/// Generate the default template content shown in the editor.
pub fn generate_template() -> String {
    format!("title: {TITLE_PLACEHOLDER}\npriority: 0\n---\n\n")
}

/// Serialize ticket fields into the frontmatter markdown format used by the editor.
///
/// This is the inverse of [`parse_ticket_file`]: given the individual fields, it
/// produces the same `title: …\npriority: …\n---\n…` format that the editor
/// template uses.
pub fn serialize_to_template(project: &str, title: &str, priority: i64, body: &str) -> String {
    let _ = project; // reserved for future use in the template
    let trimmed_body = body.trim();
    if trimmed_body.is_empty() {
        format!("title: {title}\npriority: {priority}\n---\n\n")
    } else {
        format!("title: {title}\npriority: {priority}\n---\n{trimmed_body}\n")
    }
}

/// Returns `true` if the title is the placeholder or empty.
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
    let mut priority: i64 = 0;

    for line in front_matter.lines() {
        if let Some(val) = line.strip_prefix("title:") {
            title = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("priority:")
            && let Ok(p) = val.trim().parse::<i64>()
        {
            priority = p;
        }
    }

    Some(PendingTicket {
        project: String::new(),
        title,
        priority,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_parse() {
        let content = "title: Fix the bug\npriority: 2\n---\nThis is the body.\n";
        let ticket = parse_ticket_file(content).unwrap();
        assert_eq!(ticket.title, "Fix the bug");
        assert_eq!(ticket.priority, 2);
        assert_eq!(ticket.body, "This is the body.");
        assert_eq!(ticket.project, "");
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
        assert_eq!(t, "title: <summarize>\npriority: 0\n---\n\n");
    }

    #[test]
    fn serialize_basic() {
        let output = serialize_to_template("ur", "Fix the bug", 2, "This is the body.");
        assert_eq!(
            output,
            "title: Fix the bug\npriority: 2\n---\nThis is the body.\n"
        );
    }

    #[test]
    fn serialize_empty_body() {
        let output = serialize_to_template("ur", "A title", 0, "");
        assert_eq!(output, "title: A title\npriority: 0\n---\n\n");
    }

    #[test]
    fn serialize_whitespace_only_body() {
        let output = serialize_to_template("ur", "A title", 1, "   \n  ");
        assert_eq!(output, "title: A title\npriority: 1\n---\n\n");
    }

    #[test]
    fn round_trip_basic() {
        let project = "ur";
        let title = "Fix the bug";
        let priority = 2;
        let body = "This is the body.";

        let serialized = serialize_to_template(project, title, priority, body);
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, title);
        assert_eq!(parsed.priority, priority);
        assert_eq!(parsed.body, body);
    }

    #[test]
    fn round_trip_empty_body() {
        let serialized = serialize_to_template("proj", "Empty body ticket", 0, "");
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Empty body ticket");
        assert_eq!(parsed.priority, 0);
        assert_eq!(parsed.body, "");
    }

    #[test]
    fn round_trip_special_characters() {
        let body = "Some **markdown** with `code`\n\n---\n\nAnother section after delimiter";
        let serialized = serialize_to_template("ur", "Special chars", 3, body);
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Special chars");
        assert_eq!(parsed.priority, 3);
        // The parser splits on the first \n--- so subsequent --- are part of the body
        assert_eq!(parsed.body, body.trim());
    }

    #[test]
    fn round_trip_multiline_body() {
        let body = "Line 1\nLine 2\nLine 3";
        let serialized = serialize_to_template("ur", "Multi-line", 1, body);
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Multi-line");
        assert_eq!(parsed.priority, 1);
        assert_eq!(parsed.body, body);
    }

    #[test]
    fn round_trip_negative_priority() {
        let serialized = serialize_to_template("ur", "Negative prio", -5, "body text");
        let parsed = parse_ticket_file(&serialized).unwrap();

        assert_eq!(parsed.title, "Negative prio");
        assert_eq!(parsed.priority, -5);
        assert_eq!(parsed.body, "body text");
    }
}
