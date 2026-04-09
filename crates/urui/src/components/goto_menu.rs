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
use crate::msg::{GotoTarget, Msg, OverlayMsg};

/// Handle a key event for the goto menu overlay.
///
/// All keys are captured (modal). j/k navigate, Enter/Space confirm,
/// number keys quick-select (1-indexed), Esc cancels.
pub fn handle_key(key: KeyEvent) -> Msg {
    match key.code {
        KeyCode::Esc => Msg::Overlay(OverlayMsg::GotoCancelled),
        KeyCode::Char('j') | KeyCode::Down => {
            Msg::Overlay(OverlayMsg::GotoMenuNavigate { delta: 1 })
        }
        KeyCode::Char('k') | KeyCode::Up => {
            Msg::Overlay(OverlayMsg::GotoMenuNavigate { delta: -1 })
        }
        KeyCode::Char(' ') | KeyCode::Enter => Msg::Overlay(OverlayMsg::GotoMenuConfirm),
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let digit = (c as u8 - b'0') as usize;
            if digit >= 1 {
                Msg::Overlay(OverlayMsg::GotoMenuQuickSelect { digit })
            } else {
                Msg::Overlay(OverlayMsg::Consumed)
            }
        }
        _ => Msg::Overlay(OverlayMsg::Consumed),
    }
}

/// Footer commands for the goto menu overlay.
pub fn footer_commands() -> Vec<FooterCommand> {
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

/// Render the goto menu overlay from the model state.
pub fn render_goto_menu(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let (targets, cursor) = match &model.active_overlay {
        Some(ActiveOverlay::GotoMenu { targets, cursor }) => (targets, *cursor),
        _ => return,
    };

    let height = (targets.len() as u16) + 2;
    let width = 30u16;
    let inner = render_overlay(area, buf, ctx, " Goto ", width, height);

    let theme = &ctx.theme;
    for (i, target) in targets.iter().enumerate() {
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
        let num = i + 1;
        let text = format!(" {num}. {}", target.label);
        let line = Line::from(Span::raw(text)).style(style);
        line.render(row_area, buf);
    }
}

/// Look up a goto target by cursor index. Returns the target, or None if out of range.
pub fn resolve_goto_target(targets: &[GotoTarget], cursor: usize) -> Option<GotoTarget> {
    targets.get(cursor).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

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
    fn handle_key_esc() {
        assert!(matches!(
            handle_key(key(KeyCode::Esc)),
            Msg::Overlay(OverlayMsg::GotoCancelled)
        ));
    }

    #[test]
    fn handle_key_j_navigate() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('j'))),
            Msg::Overlay(OverlayMsg::GotoMenuNavigate { delta: 1 })
        ));
    }

    #[test]
    fn handle_key_enter_confirm() {
        assert!(matches!(
            handle_key(key(KeyCode::Enter)),
            Msg::Overlay(OverlayMsg::GotoMenuConfirm)
        ));
    }

    #[test]
    fn handle_key_number_key() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('1'))),
            Msg::Overlay(OverlayMsg::GotoMenuQuickSelect { digit: 1 })
        ));
    }

    #[test]
    fn handle_key_zero_is_consumed() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('0'))),
            Msg::Overlay(OverlayMsg::Consumed)
        ));
    }

    #[test]
    fn handle_key_unknown() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('x'))),
            Msg::Overlay(OverlayMsg::Consumed)
        ));
    }

    #[test]
    fn resolve_goto_target_valid_index() {
        let targets = sample_targets();
        let target = resolve_goto_target(&targets, 0).unwrap();
        assert_eq!(target.label, "Flow Details");
    }

    #[test]
    fn resolve_goto_target_out_of_range() {
        let targets = sample_targets();
        assert!(resolve_goto_target(&targets, 5).is_none());
    }

    #[test]
    fn footer_commands_present() {
        let cmds = footer_commands();
        assert!(cmds.iter().any(|c| c.description == "Confirm"));
        assert!(cmds.iter().any(|c| c.description == "Close"));
    }
}
