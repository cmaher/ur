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

/// Handle a key event for the title input overlay.
///
/// All keys are captured (modal). Characters are appended to the buffer,
/// Backspace deletes, Enter submits, Esc cancels.
pub fn handle_key(key: KeyEvent) -> Msg {
    match key.code {
        KeyCode::Esc => Msg::Overlay(OverlayMsg::TitleInputCancelled),
        KeyCode::Enter => Msg::Overlay(OverlayMsg::TitleInputSubmitRequest),
        KeyCode::Backspace => Msg::Overlay(OverlayMsg::TitleInputBackspace),
        KeyCode::Char(c) => Msg::Overlay(OverlayMsg::TitleInputChar(c)),
        _ => Msg::Overlay(OverlayMsg::Consumed),
    }
}

/// Footer commands for the title input overlay.
pub fn footer_commands() -> Vec<FooterCommand> {
    vec![
        FooterCommand {
            key_label: "Enter".to_string(),
            description: "Submit".to_string(),
            common: false,
        },
        FooterCommand {
            key_label: "Esc".to_string(),
            description: "Cancel".to_string(),
            common: false,
        },
    ]
}

/// Modal input handler for the title input overlay (InputHandler adapter).
pub struct TitleInputHandler;

impl InputHandler for TitleInputHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        InputResult::Capture(handle_key(key))
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        footer_commands()
    }

    fn name(&self) -> &str {
        "title_input"
    }
}

/// Render the title input overlay from the model state.
pub fn render_title_input(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let buffer = match &model.active_overlay {
        Some(ActiveOverlay::TitleInput { buffer }) => buffer.as_str(),
        _ => return,
    };

    let width = 50u16;
    let height = 3u16; // borders + 1 line for input
    let inner = render_overlay(area, buf, ctx, " Title ", width, height);

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
    fn handle_key_esc_cancels() {
        assert!(matches!(
            handle_key(key(KeyCode::Esc)),
            Msg::Overlay(OverlayMsg::TitleInputCancelled)
        ));
    }

    #[test]
    fn handle_key_enter_submits() {
        assert!(matches!(
            handle_key(key(KeyCode::Enter)),
            Msg::Overlay(OverlayMsg::TitleInputSubmitRequest)
        ));
    }

    #[test]
    fn handle_key_backspace() {
        assert!(matches!(
            handle_key(key(KeyCode::Backspace)),
            Msg::Overlay(OverlayMsg::TitleInputBackspace)
        ));
    }

    #[test]
    fn handle_key_char_input() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('a'))),
            Msg::Overlay(OverlayMsg::TitleInputChar('a'))
        ));
    }

    #[test]
    fn handle_key_unknown_consumed() {
        assert!(matches!(
            handle_key(key(KeyCode::Tab)),
            Msg::Overlay(OverlayMsg::Consumed)
        ));
    }

    #[test]
    fn footer_commands_present() {
        let cmds = footer_commands();
        assert!(cmds.iter().any(|c| c.description == "Submit"));
        assert!(cmds.iter().any(|c| c.description == "Cancel"));
    }
}
