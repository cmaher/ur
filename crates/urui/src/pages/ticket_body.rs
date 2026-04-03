use std::cell::Cell;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use ur_markdown::{MarkdownColors, render_markdown};

use crate::context::TuiContext;
use crate::input::MarkdownScrollHandler;
use crate::model::Model;

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

/// Build `MarkdownColors` from the TUI theme.
fn markdown_colors(ctx: &TuiContext) -> MarkdownColors {
    MarkdownColors {
        text: ctx.theme.base_content,
        heading: ctx.theme.accent,
        code: ctx.theme.warning,
        dim: ctx.theme.neutral_content,
    }
}

/// Render the scrollable body pane and update cached metrics.
fn render_body_pane(
    body_model: &super::super::model::TicketBodyModel,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
) {
    let colors = markdown_colors(ctx);
    let all_lines = render_markdown(&body_model.body, area.width as usize, &colors);
    let visible_height = area.height as usize;
    let total = all_lines.len();

    // Update cached metrics for use by the next scroll action.
    body_model.last_body_height.set(visible_height.max(1));
    body_model.last_total_lines.set(total);

    // Clamp scroll offset to valid range.
    let max_offset = total.saturating_sub(visible_height);
    let offset = body_model.scroll_offset.min(max_offset);

    let visible: Vec<Line<'static>> = all_lines
        .into_iter()
        .skip(offset)
        .take(visible_height)
        .collect();

    let bg_style = Style::default().bg(ctx.theme.base_100);
    Paragraph::new(visible).style(bg_style).render(area, buf);
}

/// Handle scroll-down for the body page.
pub fn body_scroll_down(model: &mut Model, delta: usize) {
    let Some(ref mut body_model) = model.ticket_body else {
        return;
    };
    let total = body_model.last_total_lines.get();
    let height = body_model.last_body_height.get().max(1);
    let max_offset = total.saturating_sub(height);
    body_model.scroll_offset = (body_model.scroll_offset + delta).min(max_offset);
}

/// Handle scroll-up for the body page.
pub fn body_scroll_up(model: &mut Model, delta: usize) {
    let Some(ref mut body_model) = model.ticket_body else {
        return;
    };
    body_model.scroll_offset = body_model.scroll_offset.saturating_sub(delta);
}

/// Handle page-down for the body page.
pub fn body_page_down(model: &mut Model) {
    let Some(ref body_model) = model.ticket_body else {
        return;
    };
    let page = body_model.last_body_height.get().max(1);
    let total = body_model.last_total_lines.get();
    let max_offset = total.saturating_sub(page);
    let new_offset = (body_model.scroll_offset + page).min(max_offset);
    // Re-borrow mutably to update
    if let Some(ref mut bm) = model.ticket_body {
        bm.scroll_offset = new_offset;
    }
}

/// Handle page-up for the body page.
pub fn body_page_up(model: &mut Model) {
    let Some(ref body_model) = model.ticket_body else {
        return;
    };
    let page = body_model.last_body_height.get().max(1);
    let new_offset = body_model.scroll_offset.saturating_sub(page);
    if let Some(ref mut bm) = model.ticket_body {
        bm.scroll_offset = new_offset;
    }
}

/// Initialize the body model for a ticket. No async fetch needed since body text
/// is passed in directly.
pub fn init_body_model(model: &mut Model, ticket_id: String, title: String, body: String) {
    model.ticket_body = Some(super::super::model::TicketBodyModel {
        ticket_id,
        title,
        body,
        scroll_offset: 0,
        last_body_height: Cell::new(20),
        last_total_lines: Cell::new(0),
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
        assert_eq!(bm.scroll_offset, 0);
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
        model.ticket_body.as_ref().unwrap().last_total_lines.set(50);
        model.ticket_body.as_ref().unwrap().last_body_height.set(10);

        body_scroll_down(&mut model, 3);
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll_offset, 3);
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
        model.ticket_body.as_ref().unwrap().last_total_lines.set(5);
        model.ticket_body.as_ref().unwrap().last_body_height.set(3);

        body_scroll_down(&mut model, 100);
        // max_offset = 5 - 3 = 2
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll_offset, 2);
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
        model.ticket_body.as_mut().unwrap().scroll_offset = 5;

        body_scroll_up(&mut model, 2);
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll_offset, 3);
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

        body_scroll_up(&mut model, 10);
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll_offset, 0);
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
        model
            .ticket_body
            .as_ref()
            .unwrap()
            .last_total_lines
            .set(100);
        model.ticket_body.as_ref().unwrap().last_body_height.set(10);

        body_page_down(&mut model);
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll_offset, 10);
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
        model.ticket_body.as_mut().unwrap().scroll_offset = 15;
        model.ticket_body.as_ref().unwrap().last_body_height.set(10);

        body_page_up(&mut model);
        assert_eq!(model.ticket_body.as_ref().unwrap().scroll_offset, 5);
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
