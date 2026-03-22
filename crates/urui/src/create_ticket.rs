/// Template generation and front-matter parsing for ticket creation via an external editor.
///
/// The editor receives a temp file with front matter (title, priority) delimited by `---`
/// from the body. This module generates that template and parses the result back.

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
        } else if let Some(val) = line.strip_prefix("priority:") {
            if let Ok(p) = val.trim().parse::<i64>() {
                priority = p;
            }
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
}
