//! Terminal markdown rendering using pulldown-cmark.
//!
//! Converts markdown text into `Vec<ratatui::text::Line>` with styled spans.
//! Takes a [`MarkdownColors`] struct instead of a full theme to avoid circular
//! dependency with urui.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Color configuration for markdown rendering.
///
/// Contains the minimum set of colors needed to render markdown without
/// depending on the full urui `Theme` type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownColors {
    /// Color for normal body text.
    pub text: Color,
    /// Color for heading lines.
    pub heading: Color,
    /// Color for code blocks and inline code.
    pub code: Color,
    /// Color for dimmed elements such as list bullets and numbers.
    pub dim: Color,
}

impl Default for MarkdownColors {
    fn default() -> Self {
        Self {
            text: Color::Reset,
            heading: Color::Cyan,
            code: Color::Yellow,
            dim: Color::DarkGray,
        }
    }
}

/// Render markdown text into a list of styled terminal lines.
///
/// # Parameters
///
/// - `text` — Raw markdown source.
/// - `width` — Terminal column width used for wrapping plain-text paragraphs.
/// - `colors` — Color configuration for styled elements.
///
/// # Returns
///
/// A `Vec<Line<'static>>` suitable for use with ratatui widgets.
pub fn render_markdown(text: &str, width: usize, colors: &MarkdownColors) -> Vec<Line<'static>> {
    let mut renderer = MarkdownRenderer::new(width, colors.clone());
    renderer.render(text)
}

// ---------------------------------------------------------------------------
// Internal renderer
// ---------------------------------------------------------------------------

struct MarkdownRenderer {
    width: usize,
    colors: MarkdownColors,
}

/// Accumulated state threaded through event processing.
struct RenderState {
    output: Vec<Line<'static>>,
    current_spans: Vec<Span<'static>>,
    bold: u32,
    italic: u32,
    heading_level: u32,
    in_code_block: bool,
    list_stack: Vec<Option<u64>>,
    list_item_counters: Vec<u64>,
    pending_list_prefix: Option<String>,
}

impl RenderState {
    fn new() -> Self {
        Self {
            output: Vec::new(),
            current_spans: Vec::new(),
            bold: 0,
            italic: 0,
            heading_level: 0,
            in_code_block: false,
            list_stack: Vec::new(),
            list_item_counters: Vec::new(),
            pending_list_prefix: None,
        }
    }

    fn flush_line(&mut self) {
        if !self.current_spans.is_empty() {
            self.output
                .push(Line::from(std::mem::take(&mut self.current_spans)));
        }
    }
}

impl MarkdownRenderer {
    fn new(width: usize, colors: MarkdownColors) -> Self {
        Self { width, colors }
    }

    fn render(&mut self, text: &str) -> Vec<Line<'static>> {
        if text.is_empty() {
            return Vec::new();
        }

        let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        let parser = Parser::new_ext(text, options);
        let mut state = RenderState::new();

        for event in parser {
            self.handle_event(event, &mut state);
        }

