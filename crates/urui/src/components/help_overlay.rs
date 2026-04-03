use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::input::{FooterCommand, InputHandler, InputResult};
use crate::model::{ActiveOverlay, Model};
use crate::msg::{Msg, OverlayMsg};

use super::overlay::render_overlay;

/// Overlay width: enough for the widest content line plus border padding.
const OVERLAY_WIDTH: u16 = 56;
/// Overlay height: content lines + borders.
const OVERLAY_HEIGHT: u16 = 16;

/// Modal input handler for the help overlay.
///
/// Captures all keys (modal behavior). `?` or `Esc` closes the overlay;
/// everything else is consumed as a no-op.
pub struct HelpOverlayHandler;

impl InputHandler for HelpOverlayHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        let msg = match key.code {
            KeyCode::Char('?') | KeyCode::Esc => Msg::Overlay(OverlayMsg::HelpClosed),
            _ => Msg::Overlay(OverlayMsg::Consumed),
        };
        InputResult::Capture(msg)
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![FooterCommand {
            key_label: "?/Esc".to_string(),
            description: "Close".to_string(),
            common: true,
        }]
    }

    fn name(&self) -> &str {
        "help_overlay"
    }
}

/// The help content lines displayed in the overlay.
const HELP_LINES: &[&str] = &[
    "Navigation:    j/k \u{2193}/\u{2191} \u{2014} Move down/up",
    "               h/l \u{2190}/\u{2192} \u{2014} Page left/right",
    "               Space/Enter \u{2014} Select",
    "               q/Esc \u{2014} Back",
    "",
    "Tabs:          t \u{2014} Tickets    f \u{2014} Flows    w \u{2014} Workers",
    "               Tab \u{2014} Next tab    ~ \u{2014} Help",
    "",
    "Other:         1-9 \u{2014} Menu options",
    "               , \u{2014} Settings",
    "               Q \u{2014} Quit",
];

/// Render the help (commands) overlay.
pub fn render_help_overlay(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    if !matches!(model.active_overlay, Some(ActiveOverlay::Help)) {
        return;
    }

    let inner = render_overlay(area, buf, ctx, " Commands ", OVERLAY_WIDTH, OVERLAY_HEIGHT);

    let style = Style::default()
        .fg(ctx.theme.base_content)
        .bg(ctx.theme.base_200);

    for (i, line_text) in HELP_LINES.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let line = Line::from(vec![Span::styled(*line_text, style)]);
        let line_area = Rect::new(inner.x, y, inner.width, 1);
        line.render(line_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::*;
    use crate::input::InputHandler;

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn help_overlay_handler_closes_on_question_mark() {
        let handler = HelpOverlayHandler;
        match handler.handle_key(make_key(KeyCode::Char('?'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::HelpClosed)) => {}
            other => panic!("expected HelpClosed, got {other:?}"),
        }
    }

    #[test]
    fn help_overlay_handler_closes_on_esc() {
        let handler = HelpOverlayHandler;
        match handler.handle_key(make_key(KeyCode::Esc)) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::HelpClosed)) => {}
            other => panic!("expected HelpClosed, got {other:?}"),
        }
    }

    #[test]
    fn help_overlay_handler_consumes_other_keys() {
        let handler = HelpOverlayHandler;
        match handler.handle_key(make_key(KeyCode::Char('a'))) {
            InputResult::Capture(Msg::Overlay(OverlayMsg::Consumed)) => {}
            other => panic!("expected Consumed, got {other:?}"),
        }
    }

    #[test]
    fn help_overlay_handler_is_modal() {
        let handler = HelpOverlayHandler;
        // Every key should be captured, never bubble
        for code in [
            KeyCode::Char('q'),
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::Char('j'),
        ] {
            match handler.handle_key(make_key(code)) {
                InputResult::Capture(_) => {}
                InputResult::Bubble => panic!("key {code:?} should be captured, not bubbled"),
            }
        }
    }

    #[test]
    fn help_overlay_handler_name() {
        let handler = HelpOverlayHandler;
        assert_eq!(handler.name(), "help_overlay");
    }

    #[test]
    fn help_overlay_footer_commands() {
        let handler = HelpOverlayHandler;
        let commands = handler.footer_commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].key_label, "?/Esc");
        assert_eq!(commands[0].description, "Close");
        assert!(commands[0].common);
    }

    #[test]
    fn help_lines_fit_in_overlay() {
        // Ensure the content fits within the overlay dimensions (minus borders).
        let content_height = OVERLAY_HEIGHT - 2; // top + bottom border
        assert!(
            HELP_LINES.len() <= content_height as usize,
            "help content ({} lines) exceeds overlay capacity ({} lines)",
            HELP_LINES.len(),
            content_height
        );
    }
}
