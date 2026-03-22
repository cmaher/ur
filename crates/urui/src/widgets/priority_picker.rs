use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::FooterCommand;
use crate::widgets::overlay::render_overlay;

/// Priority level definitions with labels.
const PRIORITIES: &[(i64, &str)] = &[
    (0, "Critical"),
    (1, "High"),
    (2, "Medium"),
    (3, "Normal"),
    (4, "Backlog"),
];

/// Result of handling a key event in the priority picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PriorityPickerResult {
    /// The picker consumed the event; stay open.
    Consumed,
    /// The user selected a priority.
    Selected(i64),
    /// The picker should close without selection.
    Close,
}

/// State for the priority picker overlay.
pub struct PriorityPickerState {
    /// Current cursor position (0-indexed, maps to PRIORITIES index).
    cursor: usize,
}

impl PriorityPickerState {
    /// Create a new picker with cursor initialized to the given priority.
    pub fn new(current_priority: i64) -> Self {
        let cursor = PRIORITIES
            .iter()
            .position(|(p, _)| *p == current_priority)
            .unwrap_or(0);
        Self { cursor }
    }

    /// Handle a raw key event. Returns the result indicating what happened.
    pub fn handle_key(&mut self, key: KeyEvent) -> PriorityPickerResult {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => PriorityPickerResult::Close,
            KeyCode::Char('j') | KeyCode::Down => {
                if self.cursor < PRIORITIES.len() - 1 {
                    self.cursor += 1;
                }
                PriorityPickerResult::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                PriorityPickerResult::Consumed
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                let (priority, _) = PRIORITIES[self.cursor];
                PriorityPickerResult::Selected(priority)
            }
            KeyCode::Char(c) if ('0'..='4').contains(&c) => {
                let digit = (c as u8 - b'0') as i64;
                PriorityPickerResult::Selected(digit)
            }
            _ => PriorityPickerResult::Consumed,
        }
    }

    /// Render the priority picker as a centered overlay.
    pub fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        let height = (PRIORITIES.len() as u16) + 2; // +2 for borders
        let width = 30u16;
        let inner = render_overlay(area, buf, ctx, " Priority ", width, height);

        let theme = &ctx.theme;
        for (i, (priority, label)) in PRIORITIES.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            let row_area = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            let is_selected = i == self.cursor;

            let style = if is_selected {
                Style::default().fg(theme.primary_content).bg(theme.primary)
            } else {
                Style::default().fg(theme.base_content).bg(theme.base_200)
            };

            buf.set_style(row_area, style);
            let text = format!(" {priority}  P{priority} {label}");
            let line = Line::from(Span::raw(text)).style(style);
            line.render(row_area, buf);
        }
    }

    /// Footer commands to show when the priority picker is open.
    pub fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "j/k".to_string(),
                description: "Navigate".to_string(),
            },
            FooterCommand {
                key_label: "Space".to_string(),
                description: "Confirm".to_string(),
            },
            FooterCommand {
                key_label: "0-4".to_string(),
                description: "Quick set".to_string(),
            },
            FooterCommand {
                key_label: "Esc".to_string(),
                description: "Close".to_string(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_initializes_cursor_to_current_priority() {
        let state = PriorityPickerState::new(0);
        assert_eq!(state.cursor, 0);

        let state = PriorityPickerState::new(2);
        assert_eq!(state.cursor, 2);

        let state = PriorityPickerState::new(4);
        assert_eq!(state.cursor, 4);
    }

    #[test]
    fn new_with_invalid_priority_defaults_to_zero() {
        let state = PriorityPickerState::new(99);
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn navigate_down() {
        let mut state = PriorityPickerState::new(0);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, PriorityPickerResult::Consumed);
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn navigate_up() {
        let mut state = PriorityPickerState::new(2);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, PriorityPickerResult::Consumed);
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn navigate_up_does_not_underflow() {
        let mut state = PriorityPickerState::new(0);
        state.handle_key(KeyEvent::new(
            KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn navigate_down_does_not_overflow() {
        let mut state = PriorityPickerState::new(4);
        state.handle_key(KeyEvent::new(
            KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.cursor, 4);
    }

    #[test]
    fn space_confirms_selection() {
        let mut state = PriorityPickerState::new(2);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, PriorityPickerResult::Selected(2));
    }

    #[test]
    fn enter_confirms_selection() {
        let mut state = PriorityPickerState::new(3);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, PriorityPickerResult::Selected(3));
    }

    #[test]
    fn quick_key_selects_priority() {
        let mut state = PriorityPickerState::new(0);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('3'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, PriorityPickerResult::Selected(3));
    }

    #[test]
    fn quick_key_out_of_range_consumed() {
        let mut state = PriorityPickerState::new(0);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('5'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, PriorityPickerResult::Consumed);
    }

    #[test]
    fn esc_closes() {
        let mut state = PriorityPickerState::new(0);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, PriorityPickerResult::Close);
    }

    #[test]
    fn q_closes() {
        let mut state = PriorityPickerState::new(0);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('q'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, PriorityPickerResult::Close);
    }

    #[test]
    fn footer_commands_present() {
        let state = PriorityPickerState::new(0);
        let cmds = state.footer_commands();
        assert!(!cmds.is_empty());
        assert!(cmds.iter().any(|c| c.description == "Confirm"));
        assert!(cmds.iter().any(|c| c.description == "Quick set"));
    }
}
