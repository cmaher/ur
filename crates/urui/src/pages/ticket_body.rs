use std::cell::Cell;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use ur_markdown::{MarkdownColors, render_markdown};

use crate::context::TuiContext;
use crate::data::DataPayload;
use crate::keymap::{Action, Keymap};
use crate::page::FooterCommand;
use crate::screen::{Screen, ScreenResult};

/// Full-screen scrollable markdown viewer for a ticket's body text.
///
/// Pushed onto the Tickets tab stack when "b" is pressed from `TicketDetailScreen`.
/// Body text and ticket ID/title are passed in at construction — no RPCs needed.
///
/// Layout:
///   1. Header (Length(1)): ticket ID + title (dimmed label style)
///   2. Body pane (Min(1)): scrollable markdown content rendered via ur_markdown
pub struct TicketBodyScreen {
    ticket_id: String,
    title: String,
    body: String,
    /// Current vertical scroll offset (lines from the top).
    scroll_offset: usize,
    /// Height of the body pane from the last render, used for page scrolling.
    ///
    /// Updated via interior mutability during `render` (which takes `&self`).
    /// Seeded at construction so that page-scroll actions before the first
    /// render produce a reasonable result.
    last_body_height: Cell<usize>,
    /// Rendered line count from the last render, used to clamp NavigateDown.
    last_total_lines: Cell<usize>,
}

impl TicketBodyScreen {
    /// Create a new body viewer for the given ticket.
    ///
    /// - `ticket_id` — Shown in the header alongside the title.
    /// - `title`     — Ticket title shown in the header.
    /// - `body`      — Raw markdown body text to render.
    pub fn new(ticket_id: String, title: String, body: String) -> Self {
        Self {
            ticket_id,
            title,
            body,
            scroll_offset: 0,
            last_body_height: Cell::new(20),
            last_total_lines: Cell::new(0),
        }
    }

    /// Returns the current scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Scroll down by `delta` lines, clamped so the last content line stays visible.
    fn scroll_down(&mut self, delta: usize, content_lines: usize, visible_height: usize) {
        let max_offset = content_lines.saturating_sub(visible_height);
        self.scroll_offset = (self.scroll_offset + delta).min(max_offset);
    }