        state.flush_line();
        state.output
    }

    fn handle_event(&self, event: Event<'_>, state: &mut RenderState) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                state.flush_line();
                state.heading_level = level as u32;
            }
            Event::End(TagEnd::Heading(_)) => {
                state.flush_line();
                state.heading_level = 0;
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => state.flush_line(),
            Event::Start(Tag::BlockQuote(_)) => {}
            Event::End(TagEnd::BlockQuote(_)) => state.flush_line(),
            Event::Start(Tag::List(start)) => self.start_list(start, state),
            Event::End(TagEnd::List(_)) => self.end_list(state),
            Event::Start(Tag::Item) => self.start_item(state),
            Event::End(TagEnd::Item) => self.end_item(state),
            Event::Start(Tag::CodeBlock(_)) => self.start_code_block(state),
            Event::End(TagEnd::CodeBlock) => self.end_code_block(state),
            Event::Start(Tag::Strong) => state.bold += 1,
            Event::End(TagEnd::Strong) => state.bold = state.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => state.italic += 1,
            Event::End(TagEnd::Emphasis) => state.italic = state.italic.saturating_sub(1),
            Event::Code(code) => {
                let style = Style::default().fg(self.colors.code);
                state
                    .current_spans
                    .push(Span::styled(code.into_string(), style));
            }
            Event::Start(Tag::Link { .. }) | Event::End(TagEnd::Link) => {}
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => {}
            Event::Start(Tag::Table(_)) | Event::End(TagEnd::Table) => state.flush_line(),
            Event::Start(Tag::TableHead)
            | Event::End(TagEnd::TableHead)
            | Event::Start(Tag::TableRow)
            | Event::End(TagEnd::TableRow)
            | Event::Start(Tag::TableCell)
            | Event::End(TagEnd::TableCell) => {}
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::Text(text_cow) => self.handle_text(text_cow.into_string(), state),
            Event::SoftBreak => state.current_spans.push(Span::raw(" ")),
            Event::HardBreak => state.flush_line(),
            Event::Rule => self.handle_rule(state),
            Event::TaskListMarker(checked) => {
                let marker = if checked { "[x] " } else { "[ ] " };
                state
                    .current_spans
                    .push(Span::styled(marker, Style::default().fg(self.colors.dim)));
            }
            _ => {}
        }
    }

    fn start_list(&self, start: Option<u64>, state: &mut RenderState) {
        state.list_stack.push(start);
        state.list_item_counters.push(start.unwrap_or(1));
    }

    fn end_list(&self, state: &mut RenderState) {
        state.list_stack.pop();
        state.list_item_counters.pop();
        state.flush_line();
    }

    fn start_item(&self, state: &mut RenderState) {
        state.flush_line();
        let depth = state.list_stack.len();
        let indent = "  ".repeat(depth.saturating_sub(1));
        let prefix = build_list_prefix(&indent, &state.list_stack, &mut state.list_item_counters);
        state.pending_list_prefix = Some(prefix);
    }

    fn end_item(&self, state: &mut RenderState) {
        state.flush_line();
        state.pending_list_prefix = None;
    }

    fn start_code_block(&self, state: &mut RenderState) {
        state.flush_line();
        state.in_code_block = true;
    }

    fn end_code_block(&self, state: &mut RenderState) {
        state.flush_line();
        state.in_code_block = false;
    }

    fn handle_text(&self, text_str: String, state: &mut RenderState) {
        if let Some(prefix) = state.pending_list_prefix.take() {
            state
                .current_spans
                .push(Span::styled(prefix, Style::default().fg(self.colors.dim)));
        }

        if state.in_code_block {
            self.push_code_block_text(&text_str, state);
            return;
        }

        let style = self.text_style(state.heading_level, state.bold, state.italic);
        let wrap_width = if self.width > 0 {
            self.width
        } else {
            usize::MAX
        };
        if state.heading_level > 0 {
            state.current_spans.push(Span::styled(text_str, style));
        } else {
            push_wrapped_text(&text_str, style, wrap_width, state);
        }
    }

    fn handle_rule(&self, state: &mut RenderState) {
        state.flush_line();
        let rule = "─".repeat(self.width.min(80));
        state.output.push(Line::from(Span::styled(
            rule,
            Style::default().fg(self.colors.dim),
        )));
    }

    /// Build the appropriate style for inline text given current formatting state.
    fn text_style(&self, heading_level: u32, bold: u32, italic: u32) -> Style {
        let mut style = if heading_level > 0 {
            Style::default()
                .fg(self.colors.heading)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.colors.text)
        };

        if bold > 0 {
            style = style.add_modifier(Modifier::BOLD);
        }
        if italic > 0 {
            style = style.add_modifier(Modifier::ITALIC);
        }

        style
    }

    /// Push code block text, splitting on newlines to produce separate lines.
    fn push_code_block_text(&self, text: &str, state: &mut RenderState) {
        let style = Style::default().fg(self.colors.code);
        let mut lines = text.split('\n').peekable();
        while let Some(line) = lines.next() {
            if !line.is_empty() {
                state
                    .current_spans
                    .push(Span::styled(line.to_owned(), style));
            }
            if lines.peek().is_some() {
                state.flush_line();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Build the bullet or number prefix for a list item.
fn build_list_prefix(
    indent: &str,
    list_stack: &[Option<u64>],
    list_item_counters: &mut [u64],
) -> String {
    match list_stack.last().copied().flatten() {
        Some(_) => {
            let counter = list_item_counters
                .last_mut()
                .map(|c| {
                    let val = *c;
                    *c += 1;
                    val
                })
                .unwrap_or(1);
            format!("{indent}{counter}. ")
        }
        None => format!("{indent}• "),
    }
}

/// Push text into spans, word-wrapping long runs to fit within `width` columns.
fn push_wrapped_text(text: &str, style: Style, width: usize, state: &mut RenderState) {
    if width == usize::MAX {
        state
            .current_spans
            .push(Span::styled(text.to_owned(), style));
        return;
    }

    let current_len: usize = state
        .current_spans
        .iter()
        .map(|s| s.content.chars().count())
        .sum();
    let mut line_len = current_len;
    let mut buf = String::new();

    for word in text.split_inclusive(' ') {
        let word_len = word.chars().count();
        if line_len + word_len > width && line_len > 0 {
            if !buf.is_empty() {
                state
                    .current_spans
                    .push(Span::styled(std::mem::take(&mut buf), style));
            }
            state.flush_line();
            line_len = 0;
        }
        buf.push_str(word);
        line_len += word_len;
    }

    if !buf.is_empty() {
        state.current_spans.push(Span::styled(buf, style));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn default_colors() -> MarkdownColors {
        MarkdownColors::default()
    }

    fn spans_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    // ── MarkdownColors ───────────────────────────────────────────────────

    #[test]
    fn markdown_colors_has_required_fields() {
        let c = MarkdownColors {
            text: Color::White,
            heading: Color::Cyan,
            code: Color::Yellow,
            dim: Color::DarkGray,
        };
        assert_eq!(c.text, Color::White);
        assert_eq!(c.heading, Color::Cyan);
        assert_eq!(c.code, Color::Yellow);
        assert_eq!(c.dim, Color::DarkGray);
    }

    #[test]
    fn markdown_colors_default_is_sensible() {
        let c = MarkdownColors::default();
        let _ = c.text;
        let _ = c.heading;
        let _ = c.code;
        let _ = c.dim;
    }

    // ── Empty input ──────────────────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty_vec() {
        let lines = render_markdown("", 80, &default_colors());
        assert!(lines.is_empty());
    }

    // ── Headings ─────────────────────────────────────────────────────────

    #[test]
    fn h1_is_bold_and_heading_color() {
        let colors = default_colors();
        let lines = render_markdown("# Hello World", 80, &colors);
        assert!(!lines.is_empty());
        let first = &lines[0];
        let text = spans_text(first);
        assert!(text.contains("Hello World"), "text: {text:?}");
        let span = first
            .spans
            .iter()
            .find(|s| s.content.contains("Hello World"))
            .unwrap();
        assert_eq!(span.style.fg, Some(colors.heading));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn h2_is_bold_and_heading_color() {
        let colors = default_colors();
        let lines = render_markdown("## Section", 80, &colors);
        assert!(!lines.is_empty());
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("Section"))
            .unwrap();
        assert_eq!(span.style.fg, Some(colors.heading));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn h3_is_bold_and_heading_color() {
        let colors = default_colors();
        let lines = render_markdown("### Deep", 80, &colors);
        assert!(!lines.is_empty());
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("Deep"))
            .unwrap();
        assert_eq!(span.style.fg, Some(colors.heading));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    // ── Fenced code blocks ───────────────────────────────────────────────

    #[test]
    fn fenced_code_block_uses_code_color() {
        let colors = default_colors();
        let md = "```\nlet x = 1;\n```";
        let lines = render_markdown(md, 80, &colors);
        let code_line = lines.iter().find(|l| spans_text(l).contains("let x = 1;"));
        assert!(code_line.is_some(), "no code line found in {lines:?}");
        let span = code_line
            .unwrap()
            .spans
            .iter()
            .find(|s| s.content.contains("let x = 1;"))
            .unwrap();
        assert_eq!(span.style.fg, Some(colors.code));
    }

    #[test]
    fn fenced_code_block_multiline() {
        let colors = default_colors();
        let md = "```rust\nfn foo() {}\nfn bar() {}\n```";
        let lines = render_markdown(md, 80, &colors);
        let has_foo = lines.iter().any(|l| spans_text(l).contains("fn foo()"));
        let has_bar = lines.iter().any(|l| spans_text(l).contains("fn bar()"));
        assert!(has_foo, "missing foo in {lines:?}");
        assert!(has_bar, "missing bar in {lines:?}");
    }

    // ── Inline code ──────────────────────────────────────────────────────

    #[test]
    fn inline_code_uses_code_color() {
        let colors = default_colors();
        let md = "Use `cargo build` to compile.";
        let lines = render_markdown(md, 80, &colors);
        assert!(!lines.is_empty());
        let code_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("cargo build"));
        assert!(code_span.is_some(), "inline code span not found");
        assert_eq!(code_span.unwrap().style.fg, Some(colors.code));
    }

    // ── Bold ─────────────────────────────────────────────────────────────

    #[test]
    fn bold_text_has_bold_modifier() {
        let colors = default_colors();
        let md = "This is **important** text.";
        let lines = render_markdown(md, 80, &colors);
        assert!(!lines.is_empty());
        let bold_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("important"));
        assert!(bold_span.is_some(), "bold span not found");
        assert!(
            bold_span
                .unwrap()
                .style
                .add_modifier
                .contains(Modifier::BOLD),
            "span missing BOLD modifier"
        );
    }

    // ── Italic ───────────────────────────────────────────────────────────

    #[test]
    fn italic_text_has_italic_modifier() {
        let colors = default_colors();
        let md = "This is *emphasized* text.";
        let lines = render_markdown(md, 80, &colors);
        assert!(!lines.is_empty());
        let italic_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("emphasized"));
        assert!(italic_span.is_some(), "italic span not found");
        assert!(
            italic_span
                .unwrap()
                .style
                .add_modifier
                .contains(Modifier::ITALIC),
            "span missing ITALIC modifier"
        );
    }

    // ── Lists ────────────────────────────────────────────────────────────

    #[test]
    fn unordered_list_has_dim_bullet() {
        let colors = default_colors();
        let md = "- item one\n- item two";
        let lines = render_markdown(md, 80, &colors);
        let bullet_line = lines
            .iter()
            .find(|l| spans_text(l).contains("item one"))
            .expect("item one line not found");
        let bullet_span = bullet_line.spans.iter().find(|s| s.content.contains('•'));
        assert!(
            bullet_span.is_some(),
            "bullet span not found in {bullet_line:?}"
        );
        assert_eq!(bullet_span.unwrap().style.fg, Some(colors.dim));
    }

    #[test]
    fn ordered_list_has_dim_number() {
        let colors = default_colors();
        let md = "1. first\n2. second";
        let lines = render_markdown(md, 80, &colors);
        let first_line = lines
            .iter()
            .find(|l| spans_text(l).contains("first"))
            .expect("first line not found");
        let number_span = first_line.spans.iter().find(|s| s.content.contains("1."));
        assert!(
            number_span.is_some(),
            "number span not found in {first_line:?}"
        );
        assert_eq!(number_span.unwrap().style.fg, Some(colors.dim));
    }

    // ── Plain text wrapping ──────────────────────────────────────────────

    #[test]
    fn plain_text_wraps_to_given_width() {
        let colors = default_colors();
        let md = "aaaa bbbb cccc dddd eeee";
        let lines = render_markdown(md, 12, &colors);
        for line in &lines {
            let len: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(len <= 14, "line too long ({len}): {line:?}");
        }
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        for word in ["aaaa", "bbbb", "cccc", "dddd", "eeee"] {
            assert!(all_text.contains(word), "word {word} missing from output");
        }
    }

    // ── Unhandled elements → plain text ─────────────────────────────────

    #[test]
    fn link_text_is_rendered_as_plain_text() {
        let colors = default_colors();
        let md = "See [the docs](https://example.com) for details.";
        let lines = render_markdown(md, 80, &colors);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            all_text.contains("the docs"),
            "link text missing: {all_text:?}"
        );
    }

    #[test]
    fn html_is_ignored() {
        let colors = default_colors();
        let md = "before <b>bold</b> after";
        let _ = render_markdown(md, 80, &colors);
    }
}
