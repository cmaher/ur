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

/// Modal input handler for the project input overlay.
///
/// Captures all keys. Characters are appended to the buffer, Backspace deletes,
/// Enter submits, Esc cancels.
pub struct ProjectInputHandler;

impl InputHandler for ProjectInputHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        let msg = match key.code {
            KeyCode::Esc => Msg::Overlay(OverlayMsg::ProjectInputCancelled),
            KeyCode::Enter => Msg::Overlay(OverlayMsg::ProjectInputSubmitRequest),
            KeyCode::Backspace => Msg::Overlay(OverlayMsg::ProjectInputBackspace),
            KeyCode::Char(c) => Msg::Overlay(OverlayMsg::ProjectInputChar(c)),
            _ => Msg::Overlay(OverlayMsg::Consumed),
        };
        InputResult::Capture(msg)
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "Enter".to_string(),
                description: "Submit".to_string(),
                common: false,
                pinned: false,
            },
            FooterCommand {
                key_label: "Esc".to_string(),
                description: "Cancel".to_string(),
                common: false,
                pinned: false,
            },
        ]
    }

    fn name(&self) -> &str {
        "project_input"
    }
}

/// Render the project input overlay from the model state.
pub fn render_project_input(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let buffer = match &model.active_overlay {
        Some(ActiveOverlay::ProjectInput { buffer }) => buffer.as_str(),
        _ => return,
    };

    let width = 30u16;
    let height = 3u16; // borders + 1 line for input
    let inner = render_overlay(area, buf, ctx, " Project ", width, height);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let theme = &ctx.theme;
    let row_area = Rect::new(inner.x, inner.y, inner.width, 1);

    let input_style = Style::default().fg(theme.base_content).bg(theme.base_200);
    buf.set_style(row_area, input_style);

    let cursor_char = "\u{2588}"; // block cursor character
    let text = format!(" {}{}", buffer, cursor_char);
    let line = Line::from(Span::raw(text)).style(input_style);
    line.render(row_area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn handler_esc_cancels() {
        let handler = ProjectInputHandler;
        match handler.handle_key(key(KeyCode::Esc)) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::ProjectInputCancelled)) => {}
            other => panic!("expected ProjectInputCancelled, got {other:?}"),
        }
    }

    #[test]
    fn handler_enter_submits() {
        let handler = ProjectInputHandler;
        match handler.handle_key(key(KeyCode::Enter)) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::ProjectInputSubmitRequest)) => {}
            other => panic!("expected ProjectInputSubmitRequest, got {other:?}"),
        }
    }

    #[test]
    fn handler_backspace() {
        let handler = ProjectInputHandler;
        match handler.handle_key(key(KeyCode::Backspace)) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::ProjectInputBackspace)) => {}
            other => panic!("expected ProjectInputBackspace, got {other:?}"),
        }
    }

    #[test]
    fn handler_char_input() {
        let handler = ProjectInputHandler;
        match handler.handle_key(key(KeyCode::Char('u'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::ProjectInputChar('u'))) => {}
            other => panic!("expected ProjectInputChar('u'), got {other:?}"),
        }
    }

    #[test]
    fn handler_unknown_consumed() {
        let handler = ProjectInputHandler;
        match handler.handle_key(key(KeyCode::Tab)) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::Consumed)) => {}
            other => panic!("expected Consumed, got {other:?}"),
        }
    }

    #[test]
    fn footer_commands_present() {
        let handler = ProjectInputHandler;
        let cmds = handler.footer_commands();
        assert!(cmds.iter().any(|c| c.description == "Submit"));
        assert!(cmds.iter().any(|c| c.description == "Cancel"));
    }

    #[test]
    fn handler_name() {
        let handler = ProjectInputHandler;
        assert_eq!(handler.name(), "project_input");
    }
}
