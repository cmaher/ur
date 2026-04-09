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
use crate::msg::{CreateAction, Msg, OverlayMsg};

/// The actions and their display labels.
const ACTIONS: &[(CreateAction, &str)] = &[
    (CreateAction::Create, "Create"),
    (CreateAction::Dispatch, "Create & Dispatch"),
    (CreateAction::Edit, "Edit"),
    (CreateAction::Abandon, "Abandon"),
];

/// Handle a key event for the create action menu overlay.
///
/// All keys are captured (modal). j/k navigate (wrapping), Enter confirms,
/// 1-4 quick-select, Esc selects Abandon.
pub fn handle_key(key: KeyEvent) -> Msg {
    match key.code {
        KeyCode::Esc => Msg::Overlay(OverlayMsg::CreateActionSelected(CreateAction::Abandon)),
        KeyCode::Char('j') | KeyCode::Down => {
            Msg::Overlay(OverlayMsg::CreateActionNavigate { delta: 1 })
        }
        KeyCode::Char('k') | KeyCode::Up => {
            Msg::Overlay(OverlayMsg::CreateActionNavigate { delta: -1 })
        }
        KeyCode::Enter => Msg::Overlay(OverlayMsg::CreateActionConfirm),
        KeyCode::Char(c) if ('1'..='4').contains(&c) => {
            let index = (c as u8 - b'1') as usize;
            Msg::Overlay(OverlayMsg::CreateActionQuickSelect { index })
        }
        _ => Msg::Overlay(OverlayMsg::Consumed),
    }
}

/// Footer commands for the create action menu overlay.
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
            key_label: "1-4".to_string(),
            description: "Quick select".to_string(),
            common: false,
        },
        FooterCommand {
            key_label: "Esc".to_string(),
            description: "Abandon".to_string(),
            common: false,
        },
    ]
}

/// Render the create action menu overlay from the model state.
pub fn render_create_action_menu(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let (pending, cursor) = match &model.active_overlay {
        Some(ActiveOverlay::CreateActionMenu { pending, cursor }) => (pending, *cursor),
        _ => return,
    };

    // 3 lines for summary + 1 blank + 4 options + 2 borders
    let height = 10u16;
    let width = 50u16;
    let inner = render_overlay(area, buf, ctx, " Create Ticket ", width, height);

    let theme = &ctx.theme;
    let summary_style = Style::default().fg(theme.base_content).bg(theme.base_200);

    // Render summary lines
    render_summary_lines(inner, buf, summary_style, pending);

    // Render action options starting at row 4
    render_action_options(inner, buf, theme, cursor);
}

fn render_summary_lines(
    inner: Rect,
    buf: &mut Buffer,
    style: Style,
    pending: &crate::msg::PendingTicket,
) {
    let summary_lines = [
        format!(" Project:  {}", pending.project),
        format!(" Title:    {}", pending.title),
        format!(" Priority: P{}", pending.priority),
    ];
    for (i, text) in summary_lines.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let row_area = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        buf.set_style(row_area, style);
        let line = Line::from(Span::raw(text.as_str())).style(style);
        line.render(row_area, buf);
    }
}

fn render_action_options(
    inner: Rect,
    buf: &mut Buffer,
    theme: &crate::theme::Theme,
    cursor: usize,
) {
    let options_start = 4u16;
    for (i, (_, label)) in ACTIONS.iter().enumerate() {
        let row_idx = options_start + i as u16;
        if row_idx >= inner.height {
            break;
        }
        let row_area = Rect::new(inner.x, inner.y + row_idx, inner.width, 1);
        let is_selected = i == cursor;

        let style = if is_selected {
            Style::default().fg(theme.primary_content).bg(theme.primary)
        } else {
            Style::default().fg(theme.base_content).bg(theme.base_200)
        };

        buf.set_style(row_area, style);
        let num = i + 1;
        let text = format!(" {num}  {label}");
        let line = Line::from(Span::raw(text)).style(style);
        line.render(row_area, buf);
    }
}

/// Returns the number of available create actions.
pub fn action_count() -> usize {
    ACTIONS.len()
}

/// Look up the action at the given cursor index.
pub fn action_at(index: usize) -> Option<CreateAction> {
    ACTIONS.get(index).map(|(action, _)| *action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn handle_key_esc_selects_abandon() {
        assert!(matches!(
            handle_key(key(KeyCode::Esc)),
            Msg::Overlay(OverlayMsg::CreateActionSelected(CreateAction::Abandon))
        ));
    }

    #[test]
    fn handle_key_enter_confirms() {
        assert!(matches!(
            handle_key(key(KeyCode::Enter)),
            Msg::Overlay(OverlayMsg::CreateActionConfirm)
        ));
    }

    #[test]
    fn handle_key_j_navigates() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('j'))),
            Msg::Overlay(OverlayMsg::CreateActionNavigate { delta: 1 })
        ));
    }

    #[test]
    fn handle_key_quick_select_1() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('1'))),
            Msg::Overlay(OverlayMsg::CreateActionQuickSelect { index: 0 })
        ));
    }

    #[test]
    fn handle_key_quick_select_4() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('4'))),
            Msg::Overlay(OverlayMsg::CreateActionQuickSelect { index: 3 })
        ));
    }

    #[test]
    fn handle_key_quick_select_5_consumed() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('5'))),
            Msg::Overlay(OverlayMsg::Consumed)
        ));
    }

    #[test]
    fn handle_key_unknown_consumed() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('x'))),
            Msg::Overlay(OverlayMsg::Consumed)
        ));
    }

    #[test]
    fn action_at_valid() {
        assert_eq!(action_at(0), Some(CreateAction::Create));
        assert_eq!(action_at(1), Some(CreateAction::Dispatch));
        assert_eq!(action_at(2), Some(CreateAction::Edit));
        assert_eq!(action_at(3), Some(CreateAction::Abandon));
    }

    #[test]
    fn action_at_invalid() {
        assert_eq!(action_at(10), None);
    }

    #[test]
    fn footer_commands_present() {
        let cmds = footer_commands();
        assert!(cmds.iter().any(|c| c.description == "Confirm"));
        assert!(cmds.iter().any(|c| c.description == "Abandon"));
    }
}
