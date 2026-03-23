use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::FooterCommand;
use crate::widgets::overlay::render_overlay;

/// Result of handling a key event in the project input overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectInputResult {
    /// The overlay consumed the event; stay open.
    Consumed,
    /// The user submitted the text buffer contents.
    Submit(String),
    /// The user cancelled input.
    Cancel,
}

/// State for the project input text overlay.
pub struct ProjectInputState {
    /// Current text buffer.
    buffer: String,
}

impl ProjectInputState {
    /// Create a new empty project input state.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Handle a raw key event. Returns the result indicating what happened.
    pub fn handle_key(&mut self, key: KeyEvent) -> ProjectInputResult {
        match key.code {
            KeyCode::Esc => ProjectInputResult::Cancel,
            KeyCode::Enter => ProjectInputResult::Submit(self.buffer.clone()),
            KeyCode::Backspace => {
                self.buffer.pop();
                ProjectInputResult::Consumed
            }
            KeyCode::Char(c) => {
                self.buffer.push(c);
                ProjectInputResult::Consumed
            }
            _ => ProjectInputResult::Consumed,
        }
    }

    /// Render the project input as a centered overlay.
    pub fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
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

        let cursor = "\u{2588}"; // block cursor character
        let text = format!(" {}{}", self.buffer, cursor);
        let line = Line::from(Span::raw(text)).style(input_style);
        line.render(row_area, buf);
    }

    /// Footer commands to show when the project input is open.
    pub fn footer_commands(&self) -> Vec<FooterCommand> {
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
}

impl Default for ProjectInputState {
    fn default() -> Self {
        Self::new()
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
    fn character_input_appends_to_buffer() {
        let mut state = ProjectInputState::new();
        let r = state.handle_key(key(KeyCode::Char('u')));
        assert_eq!(r, ProjectInputResult::Consumed);
        assert_eq!(state.buffer, "u");

        state.handle_key(key(KeyCode::Char('r')));
        assert_eq!(state.buffer, "ur");
    }

    #[test]
    fn backspace_deletes_last_char() {
        let mut state = ProjectInputState::new();
        state.handle_key(key(KeyCode::Char('a')));
        state.handle_key(key(KeyCode::Char('b')));
        let r = state.handle_key(key(KeyCode::Backspace));
        assert_eq!(r, ProjectInputResult::Consumed);
        assert_eq!(state.buffer, "a");
    }

    #[test]
    fn backspace_on_empty_buffer_is_noop() {
        let mut state = ProjectInputState::new();
        let r = state.handle_key(key(KeyCode::Backspace));
        assert_eq!(r, ProjectInputResult::Consumed);
        assert_eq!(state.buffer, "");
    }

    #[test]
    fn enter_submits_buffer() {
        let mut state = ProjectInputState::new();
        state.handle_key(key(KeyCode::Char('u')));
        state.handle_key(key(KeyCode::Char('r')));
        let r = state.handle_key(key(KeyCode::Enter));
        assert_eq!(r, ProjectInputResult::Submit("ur".to_string()));
    }

    #[test]
    fn enter_submits_empty_buffer() {
        let mut state = ProjectInputState::new();
        let r = state.handle_key(key(KeyCode::Enter));
        assert_eq!(r, ProjectInputResult::Submit(String::new()));
    }

    #[test]
    fn escape_cancels() {
        let mut state = ProjectInputState::new();
        state.handle_key(key(KeyCode::Char('x')));
        let r = state.handle_key(key(KeyCode::Esc));
        assert_eq!(r, ProjectInputResult::Cancel);
    }

    #[test]
    fn unknown_keys_consumed() {
        let mut state = ProjectInputState::new();
        let r = state.handle_key(key(KeyCode::Tab));
        assert_eq!(r, ProjectInputResult::Consumed);
        assert_eq!(state.buffer, "");
    }

    #[test]
    fn footer_commands_present() {
        let state = ProjectInputState::new();
        let cmds = state.footer_commands();
        assert!(!cmds.is_empty());
        assert!(cmds.iter().any(|c| c.description == "Submit"));
        assert!(cmds.iter().any(|c| c.description == "Cancel"));
    }
}
