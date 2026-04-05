use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::components::scrollable_markdown::render_scrollable_markdown;
use crate::context::TuiContext;
use crate::input::MarkdownScrollHandler;
use crate::model::{Model, ScrollableMarkdownModel, TicketBodyModel};

/// Render the ticket body page into the given content area.
///
/// Shows a header with ticket ID and title, and a scrollable markdown body.
/// Body data is passed in at construction time (no async fetch needed).
pub fn render_ticket_body(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let Some(ref body_model) = model.ticket_body else {
        render_message(area, buf, ctx, "No body data");
        return;
    };

    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(1),    // body
    ])
    .split(area);

    render_header(
        &body_model.ticket_id,
        &body_model.title,
        chunks[0],
        buf,
        ctx,
    );
    render_body_pane(body_model, chunks[1], buf, ctx);
}

/// Render a simple message.
fn render_message(area: Rect, buf: &mut Buffer, ctx: &TuiContext, msg: &str) {
    let style = Style::default()
        .fg(ctx.theme.base_content)
        .bg(ctx.theme.base_100);
    Paragraph::new(Line::raw(msg))
        .style(style)
        .render(area, buf);
}

/// Render the header line: ticket ID (accented) + title (dim).
fn render_header(ticket_id: &str, title: &str, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    let id_style = Style::default().fg(ctx.theme.accent);
    let sep_style = Style::default().fg(ctx.theme.neutral_content);
    let title_style = Style::default().fg(ctx.theme.neutral_content);

    let id_part = ticket_id.to_string();
    let sep_part = "  ".to_string();
    let title_budget = (area.width as usize)
        .saturating_sub(id_part.len() + sep_part.len())
        .max(1);
    let title_truncated = if title.chars().count() > title_budget {
        let s: String = title.chars().take(title_budget.saturating_sub(1)).collect();
        format!("{s}...")
    } else {
        title.to_string()
    };

    let line = Line::from(vec![
        Span::styled(id_part, id_style),
        Span::styled(sep_part, sep_style),
        Span::styled(title_truncated, title_style),
    ]);

    Paragraph::new(line).render(area, buf);
}

/// Render the scrollable body pane using the shared markdown component.
fn render_body_pane(
    body_model: &TicketBodyModel,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
) {
    render_scrollable_markdown(&body_model.body, area, buf, ctx, &body_model.scroll);
}

/// Initialize the body model for a ticket. No async fetch needed since body text
/// is passed in directly.
pub fn init_body_model(model: &mut Model, ticket_id: String, title: String, body: String) {
    model.ticket_body = Some(TicketBodyModel {
        ticket_id,
        title,
        body,
        scroll: ScrollableMarkdownModel::default(),
    });
}

