use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::context::TuiContext;

/// Render a floating overlay box centered in the given area.
///
/// Clears the overlay region and draws a bordered box with the given title.
/// Returns the inner content area (inside the border) for the caller to
/// render content into.
///
/// This is the v2 equivalent of `crate::widgets::overlay::render_overlay`,
/// reusing the same visual style.
pub fn render_overlay(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    title: &str,
    width: u16,
    height: u16,
) -> Rect {
    let theme = &ctx.theme;

    // Center the overlay in the available area, clamping to bounds.
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;

    let overlay_rect = Rect::new(x, y, w, h);

    // Clear the region behind the overlay.
    Clear.render(overlay_rect, buf);

    // Fill with overlay background.
    let bg_style = Style::default().bg(theme.base_200).fg(theme.base_content);
    buf.set_style(overlay_rect, bg_style);

    let border_set = if theme.border_rounded {
        ratatui::symbols::border::ROUNDED
    } else {
        ratatui::symbols::border::PLAIN
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent).bg(theme.base_200))
        .border_set(border_set);

    let inner = block.inner(overlay_rect);
    block.render(overlay_rect, buf);

    inner
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
        TuiContext {
            theme,
            keymap: Keymap::default(),
            projects: vec![],
            project_configs: std::collections::HashMap::new(),
            tui_config,
            config_dir: std::path::PathBuf::from("/tmp/test"),
            project_filter: None,
        }
    }

    #[test]
    fn render_overlay_returns_inner_rect() {
        let ctx = make_ctx();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let inner = render_overlay(area, &mut buf, &ctx, " Test ", 30, 10);
        // Inner should be smaller than the overlay (borders consume space).
        assert!(inner.width < 30);
        assert!(inner.height < 10);
    }

    #[test]
    fn render_overlay_clamps_to_area() {
        let ctx = make_ctx();
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        let inner = render_overlay(area, &mut buf, &ctx, " Big ", 100, 100);
        // Should be clamped to area size.
        assert!(inner.width <= area.width);
        assert!(inner.height <= area.height);
    }
}
