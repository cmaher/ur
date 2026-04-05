use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use ur_markdown::{MarkdownColors, render_markdown};

use crate::context::TuiContext;
use crate::model::ScrollableMarkdownModel;

/// Build `MarkdownColors` from the TUI theme.
pub fn markdown_colors(ctx: &TuiContext) -> MarkdownColors {
    MarkdownColors {
        text: ctx.theme.base_content,
        heading: ctx.theme.accent,
        code: ctx.theme.secondary,
        dim: ctx.theme.neutral_content,
    }
}

/// Render markdown content with scrolling into the given area.
///
/// This is the shared render function used by both the help page and the ticket
/// body page. It renders the markdown, caches the total line count and visible
/// height on the scroll model, clamps the scroll offset, slices out the visible
/// lines, and draws them with a `Paragraph`.
pub fn render_scrollable_markdown(
    content: &str,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    scroll: &ScrollableMarkdownModel,
) {
    let colors = markdown_colors(ctx);
    let all_lines = render_markdown(content, area.width as usize, &colors);
    let visible_height = area.height as usize;
    let total = all_lines.len();

    // Update cached metrics for use by the next scroll action.
    scroll.last_body_height.set(visible_height.max(1));
    scroll.last_total_lines.set(total);

    // Clamp scroll offset to valid range.
    let max_offset = total.saturating_sub(visible_height);
    let offset = scroll.scroll_offset.min(max_offset);

    let visible: Vec<Line<'static>> = all_lines
        .into_iter()
        .skip(offset)
        .take(visible_height)
        .collect();

    let bg_style = Style::default().bg(ctx.theme.base_100);
    Paragraph::new(visible).style(bg_style).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashMap;

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ur_config::TuiConfig;

    use crate::keymap::Keymap;
    use crate::theme::Theme;

    fn test_ctx() -> TuiContext {
        let tui_config = TuiConfig::default();
        TuiContext {
            theme: Theme::resolve(&tui_config),
            keymap: Keymap::default(),
            projects: Vec::new(),
            project_configs: HashMap::new(),
            tui_config,
            config_dir: std::path::PathBuf::new(),
            project_filter: None,
        }
    }

    fn make_scroll(offset: usize) -> ScrollableMarkdownModel {
        ScrollableMarkdownModel {
            scroll_offset: offset,
            last_body_height: Cell::new(0),
            last_total_lines: Cell::new(0),
        }
    }

    #[test]
    fn render_caches_total_lines_and_height() {
        let ctx = test_ctx();
        let scroll = make_scroll(0);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);

        render_scrollable_markdown("Hello\n\nWorld", area, &mut buf, &ctx, &scroll);

        assert!(scroll.last_total_lines.get() > 0);
        assert_eq!(scroll.last_body_height.get(), 10);
    }

    #[test]
    fn render_clamps_offset_beyond_total() {
        let ctx = test_ctx();
        let scroll = make_scroll(999);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);

        // Should not panic even with a huge offset.
        render_scrollable_markdown("Short", area, &mut buf, &ctx, &scroll);

        assert!(scroll.last_total_lines.get() > 0);
    }

    #[test]
    fn render_empty_content() {
        let ctx = test_ctx();
        let scroll = make_scroll(0);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);

        render_scrollable_markdown("", area, &mut buf, &ctx, &scroll);

        assert_eq!(scroll.last_body_height.get(), 10);
    }

    #[test]
    fn markdown_colors_maps_theme() {
        let ctx = test_ctx();
        let colors = markdown_colors(&ctx);
        assert_eq!(colors.text, ctx.theme.base_content);
        assert_eq!(colors.heading, ctx.theme.accent);
        assert_eq!(colors.code, ctx.theme.secondary);
        assert_eq!(colors.dim, ctx.theme.neutral_content);
    }

    #[test]
    fn render_zero_height_area() {
        let ctx = test_ctx();
        let scroll = make_scroll(0);
        let area = Rect::new(0, 0, 40, 0);
        let mut buf = Buffer::empty(area);

        render_scrollable_markdown("Content", area, &mut buf, &ctx, &scroll);

        // last_body_height should be clamped to at least 1.
        assert_eq!(scroll.last_body_height.get(), 1);
    }
}
