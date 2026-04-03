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

/// Priority level definitions with labels.
const PRIORITIES: &[(i64, &str)] = &[
    (0, "Critical"),
    (1, "High"),
    (2, "Medium"),
    (3, "Normal"),
    (4, "Backlog"),
];

/// Modal input handler for the priority picker overlay.
///
/// Captures all keys (no bubbling). Number keys 0-4 select a priority directly,
/// j/k navigate, Enter/Space confirm, Esc cancels.
pub struct PriorityPickerHandler;

impl InputHandler for PriorityPickerHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        // All keys are captured — this is a modal overlay.
        let msg = match key.code {
            KeyCode::Esc => Msg::Overlay(OverlayMsg::PriorityCancelled),
            KeyCode::Char('j') | KeyCode::Down => {
                Msg::Overlay(OverlayMsg::PriorityPickerNavigate { delta: 1 })
            }
            KeyCode::Char('k') | KeyCode::Up => {
                Msg::Overlay(OverlayMsg::PriorityPickerNavigate { delta: -1 })
            }
            KeyCode::Char(' ') | KeyCode::Enter => Msg::Overlay(OverlayMsg::PriorityPickerConfirm),
            KeyCode::Char(c) if ('0'..='4').contains(&c) => {
                Msg::Overlay(OverlayMsg::PriorityPickerQuickSelect {
                    digit: (c as u8 - b'0') as i64,
                })
            }
            _ => Msg::Overlay(OverlayMsg::Consumed),
        };
        InputResult::Capture(msg)
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "j/k".to_string(),
                description: "Navigate".to_string(),
                common: false,
                pinned: false,
            },
            FooterCommand {
                key_label: "0-4".to_string(),
                description: "Quick set".to_string(),
                common: false,
                pinned: false,
            },
            FooterCommand {
                key_label: "Space".to_string(),
                description: "Confirm".to_string(),
                common: false,
                pinned: false,
            },
            FooterCommand {
                key_label: "Esc".to_string(),
                description: "Close".to_string(),
                common: false,
                pinned: false,
            },
        ]
    }

    fn name(&self) -> &str {
        "priority_picker"
    }
}

/// Render the priority picker overlay from the model state.
pub fn render_priority_picker(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let cursor = match &model.active_overlay {
        Some(ActiveOverlay::PriorityPicker { cursor, .. }) => *cursor,
        _ => return,
    };

    let height = (PRIORITIES.len() as u16) + 2; // +2 for borders
    let width = 30u16;
    let inner = render_overlay(area, buf, ctx, " Priority ", width, height);

    let theme = &ctx.theme;
    for (i, (priority, label)) in PRIORITIES.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let row_area = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let is_selected = i == cursor;

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

/// Map a current priority value to a cursor index into PRIORITIES.
pub fn priority_to_cursor(current_priority: i64) -> usize {
    PRIORITIES
        .iter()
        .position(|(p, _)| *p == current_priority)
        .unwrap_or(0)
}

/// Map a cursor index to the priority value.
pub fn cursor_to_priority(cursor: usize) -> i64 {
    PRIORITIES.get(cursor).map(|(p, _)| *p).unwrap_or(0)
}

/// Returns the number of priority options.
pub fn priority_count() -> usize {
    PRIORITIES.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn handler_captures_esc() {
        let handler = PriorityPickerHandler;
        match handler.handle_key(key(KeyCode::Esc)) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::PriorityCancelled)) => {}
            other => panic!("expected PriorityCancelled, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_number_keys() {
        let handler = PriorityPickerHandler;
        match handler.handle_key(key(KeyCode::Char('3'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::PriorityPickerQuickSelect {
                digit: 3,
            })) => {}
            other => panic!("expected QuickSelect(3), got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_j_navigate() {
        let handler = PriorityPickerHandler;
        match handler.handle_key(key(KeyCode::Char('j'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::PriorityPickerNavigate { delta: 1 })) => {
            }
            other => panic!("expected Navigate(1), got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_k_navigate() {
        let handler = PriorityPickerHandler;
        match handler.handle_key(key(KeyCode::Char('k'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::PriorityPickerNavigate {
                delta: -1,
            })) => {}
            other => panic!("expected Navigate(-1), got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_enter_confirm() {
        let handler = PriorityPickerHandler;
        match handler.handle_key(key(KeyCode::Enter)) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::PriorityPickerConfirm)) => {}
            other => panic!("expected PriorityPickerConfirm, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_space_confirm() {
        let handler = PriorityPickerHandler;
        match handler.handle_key(key(KeyCode::Char(' '))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::PriorityPickerConfirm)) => {}
            other => panic!("expected PriorityPickerConfirm, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_unknown_keys() {
        let handler = PriorityPickerHandler;
        // Unknown keys should still be captured (modal), treated as cancel
        match handler.handle_key(key(KeyCode::Char('x'))) {
            InputResult::Capture(_) => {}
            InputResult::Bubble => panic!("modal handler should never bubble"),
        }
    }

    #[test]
    fn priority_to_cursor_maps_correctly() {
        assert_eq!(priority_to_cursor(0), 0);
        assert_eq!(priority_to_cursor(2), 2);
        assert_eq!(priority_to_cursor(4), 4);
    }

    #[test]
    fn priority_to_cursor_defaults_for_invalid() {
        assert_eq!(priority_to_cursor(99), 0);
    }

    #[test]
    fn cursor_to_priority_maps_correctly() {
        assert_eq!(cursor_to_priority(0), 0);
        assert_eq!(cursor_to_priority(2), 2);
        assert_eq!(cursor_to_priority(4), 4);
    }

    #[test]
    fn footer_commands_present() {
        let handler = PriorityPickerHandler;
        let cmds = handler.footer_commands();
        assert!(!cmds.is_empty());
        assert!(cmds.iter().any(|c| c.description == "Confirm"));
        assert!(cmds.iter().any(|c| c.description == "Close"));
    }

    #[test]
    fn handler_name() {
        let handler = PriorityPickerHandler;
        assert_eq!(handler.name(), "priority_picker");
    }
}