/// Create the input handler for the ticket body page.
///
/// Uses the shared `MarkdownScrollHandler` for scroll navigation
/// (j/k/↓/↑ for lines, h/l/←/→/Ctrl+f/Ctrl+b for pages).
/// Back is handled by the GlobalHandler's Esc.
pub fn ticket_body_handler() -> MarkdownScrollHandler {
    MarkdownScrollHandler {
        handler_name: "ticket_body",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputHandler, InputResult};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // ── init_body_model ────────────────────────────────────────────────

    #[test]
    fn init_creates_body_model() {
        let mut model = Model::initial();
        init_body_model(
            &mut model,
            "ur-test".to_string(),
            "Title".to_string(),
            "Body text".to_string(),
        );
        assert!(model.ticket_body.is_some());
        let bm = model.ticket_body.as_ref().unwrap();
        assert_eq!(bm.ticket_id, "ur-test");
        assert_eq!(bm.title, "Title");
        assert_eq!(bm.body, "Body text");
        assert_eq!(bm.scroll.scroll_offset, 0);
    }

    // ── scroll functions ───────────────────────────────────────────────

    #[test]
    fn scroll_down_increments_offset() {
        let mut model = Model::initial();
        init_body_model(
            &mut model,
            "ur-test".to_string(),
            "T".to_string(),
            "body".to_string(),
        );
        let bm = model.ticket_body.as_ref().unwrap();
        bm.scroll.last_total_lines.set(50);
        bm.scroll.last_body_height.set(10);

        model.ticket_body.as_mut().unwrap().scroll.scroll_down(3);
        assert_eq!(
            model.ticket_body.as_ref().unwrap().scroll.scroll_offset,
            3
        );
    }

    #[test]
    fn scroll_down_clamps_to_max() {
        let mut model = Model::initial();
        init_body_model(
            &mut model,
            "ur-test".to_string(),
            "T".to_string(),
            "body".to_string(),
        );
        let bm = model.ticket_body.as_ref().unwrap();
        bm.scroll.last_total_lines.set(5);
        bm.scroll.last_body_height.set(3);

        model
            .ticket_body
            .as_mut()
            .unwrap()
            .scroll
            .scroll_down(100);
        // max_offset = 5 - 3 = 2
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll.scroll_offset, 2);
    }

    #[test]
    fn scroll_up_decrements_offset() {
        let mut model = Model::initial();
        init_body_model(
            &mut model,
            "ur-test".to_string(),
            "T".to_string(),
            "body".to_string(),
        );
        model.ticket_body.as_mut().unwrap().scroll.scroll_offset = 5;

        model.ticket_body.as_mut().unwrap().scroll.scroll_up(2);
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll.scroll_offset, 3);
    }

    #[test]
    fn scroll_up_clamps_to_zero() {
        let mut model = Model::initial();
        init_body_model(
            &mut model,
            "ur-test".to_string(),
            "T".to_string(),
            "body".to_string(),
        );

        model.ticket_body.as_mut().unwrap().scroll.scroll_up(10);
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll.scroll_offset, 0);
    }

    #[test]
    fn page_down_scrolls_by_height() {
        let mut model = Model::initial();
        init_body_model(
            &mut model,
            "ur-test".to_string(),
            "T".to_string(),
            "body".to_string(),
        );
        let bm = model.ticket_body.as_ref().unwrap();
        bm.scroll.last_total_lines.set(100);
        bm.scroll.last_body_height.set(10);

        model.ticket_body.as_mut().unwrap().scroll.page_down();
        assert_eq!(
            model.ticket_body.as_ref().unwrap().scroll.scroll_offset,
            10
        );
    }

    #[test]
    fn page_up_scrolls_by_height() {
        let mut model = Model::initial();
        init_body_model(
            &mut model,
            "ur-test".to_string(),
            "T".to_string(),
            "body".to_string(),
        );
        model.ticket_body.as_mut().unwrap().scroll.scroll_offset = 15;
        model
            .ticket_body
            .as_ref()
            .unwrap()
            .scroll
            .last_body_height
            .set(10);

        model.ticket_body.as_mut().unwrap().scroll.page_up();
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll.scroll_offset, 5);
    }

    // ── input handler ──────────────────────────────────────────────────

    #[test]
    fn handler_j_captures_scroll_down() {
        let handler = ticket_body_handler();
        let key = make_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_k_captures_scroll_up() {
        let handler = ticket_body_handler();
        let key = make_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_l_captures_page_down() {
        let handler = ticket_body_handler();
        let key = make_key(KeyCode::Char('l'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_unknown_bubbles() {
        let handler = ticket_body_handler();
        let key = make_key(KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Bubble));
    }

    #[test]
    fn handler_footer_has_scroll_and_page() {
        let handler = ticket_body_handler();
        let commands = handler.footer_commands();
        assert!(commands.iter().any(|c| c.description == "Scroll"));
        assert!(commands.iter().any(|c| c.description == "Page"));
    }

    #[test]
    fn handler_name() {
        let handler = ticket_body_handler();
        assert_eq!(handler.name(), "ticket_body");
    }
}
