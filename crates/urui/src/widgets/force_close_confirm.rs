use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::FooterCommand;
use crate::widgets::overlay::render_overlay;

/// Result of handling a key event in the force-close confirmation dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForceCloseConfirmResult {
    /// The dialog consumed the event; stay open.
    Consumed,
    /// The user confirmed the force close.
    Confirmed,
    /// The user cancelled the force close.
    Cancelled,
}

/// State for the force-close confirmation overlay.
pub struct ForceCloseConfirmState {
    pub ticket_id: String,
    pub open_children: i32,
}

impl ForceCloseConfirmState {
    /// Handle a raw key event. Returns the result indicating what happened.
    pub fn handle_key(&mut self, key: KeyEvent) -> ForceCloseConfirmResult {
        match key.code {
            KeyCode::Char('1') | KeyCode::Char('y') => ForceCloseConfirmResult::Confirmed,
            KeyCode::Char('2') | KeyCode::Char('n') | KeyCode::Esc => {
                ForceCloseConfirmResult::Cancelled
            }
            _ => ForceCloseConfirmResult::Consumed,
        }
    }

    /// Render the force-close confirmation as a centered overlay.
    pub fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        let height = 4u16; // border top + prompt line + options line + border bottom
        let width = 44u16;
        let inner = render_overlay(area, buf, ctx, " Confirm ", width, height);

        let theme = &ctx.theme;
        let style = Style::default().fg(theme.base_content).bg(theme.base_200);

        // Prompt line
        if inner.height > 0 {
            let row = Rect::new(inner.x, inner.y, inner.width, 1);
            let prompt = format!(
                " Force Close {} and {} children?",
                self.ticket_id, self.open_children
            );
            let line = Line::from(Span::raw(prompt)).style(style);
            line.render(row, buf);
        }

        // Options line
        if inner.height > 1 {
            let row = Rect::new(inner.x, inner.y + 1, inner.width, 1);
            let options = Line::from(vec![
                Span::styled(" 1 ", Style::default().fg(theme.primary).bg(theme.base_200)),
                Span::styled("Yes  ", style),
                Span::styled("2 ", Style::default().fg(theme.primary).bg(theme.base_200)),
                Span::styled("No", style),
            ]);
            options.render(row, buf);
        }
    }

    /// Footer commands to show when the force-close confirmation is open.
    pub fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "1".to_string(),
                description: "Yes".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "2".to_string(),
                description: "No".to_string(),
                common: false,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> ForceCloseConfirmState {
        ForceCloseConfirmState {
            ticket_id: "ur-abc12".to_string(),
            open_children: 3,
        }
    }

    #[test]
    fn key_1_confirms() {
        let mut state = make_state();
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('1'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, ForceCloseConfirmResult::Confirmed);
    }

    #[test]
    fn key_y_confirms() {
        let mut state = make_state();
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('y'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, ForceCloseConfirmResult::Confirmed);
    }

    #[test]
    fn key_2_cancels() {
        let mut state = make_state();
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('2'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, ForceCloseConfirmResult::Cancelled);
    }

    #[test]
    fn key_n_cancels() {
        let mut state = make_state();
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('n'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, ForceCloseConfirmResult::Cancelled);
    }

    #[test]
    fn key_esc_cancels() {
        let mut state = make_state();
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, ForceCloseConfirmResult::Cancelled);
    }

    #[test]
    fn other_key_consumed() {
        let mut state = make_state();
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('x'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, ForceCloseConfirmResult::Consumed);
    }

    #[test]
    fn footer_commands_present() {
        let state = make_state();
        let cmds = state.footer_commands();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].key_label, "1");
        assert_eq!(cmds[0].description, "Yes");
        assert_eq!(cmds[1].key_label, "2");
        assert_eq!(cmds[1].description, "No");
    }
}