    /// Scroll up by `delta` lines, clamped to zero.
    fn scroll_up(&mut self, delta: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(delta);
    }
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
        format!("{s}…")
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

/// Render the scrollable body pane and update height/line-count cells.
fn render_body_pane(screen: &TicketBodyScreen, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    let colors = markdown_colors(ctx);
    let all_lines = render_markdown(&screen.body, area.width as usize, &colors);
    let visible_height = area.height as usize;
    let total = all_lines.len();

    // Update cached metrics for use by the next handle_action call.
    screen.last_body_height.set(visible_height.max(1));
    screen.last_total_lines.set(total);

    // Clamp scroll offset to valid range.
    let max_offset = total.saturating_sub(visible_height);
    let offset = screen.scroll_offset.min(max_offset);

    let visible: Vec<Line<'static>> = all_lines
        .into_iter()
        .skip(offset)
        .take(visible_height)
        .collect();

    let bg_style = Style::default().bg(ctx.theme.base_100);
    Paragraph::new(visible).style(bg_style).render(area, buf);
}

impl Screen for TicketBodyScreen {
    fn handle_action(&mut self, action: Action) -> ScreenResult {
        match action {
            Action::Back => ScreenResult::Pop,
            Action::Quit => ScreenResult::Quit,
            Action::NavigateDown => {
                // j / Down: scroll one line forward.
                let total = self.last_total_lines.get();
                let height = self.last_body_height.get().max(1);
                self.scroll_down(1, total, height);
                ScreenResult::Consumed
            }
            Action::NavigateUp => {
                // k / Up: scroll one line backward.
                self.scroll_up(1);
                ScreenResult::Consumed
            }
            Action::PageRight => {
                // Ctrl-F: page forward by one full pane height.
                let page = self.last_body_height.get().max(1);
                let total = self.last_total_lines.get();
                self.scroll_down(page, total, page);
                ScreenResult::Consumed
            }
            Action::PageLeft => {
                // Ctrl-B: page backward by one full pane height.
                let page = self.last_body_height.get().max(1);
                self.scroll_up(page);
                ScreenResult::Consumed
            }
            _ => ScreenResult::Ignored,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        let chunks = Layout::vertical([
            Constraint::Length(1), // header
            Constraint::Min(1),    // body
        ])
        .split(area);

        render_header(&self.ticket_id, &self.title, chunks[0], buf, ctx);
        render_body_pane(self, chunks[1], buf, ctx);
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: keymap.combined_label(&Action::PageLeft, &Action::PageRight),
                description: "Page".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.combined_label(&Action::NavigateUp, &Action::NavigateDown),
                description: "Scroll".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Back),
                description: "Back".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Quit),
                description: "Quit".to_string(),
                common: true,
            },
        ]
    }

    fn on_data(&mut self, _payload: &DataPayload) {}

    fn needs_data(&self) -> bool {
        false
    }

    fn mark_stale(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_screen(body: &str) -> TicketBodyScreen {
        TicketBodyScreen::new(
            "ur-test".to_string(),
            "Test Ticket".to_string(),
            body.to_string(),
        )
    }

    // ── scroll_offset initialization ───────────────────────────────────────

    #[test]
    fn new_screen_starts_at_zero_offset() {
        let screen = make_screen("hello");
        assert_eq!(screen.scroll_offset(), 0);
    }

    // ── scroll_up clamping ─────────────────────────────────────────────────

    #[test]
    fn scroll_up_does_not_underflow() {
        let mut screen = make_screen("line one\nline two\n");
        screen.scroll_up(5);
        assert_eq!(screen.scroll_offset(), 0);
    }

    #[test]
    fn scroll_up_decrements_offset() {
        let mut screen = make_screen("line one\n");
        screen.scroll_offset = 10;
        screen.scroll_up(3);
        assert_eq!(screen.scroll_offset(), 7);
    }

    // ── scroll_down clamping ───────────────────────────────────────────────

    #[test]
    fn scroll_down_clamps_to_max_offset() {
        let mut screen = make_screen("a\nb\nc\n");
        // content_lines=3, visible_height=2 → max_offset=1
        screen.scroll_down(100, 3, 2);
        assert_eq!(screen.scroll_offset(), 1);
    }

    #[test]
    fn scroll_down_stops_at_last_visible_line() {
        let mut screen = make_screen("a\n");
        // content_lines=1, visible_height=1 → max_offset=0
        screen.scroll_down(10, 1, 1);
        assert_eq!(screen.scroll_offset(), 0);
    }

    #[test]
    fn scroll_down_increments_offset() {
        let mut screen = make_screen("a\nb\nc\nd\ne\n");
        // content_lines=5, visible_height=2 → max_offset=3
        screen.scroll_down(2, 5, 2);
        assert_eq!(screen.scroll_offset(), 2);
    }

    #[test]
    fn scroll_down_does_not_overflow_past_end() {
        let mut screen = make_screen("a\nb\n");
        // content_lines=2, visible_height=2 → max_offset=0; any delta stays at 0
        screen.scroll_down(5, 2, 2);
        assert_eq!(screen.scroll_offset(), 0);
    }

    // ── action handling ────────────────────────────────────────────────────

    #[test]
    fn back_action_returns_pop() {
        let mut screen = make_screen("body");
        assert!(matches!(
            screen.handle_action(Action::Back),
            ScreenResult::Pop
        ));
    }

    #[test]
    fn quit_action_returns_quit() {
        let mut screen = make_screen("body");
        assert!(matches!(
            screen.handle_action(Action::Quit),
            ScreenResult::Quit
        ));
    }

    #[test]
    fn navigate_down_returns_consumed() {
        let mut screen = make_screen("a\nb\nc\n");
        assert!(matches!(
            screen.handle_action(Action::NavigateDown),
            ScreenResult::Consumed
        ));
    }

    #[test]
    fn navigate_up_returns_consumed() {
        let mut screen = make_screen("body");
        assert!(matches!(
            screen.handle_action(Action::NavigateUp),
            ScreenResult::Consumed
        ));
    }

    #[test]
    fn page_right_returns_consumed() {
        let mut screen = make_screen("body");
        assert!(matches!(
            screen.handle_action(Action::PageRight),
            ScreenResult::Consumed
        ));
    }

    #[test]
    fn page_left_returns_consumed() {
        let mut screen = make_screen("body");
        assert!(matches!(
            screen.handle_action(Action::PageLeft),
            ScreenResult::Consumed
        ));
    }

    #[test]
    fn unhandled_action_returns_ignored() {
        let mut screen = make_screen("body");
        assert!(matches!(
            screen.handle_action(Action::Refresh),
            ScreenResult::Ignored
        ));
    }

    // ── navigate_up clamping via action ───────────────────────────────────

    #[test]
    fn navigate_up_at_zero_stays_zero() {
        let mut screen = make_screen("body");
        assert_eq!(screen.scroll_offset(), 0);
        screen.handle_action(Action::NavigateUp);
        assert_eq!(screen.scroll_offset(), 0);
    }

    // ── page scroll clamping via last_total_lines ──────────────────────────

    #[test]
    fn page_right_clamps_to_content() {
        let mut screen = make_screen("a\nb\nc\n");
        // Seed: 3 total lines, body height 2 → max_offset = 1
        screen.last_total_lines.set(3);
        screen.last_body_height.set(2);

        screen.handle_action(Action::PageRight);
        // scroll_down(2, 3, 2) → max_offset = 3-2 = 1
        assert_eq!(screen.scroll_offset(), 1);
    }

    #[test]
    fn page_left_clamps_to_zero() {
        let mut screen = make_screen("a\nb\nc\n");
        screen.last_body_height.set(5);
        // Even with large page, should not go below 0
        screen.handle_action(Action::PageLeft);
        assert_eq!(screen.scroll_offset(), 0);
    }

    // ── needs_data / mark_stale ────────────────────────────────────────────

    #[test]
    fn needs_data_is_false() {
        let screen = make_screen("body");
        assert!(!screen.needs_data());
    }

    #[test]
    fn mark_stale_is_noop() {
        let mut screen = make_screen("body");
        screen.mark_stale();
        assert!(!screen.needs_data());
    }

    // ── footer_commands ────────────────────────────────────────────────────

    #[test]
    fn footer_has_back_and_quit() {
        let screen = make_screen("body");
        let keymap = Keymap::default();
        let cmds = screen.footer_commands(&keymap);
        assert!(cmds.iter().any(|c| c.description == "Back"));
        assert!(cmds.iter().any(|c| c.description == "Quit"));
    }

    #[test]
    fn footer_has_page_and_scroll() {
        let screen = make_screen("body");
        let keymap = Keymap::default();
        let cmds = screen.footer_commands(&keymap);
        assert!(cmds.iter().any(|c| c.description == "Page"));
        assert!(cmds.iter().any(|c| c.description == "Scroll"));
    }
}
