use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::input::{FooterCommand, InputHandler, InputResult};
use crate::model::BannerModel;
use crate::msg::Msg;

/// Render a single-line banner notification into the given area.
///
/// Success banners use `theme.success` background with `theme.success_content`
/// foreground; error banners use `theme.error` background with
/// `theme.error_content` foreground.
pub fn render_banner(area: Rect, buf: &mut Buffer, ctx: &TuiContext, banner: &BannerModel) {
    let theme = &ctx.theme;

    let style = match banner.variant {
        BannerVariant::Success => Style::default().bg(theme.success).fg(theme.success_content),
        BannerVariant::Error => Style::default().bg(theme.error).fg(theme.error_content),
    };

    // Fill the entire row with the banner background.
    buf.set_style(area, style);

    let text = format!(" {} ", banner.message);
    let line = Line::from(Span::styled(text, style));
    line.render(area, buf);
}

/// Visual variant controlling banner color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerVariant {
    Success,
    Error,
}

/// Input handler for an active banner.
///
/// Captures Enter and Esc to dismiss the banner, bubbles all other keys
/// so underlying handlers can still process them.
pub struct BannerHandler;

impl InputHandler for BannerHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => InputResult::Capture(Msg::BannerDismiss),
            _ => InputResult::Bubble,
        }
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![FooterCommand {
            key_label: "Enter/Esc".to_string(),
            description: "Dismiss".to_string(),
            common: false,
        }]
    }

    fn name(&self) -> &str {
        "banner"
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use std::time::Instant;

    use super::*;
    use crate::keymap::Keymap;
    use crate::theme::Theme;
    use ur_config::TuiConfig;

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn make_ctx() -> TuiContext {
        let tui_config = TuiConfig::default();
        let theme = Theme::resolve(&tui_config);
        let keymap = Keymap::default();
        TuiContext {
            theme,
            keymap,
            projects: vec![],
            project_configs: std::collections::HashMap::new(),
            tui_config: TuiConfig::default(),
            config_dir: std::path::PathBuf::from("/tmp/test-urui"),
            project_filter: None,
        }
    }

    #[test]
    fn banner_handler_captures_enter() {
        let handler = BannerHandler;
        let key = make_key(KeyCode::Enter, KeyModifiers::NONE);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::BannerDismiss) => {}
            other => panic!("expected Capture(BannerDismiss), got {other:?}"),
        }
    }

    #[test]
    fn banner_handler_captures_esc() {
        let handler = BannerHandler;
        let key = make_key(KeyCode::Esc, KeyModifiers::NONE);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::BannerDismiss) => {}
            other => panic!("expected Capture(BannerDismiss), got {other:?}"),
        }
    }

    #[test]
    fn banner_handler_bubbles_other_keys() {
        let handler = BannerHandler;
        let key = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Bubble));
    }

    #[test]
    fn banner_handler_bubbles_ctrl_c() {
        let handler = BannerHandler;
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(handler.handle_key(key), InputResult::Bubble));
    }

    #[test]
    fn banner_handler_bubbles_tab() {
        let handler = BannerHandler;
        let key = make_key(KeyCode::Tab, KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Bubble));
    }

    #[test]
    fn banner_handler_footer_commands() {
        let handler = BannerHandler;
        let commands = handler.footer_commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].key_label, "Enter/Esc");
        assert_eq!(commands[0].description, "Dismiss");
    }

    #[test]
    fn banner_handler_name() {
        let handler = BannerHandler;
        assert_eq!(handler.name(), "banner");
    }

    #[test]
    fn render_success_banner_does_not_panic() {
        let ctx = make_ctx();
        let banner = BannerModel {
            message: "Operation succeeded".to_string(),
            variant: BannerVariant::Success,
            created_at: Instant::now(),
        };
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        render_banner(area, &mut buf, &ctx, &banner);
    }

    #[test]
    fn render_error_banner_does_not_panic() {
        let ctx = make_ctx();
        let banner = BannerModel {
            message: "Something went wrong".to_string(),
            variant: BannerVariant::Error,
            created_at: Instant::now(),
        };
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        render_banner(area, &mut buf, &ctx, &banner);
    }

    #[test]
    fn banner_handler_in_stack_captures_enter_before_global() {
        use crate::input::{GlobalHandler, InputStack};

        let mut stack = InputStack::default();
        stack.push(Box::new(GlobalHandler));
        stack.push(Box::new(BannerHandler));

        // Enter should be captured by BannerHandler (top), not GlobalHandler
        let key = make_key(KeyCode::Enter, KeyModifiers::NONE);
        let result = stack.dispatch(key);
        assert!(matches!(result, Some(Msg::BannerDismiss)));
    }

    #[test]
    fn banner_handler_in_stack_bubbles_ctrl_c_to_global() {
        use crate::input::{GlobalHandler, InputStack};

        let mut stack = InputStack::default();
        stack.push(Box::new(GlobalHandler));
        stack.push(Box::new(BannerHandler));

        // Ctrl+C should bubble past BannerHandler and be captured by GlobalHandler
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let result = stack.dispatch(key);
        assert!(matches!(result, Some(Msg::Quit)));
    }
}
