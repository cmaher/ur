use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;

use super::overlay::render_overlay;
use crate::input::FooterCommand;
use crate::model::{ActiveOverlay, Model};
use crate::msg::{Msg, OverlayMsg};

/// Ticket type definitions with labels.
const TICKET_TYPES: &[(&str, &str)] = &[("code", "Code"), ("design", "Design")];

/// Handle a key event for the type menu overlay.
///
/// All keys are captured (modal). j/k navigate, Enter/Space confirm,
/// 1/2 quick-select, Esc cancels.
pub fn handle_key(key: KeyEvent) -> Msg {
    match key.code {
        KeyCode::Esc => Msg::Overlay(OverlayMsg::TypeMenuCancelled),
        KeyCode::Char('j') | KeyCode::Down => {
            Msg::Overlay(OverlayMsg::TypeMenuNavigate { delta: 1 })
        }
        KeyCode::Char('k') | KeyCode::Up => {
            Msg::Overlay(OverlayMsg::TypeMenuNavigate { delta: -1 })
        }
        KeyCode::Char(' ') | KeyCode::Enter => Msg::Overlay(OverlayMsg::TypeMenuConfirm),
        KeyCode::Char('1') => Msg::Overlay(OverlayMsg::TypeMenuQuickSelect { index: 0 }),
        KeyCode::Char('2') => Msg::Overlay(OverlayMsg::TypeMenuQuickSelect { index: 1 }),
        _ => Msg::Overlay(OverlayMsg::Consumed),
    }
}

/// Footer commands for the type menu overlay.
pub fn footer_commands() -> Vec<FooterCommand> {
    vec![
        FooterCommand {
            key_label: "j/k".to_string(),
            description: "Navigate".to_string(),
            common: false,
        },
        FooterCommand {
            key_label: "1-2".to_string(),
            description: "Quick set".to_string(),
            common: false,
        },
        FooterCommand {
            key_label: "Space".to_string(),
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

/// Render the type menu overlay from the model state.
pub fn render_type_menu(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let cursor = match &model.active_overlay {
        Some(ActiveOverlay::TypeMenu { cursor, .. }) => *cursor,
        _ => return,
    };

    let height = (TICKET_TYPES.len() as u16) + 2; // +2 for borders
    let width = 24u16;
    let inner = render_overlay(area, buf, ctx, " Type ", width, height);

    let theme = &ctx.theme;
    for (i, (_type_key, label)) in TICKET_TYPES.iter().enumerate() {
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
        let text = format!(" {}  {label}", i + 1);
        let line = Line::from(Span::raw(text)).style(style);
        line.render(row_area, buf);
    }
}

/// Map a current ticket_type string to a cursor index into TICKET_TYPES.
pub fn type_to_cursor(current_type: &str) -> usize {
    TICKET_TYPES
        .iter()
        .position(|(t, _)| *t == current_type)
        .unwrap_or(0)
}

/// Map a cursor index to the ticket_type string.
pub fn cursor_to_type(cursor: usize) -> &'static str {
    TICKET_TYPES.get(cursor).map(|(t, _)| *t).unwrap_or("code")
}

/// Returns the number of type options.
pub fn type_count() -> usize {
    TICKET_TYPES.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn handle_key_esc() {
        assert!(matches!(
            handle_key(key(KeyCode::Esc)),
            Msg::Overlay(OverlayMsg::TypeMenuCancelled)
        ));
    }

    #[test]
    fn handle_key_number_keys() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('1'))),
            Msg::Overlay(OverlayMsg::TypeMenuQuickSelect { index: 0 })
        ));
        assert!(matches!(
            handle_key(key(KeyCode::Char('2'))),
            Msg::Overlay(OverlayMsg::TypeMenuQuickSelect { index: 1 })
        ));
    }

    #[test]
    fn handle_key_j_navigate() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('j'))),
            Msg::Overlay(OverlayMsg::TypeMenuNavigate { delta: 1 })
        ));
    }

    #[test]
    fn handle_key_k_navigate() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('k'))),
            Msg::Overlay(OverlayMsg::TypeMenuNavigate { delta: -1 })
        ));
    }

    #[test]
    fn handle_key_enter_confirm() {
        assert!(matches!(
            handle_key(key(KeyCode::Enter)),
            Msg::Overlay(OverlayMsg::TypeMenuConfirm)
        ));
    }

    #[test]
    fn handle_key_space_confirm() {
        assert!(matches!(
            handle_key(key(KeyCode::Char(' '))),
            Msg::Overlay(OverlayMsg::TypeMenuConfirm)
        ));
    }

    #[test]
    fn handle_key_unknown_keys() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('x'))),
            Msg::Overlay(OverlayMsg::Consumed)
        ));
    }

    #[test]
    fn type_to_cursor_maps_correctly() {
        assert_eq!(type_to_cursor("code"), 0);
        assert_eq!(type_to_cursor("design"), 1);
    }

    #[test]
    fn type_to_cursor_defaults_for_invalid() {
        assert_eq!(type_to_cursor("unknown"), 0);
    }

    #[test]
    fn cursor_to_type_maps_correctly() {
        assert_eq!(cursor_to_type(0), "code");
        assert_eq!(cursor_to_type(1), "design");
    }

    #[test]
    fn cursor_to_type_defaults_for_out_of_range() {
        assert_eq!(cursor_to_type(99), "code");
    }

    #[test]
    fn footer_commands_present() {
        let cmds = footer_commands();
        assert!(!cmds.is_empty());
        assert!(cmds.iter().any(|c| c.description == "Confirm"));
        assert!(cmds.iter().any(|c| c.description == "Close"));
    }
}
