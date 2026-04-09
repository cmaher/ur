use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
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

/// Dispatch a key event when a banner is active.
///
/// Returns `Some(Msg::BannerDismiss)` for Enter/Esc, `None` for everything
/// else (allowing fallthrough to the input stack and page handlers).
pub fn dispatch_banner_key(key: KeyEvent) -> Option<Msg> {
    match key.code {
        KeyCode::Enter | KeyCode::Esc => Some(Msg::BannerDismiss),
        _ => None,
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
    fn dispatch_banner_key_captures_enter() {
        let key = make_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(dispatch_banner_key(key), Some(Msg::BannerDismiss)));
    }

    #[test]
    fn dispatch_banner_key_captures_esc() {
        let key = make_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(dispatch_banner_key(key), Some(Msg::BannerDismiss)));
    }

    #[test]
    fn dispatch_banner_key_returns_none_for_other_keys() {
        let key = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(dispatch_banner_key(key).is_none());
    }

    #[test]
    fn dispatch_banner_key_returns_none_for_ctrl_c() {
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(dispatch_banner_key(key).is_none());
    }

    #[test]
    fn dispatch_banner_key_returns_none_for_tab() {
        let key = make_key(KeyCode::Tab, KeyModifiers::NONE);
        assert!(dispatch_banner_key(key).is_none());
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
}
