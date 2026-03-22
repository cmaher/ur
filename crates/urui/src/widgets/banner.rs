use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::{Banner, BannerVariant};

/// Render a single-line banner notification into the given area.
///
/// Success banners use `theme.success` background with `theme.success_content`
/// foreground; error banners use `theme.error` background with
/// `theme.error_content` foreground.
pub fn render_banner(area: Rect, buf: &mut Buffer, ctx: &TuiContext, banner: &Banner) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use crate::keymap::Keymap;
    use crate::theme::Theme;
    use ur_config::TuiConfig;

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
        }
    }

    #[test]
    fn render_success_banner_does_not_panic() {
        let ctx = make_ctx();
        let banner = Banner {
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
        let banner = Banner {
            message: "Something went wrong".to_string(),
            variant: BannerVariant::Error,
            created_at: Instant::now(),
        };
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        render_banner(area, &mut buf, &ctx, &banner);
    }
}
