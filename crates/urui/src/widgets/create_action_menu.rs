use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::FooterCommand;
use crate::widgets::overlay::render_overlay;

/// The four actions available after editing a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateAction {
    Create,
    Dispatch,
    Design,
    Abandon,
}

const ACTIONS: &[(CreateAction, &str)] = &[
    (CreateAction::Create, "Create"),
    (CreateAction::Dispatch, "Create & Dispatch"),
    (CreateAction::Design, "Create as Design"),
    (CreateAction::Abandon, "Abandon"),
];

/// Result of handling a key event in the create action menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateActionResult {
    /// The menu consumed the event; stay open.
    Consumed,
    /// The user selected an action.
    Selected(CreateAction),
}

/// Summary of a ticket pending creation.
#[derive(Debug, Clone)]
pub struct PendingTicket {
    pub project: String,
    pub title: String,
    pub priority: i64,
}

/// State for the create action menu overlay.
pub struct CreateActionMenuState {
    /// Current cursor position (0-indexed, maps to ACTIONS index).
    cursor: usize,
    /// The pending ticket to display in the summary.
    pending: PendingTicket,
}

impl CreateActionMenuState {
    pub fn new(pending: PendingTicket) -> Self {
        Self { cursor: 0, pending }
    }

    /// Handle a raw key event. Returns the result indicating what happened.
    pub fn handle_key(&mut self, key: KeyEvent) -> CreateActionResult {
        match key.code {
            KeyCode::Esc => CreateActionResult::Selected(CreateAction::Abandon),
            KeyCode::Char('j') | KeyCode::Down => {
                self.cursor = (self.cursor + 1) % ACTIONS.len();
                CreateActionResult::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor = (self.cursor + ACTIONS.len() - 1) % ACTIONS.len();
                CreateActionResult::Consumed
            }
            KeyCode::Enter => {
                let (action, _) = ACTIONS[self.cursor];
                CreateActionResult::Selected(action)
            }
            KeyCode::Char(c) if ('1'..='4').contains(&c) => {
                let index = (c as u8 - b'1') as usize;
                let (action, _) = ACTIONS[index];
                CreateActionResult::Selected(action)
            }
            _ => CreateActionResult::Consumed,
        }
    }

    /// Render the create action menu as a centered overlay.
    pub fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        // 3 lines for summary (project, title, priority) + 1 blank + 4 options + 2 borders
        let height = 10u16;
        let width = 50u16;
        let inner = render_overlay(area, buf, ctx, " Create Ticket ", width, height);

        let theme = &ctx.theme;
        let summary_style = Style::default().fg(theme.base_content).bg(theme.base_200);

        // Render summary lines
        self.render_summary_lines(inner, buf, summary_style);

        // Blank separator line at row 3
        // Render action options starting at row 4
        self.render_action_options(inner, buf, theme);
    }

    fn render_summary_lines(&self, inner: Rect, buf: &mut Buffer, style: Style) {
        let summary_lines = [
            format!(" Project:  {}", self.pending.project),
            format!(" Title:    {}", self.pending.title),
            format!(" Priority: P{}", self.pending.priority),
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

    fn render_action_options(&self, inner: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let options_start = 4u16;
        for (i, (_, label)) in ACTIONS.iter().enumerate() {
            let row_idx = options_start + i as u16;
            if row_idx >= inner.height {
                break;
            }
            let row_area = Rect::new(inner.x, inner.y + row_idx, inner.width, 1);
            let is_selected = i == self.cursor;

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

    /// Footer commands to show when the create action menu is open.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pending() -> PendingTicket {
        PendingTicket {
            project: "ur".to_string(),
            title: "Test ticket".to_string(),
            priority: 2,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn initial_cursor_is_zero() {
        let state = CreateActionMenuState::new(make_pending());
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn navigate_down_wraps() {
        let mut state = CreateActionMenuState::new(make_pending());
        // Move to last item
        for _ in 0..3 {
            state.handle_key(key(KeyCode::Char('j')));
        }
        assert_eq!(state.cursor, 3);
        // Wrap to first
        let r = state.handle_key(key(KeyCode::Char('j')));
        assert_eq!(r, CreateActionResult::Consumed);
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn navigate_up_wraps() {
        let mut state = CreateActionMenuState::new(make_pending());
        assert_eq!(state.cursor, 0);
        let r = state.handle_key(key(KeyCode::Char('k')));
        assert_eq!(r, CreateActionResult::Consumed);
        assert_eq!(state.cursor, 3);
    }

    #[test]
    fn arrow_keys_navigate() {
        let mut state = CreateActionMenuState::new(make_pending());
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.cursor, 1);
        state.handle_key(key(KeyCode::Up));
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn enter_confirms_selection() {
        let mut state = CreateActionMenuState::new(make_pending());
        state.handle_key(key(KeyCode::Down)); // cursor = 1 (Dispatch)
        let r = state.handle_key(key(KeyCode::Enter));
        assert_eq!(r, CreateActionResult::Selected(CreateAction::Dispatch));
    }

    #[test]
    fn hotkey_1_selects_create() {
        let mut state = CreateActionMenuState::new(make_pending());
        let r = state.handle_key(key(KeyCode::Char('1')));
        assert_eq!(r, CreateActionResult::Selected(CreateAction::Create));
    }

    #[test]
    fn hotkey_2_selects_dispatch() {
        let mut state = CreateActionMenuState::new(make_pending());
        let r = state.handle_key(key(KeyCode::Char('2')));
        assert_eq!(r, CreateActionResult::Selected(CreateAction::Dispatch));
    }

    #[test]
    fn hotkey_3_selects_design() {
        let mut state = CreateActionMenuState::new(make_pending());
        let r = state.handle_key(key(KeyCode::Char('3')));
        assert_eq!(r, CreateActionResult::Selected(CreateAction::Design));
    }

    #[test]
    fn hotkey_4_selects_abandon() {
        let mut state = CreateActionMenuState::new(make_pending());
        let r = state.handle_key(key(KeyCode::Char('4')));
        assert_eq!(r, CreateActionResult::Selected(CreateAction::Abandon));
    }

    #[test]
    fn escape_returns_abandon() {
        let mut state = CreateActionMenuState::new(make_pending());
        let r = state.handle_key(key(KeyCode::Esc));
        assert_eq!(r, CreateActionResult::Selected(CreateAction::Abandon));
    }

    #[test]
    fn unknown_key_consumed() {
        let mut state = CreateActionMenuState::new(make_pending());
        let r = state.handle_key(key(KeyCode::Char('x')));
        assert_eq!(r, CreateActionResult::Consumed);
    }

    #[test]
    fn footer_commands_present() {
        let state = CreateActionMenuState::new(make_pending());
        let cmds = state.footer_commands();
        assert_eq!(cmds.len(), 4);
        assert!(cmds.iter().any(|c| c.description == "Navigate"));
        assert!(cmds.iter().any(|c| c.description == "Confirm"));
        assert!(cmds.iter().any(|c| c.description == "Quick select"));
        assert!(cmds.iter().any(|c| c.description == "Abandon"));
    }
}
