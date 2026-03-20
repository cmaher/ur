use std::fmt::Write;

use ur_rpc::proto::knowledge::{KnowledgeDoc, KnowledgeSummary};

/// Format a single knowledge document's full detail view (used by `read`).
pub fn format_doc(doc: &KnowledgeDoc) -> String {
    let mut out = String::new();
    writeln!(out, "ID:      {}", doc.id).unwrap();
    writeln!(out, "Title:   {}", doc.title).unwrap();
    if !doc.source.is_empty() {
        writeln!(out, "Source:  {}", doc.source).unwrap();
    }
    if !doc.tags.is_empty() {
        writeln!(out, "Tags:    {}", doc.tags.join(", ")).unwrap();
    }
    writeln!(out, "Created: {}", doc.created_at).unwrap();
    writeln!(out, "Updated: {}", doc.updated_at).unwrap();
    if !doc.content.is_empty() {
        writeln!(out).unwrap();
        write!(out, "{}", doc.content).unwrap();
    }
    // Remove trailing newline
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Format a table of knowledge summaries (used by `list`).
pub fn format_summary_list(docs: &[KnowledgeSummary]) -> String {
    let mut out = String::new();
    writeln!(out, "{:<16} {:<10} {:<24} TITLE", "ID", "SOURCE", "TAGS").unwrap();
    let separator: String = std::iter::repeat_n('-', 72).collect();
    writeln!(out, "{separator}").unwrap();
    for d in docs {
        let tags = if d.tags.is_empty() {
            "-".to_string()
        } else {
            d.tags.join(", ")
        };
        let tags_display = if tags.len() > 22 {
            format!("{}...", &tags[..19])
        } else {
            tags
        };
        writeln!(
            out,
            "{:<16} {:<10} {:<24} {}",
            d.id, d.source, tags_display, d.title
        )
        .unwrap();
    }
    write!(out, "\n{} doc(s)", docs.len()).unwrap();
    out
}

/// Format a list of tags (used by `list-tags`).
pub fn format_tags(tags: &[String]) -> String {
    let mut out = String::new();
    for tag in tags {
        writeln!(out, "{tag}").unwrap();
    }
    write!(out, "\n{} tag(s)", tags.len()).unwrap();
    out
}
