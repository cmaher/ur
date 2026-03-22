use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::StatusMessage;

/// Render a single-line status header into the given area.
///
/// Uses `theme.secondary` background with `theme.secondary_content` foreground
/// to distinguish from success/error banners.
pub fn render_status_header(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    status: &StatusMessage,
) {
    let theme = &ctx.theme;
    let style = Style::default()
        .bg(theme.secondary)
        .fg(theme.secondary_content);

    // Fill the entire row with the status background.
    buf.set_style(area, style);

    let text = format!(" {} ", status.text);
    let line = Line::from(Span::styled(text, style));
    line.render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn render_status_header_does_not_panic() {
        let ctx = make_ctx();
        let status = StatusMessage {
            text: "Dispatching ticket ur-abc12...".to_string(),
            dismissable: true,
        };
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        render_status_header(area, &mut buf, &ctx, &status);
    }

    #[test]
    fn render_status_header_content() {
        let ctx = make_ctx();
        let status = StatusMessage {
            text: "Refreshing tickets...".to_string(),
            dismissable: true,
        };
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        render_status_header(area, &mut buf, &ctx, &status);

        // Verify the text appears in the buffer (with padding)
        let rendered: String = (0..area.width)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(rendered.contains("Refreshing tickets..."));
    }
}
