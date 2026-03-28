use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::FooterCommand;
use crate::widgets::overlay::render_overlay;

/// A target that the user can navigate to via the goto menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoTarget {
    /// Display label shown in the menu (e.g., "Flow Details").
    pub label: String,
    /// The screen name to navigate to (e.g., "flow", "worker", "ticket").
    pub screen: String,
    /// The entity ID to navigate to.
    pub id: String,
}

/// Result of handling a key event in the goto menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GotoMenuResult {
    /// The menu consumed the event; stay open.
    Consumed,
    /// The user selected a goto target.
    Selected(GotoTarget),
    /// The menu should close without selection.
    Close,
}

/// State for the goto menu overlay.
pub struct GotoMenuState {
    /// Available targets to navigate to.
    targets: Vec<GotoTarget>,
    /// Current cursor position (0-indexed).
    cursor: usize,
}

impl GotoMenuState {
    /// Create a new goto menu with the given targets.
    pub fn new(targets: Vec<GotoTarget>) -> Self {
        Self { targets, cursor: 0 }
    }

    /// Handle a raw key event. Returns the result indicating what happened.
    pub fn handle_key(&mut self, key: KeyEvent) -> GotoMenuResult {
        match key.code {
            KeyCode::Esc => GotoMenuResult::Close,
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.targets.is_empty() && self.cursor < self.targets.len() - 1 {
                    self.cursor += 1;
                }
                GotoMenuResult::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                GotoMenuResult::Consumed
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                if let Some(target) = self.targets.get(self.cursor) {
                    GotoMenuResult::Selected(target.clone())
                } else {
                    GotoMenuResult::Consumed
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let digit = (c as u8 - b'0') as usize;
                // 1-indexed: '1' selects index 0, '2' selects index 1, etc.
                if digit >= 1 && digit <= self.targets.len() {
                    GotoMenuResult::Selected(self.targets[digit - 1].clone())
                } else {
                    GotoMenuResult::Consumed
                }
            }
            _ => GotoMenuResult::Consumed,
        }
    }

    /// Render the goto menu as a centered overlay.
    pub fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        let height = (self.targets.len() as u16) + 2; // +2 for borders
        let width = 30u16;
        let inner = render_overlay(area, buf, ctx, " Goto ", width, height);

        let theme = &ctx.theme;
        for (i, target) in self.targets.iter().enumerate() {
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
            let num = i + 1;
            let text = format!(" {num}. {}", target.label);
            let line = Line::from(Span::raw(text)).style(style);
            line.render(row_area, buf);
        }
    }

    /// Footer commands to show when the goto menu is open.
    pub fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "j/k".to_string(),
                description: "Navigate".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "Enter".to_string(),
                description: "Confirm".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "Esc".to_string(),
                description: "Close".to_string(),
                common: false,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_targets() -> Vec<GotoTarget> {
        vec![
            GotoTarget {
                label: "Flow Details".to_string(),
                screen: "flow".to_string(),
                id: "flow-123".to_string(),
            },
            GotoTarget {
                label: "Worker".to_string(),
                screen: "worker".to_string(),
                id: "worker-456".to_string(),
            },
        ]
    }

    #[test]
    fn number_key_selection_returns_correct_target() {
        let mut state = GotoMenuState::new(sample_targets());

        // Press '1' to select first target
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('1'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            r,
            GotoMenuResult::Selected(GotoTarget {
                label: "Flow Details".to_string(),
                screen: "flow".to_string(),
                id: "flow-123".to_string(),
            })
        );

        // Press '2' to select second target
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('2'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            r,
            GotoMenuResult::Selected(GotoTarget {
                label: "Worker".to_string(),
                screen: "worker".to_string(),
                id: "worker-456".to_string(),
            })
        );
    }

    #[test]
    fn esc_closes_menu() {
        let mut state = GotoMenuState::new(sample_targets());
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, GotoMenuResult::Close);
    }

    #[test]
    fn navigate_down_and_confirm() {
        let mut state = GotoMenuState::new(sample_targets());
        assert_eq!(state.cursor, 0);

        // Move down
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, GotoMenuResult::Consumed);
        assert_eq!(state.cursor, 1);

        // Confirm with Enter
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            r,
            GotoMenuResult::Selected(GotoTarget {
                label: "Worker".to_string(),
                screen: "worker".to_string(),
                id: "worker-456".to_string(),
            })
        );
    }

    #[test]
    fn navigate_up_does_not_underflow() {
        let mut state = GotoMenuState::new(sample_targets());
        state.handle_key(KeyEvent::new(
            KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn navigate_down_does_not_overflow() {
        let mut state = GotoMenuState::new(sample_targets());
        state.cursor = 1;
        state.handle_key(KeyEvent::new(
            KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn out_of_range_number_key_is_consumed() {
        let mut state = GotoMenuState::new(sample_targets());
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('3'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, GotoMenuResult::Consumed);
    }

    #[test]
    fn zero_key_is_consumed() {
        let mut state = GotoMenuState::new(sample_targets());
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('0'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, GotoMenuResult::Consumed);
    }

    #[test]
    fn empty_targets_navigate_is_safe() {
        let mut state = GotoMenuState::new(vec![]);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, GotoMenuResult::Consumed);
    }

    #[test]
    fn empty_targets_enter_is_consumed() {
        let mut state = GotoMenuState::new(vec![]);
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, GotoMenuResult::Consumed);
    }

    #[test]
    fn footer_commands_present() {
        let state = GotoMenuState::new(sample_targets());
        let cmds = state.footer_commands();
        assert!(!cmds.is_empty());
        assert!(cmds.iter().any(|c| c.description == "Confirm"));
        assert!(cmds.iter().any(|c| c.description == "Close"));
        assert!(cmds.iter().any(|c| c.description == "Navigate"));
    }
}
