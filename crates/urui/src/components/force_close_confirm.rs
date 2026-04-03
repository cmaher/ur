use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;

use super::overlay::render_overlay;
use crate::input::{FooterCommand, InputHandler, InputResult};
use crate::model::{ActiveOverlay, Model};
use crate::msg::{Msg, OverlayMsg};

/// Modal input handler for the force-close confirmation overlay.
///
/// Captures all keys. y/1 confirms, n/2/Esc cancels, everything else consumed.
pub struct ForceCloseConfirmHandler;

impl InputHandler for ForceCloseConfirmHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        let msg = match key.code {
            KeyCode::Char('1') | KeyCode::Char('y') => {
                Msg::Overlay(OverlayMsg::ForceCloseConfirmYes)
            }
            KeyCode::Char('2') | KeyCode::Char('n') | KeyCode::Esc => {
                Msg::Overlay(OverlayMsg::ForceCloseCancelled)
            }
            _ => Msg::Overlay(OverlayMsg::Consumed),
        };
        InputResult::Capture(msg)
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "1/y".to_string(),
                description: "Yes".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "2/n".to_string(),
                description: "No".to_string(),
                common: false,
            },
        ]
    }

    fn name(&self) -> &str {
        "force_close_confirm"
    }
}

/// Render the force-close confirmation overlay from the model state.
pub fn render_force_close_confirm(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let (ticket_id, open_children) = match &model.active_overlay {
        Some(ActiveOverlay::ForceCloseConfirm {
            ticket_id,
            open_children,
        }) => (ticket_id.as_str(), *open_children),
        _ => return,
    };

    let height = 4u16; // border top + prompt line + options line + border bottom
    let width = 44u16;
    let inner = render_overlay(area, buf, ctx, " Confirm ", width, height);

    let theme = &ctx.theme;
    let style = Style::default().fg(theme.base_content).bg(theme.base_200);

    // Prompt line
    if inner.height > 0 {
        let row = Rect::new(inner.x, inner.y, inner.width, 1);
        let prompt = format!(" Force Close {} and {} children?", ticket_id, open_children);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn handler_y_confirms() {
        let handler = ForceCloseConfirmHandler;
        match handler.handle_key(key(KeyCode::Char('y'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::ForceCloseConfirmYes)) => {}
            other => panic!("expected ForceCloseConfirmYes, got {other:?}"),
        }
    }

    #[test]
    fn handler_1_confirms() {
        let handler = ForceCloseConfirmHandler;
        match handler.handle_key(key(KeyCode::Char('1'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::ForceCloseConfirmYes)) => {}
            other => panic!("expected ForceCloseConfirmYes, got {other:?}"),
        }
    }

    #[test]
    fn handler_n_cancels() {
        let handler = ForceCloseConfirmHandler;
        match handler.handle_key(key(KeyCode::Char('n'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::ForceCloseCancelled)) => {}
            other => panic!("expected ForceCloseCancelled, got {other:?}"),
        }
    }

    #[test]
    fn handler_2_cancels() {
        let handler = ForceCloseConfirmHandler;
        match handler.handle_key(key(KeyCode::Char('2'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::ForceCloseCancelled)) => {}
            other => panic!("expected ForceCloseCancelled, got {other:?}"),
        }
    }

    #[test]
    fn handler_esc_cancels() {
        let handler = ForceCloseConfirmHandler;
        match handler.handle_key(key(KeyCode::Esc)) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::ForceCloseCancelled)) => {}
            other => panic!("expected ForceCloseCancelled, got {other:?}"),
        }
    }

    #[test]
    fn handler_unknown_consumed() {
        let handler = ForceCloseConfirmHandler;
        match handler.handle_key(key(KeyCode::Char('x'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::Consumed)) => {}
            other => panic!("expected Consumed, got {other:?}"),
        }
    }

    #[test]
    fn footer_commands_present() {
        let handler = ForceCloseConfirmHandler;
        let cmds = handler.footer_commands();
        assert_eq!(cmds.len(), 2);
        assert!(cmds.iter().any(|c| c.description == "Yes"));
        assert!(cmds.iter().any(|c| c.description == "No"));
    }

    #[test]
    fn handler_name() {
        let handler = ForceCloseConfirmHandler;
        assert_eq!(handler.name(), "force_close_confirm");
    }
}
