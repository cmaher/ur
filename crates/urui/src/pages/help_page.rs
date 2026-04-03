use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use ur_markdown::{MarkdownColors, render_markdown};

use crate::context::TuiContext;
use crate::input::MarkdownScrollHandler;
use crate::model::Model;

/// Static help content embedded at compile time from docs/help.md.
const HELP_CONTENT: &str = include_str!("../../../../docs/help.md");

/// Render the help page into the given content area.
///
/// Shows the static help guide content with markdown rendering and scrolling.
/// Help content is embedded at compile time, so it is always available immediately.
pub fn render_help_page(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let help_model = &model.help_page;

    let colors = markdown_colors(ctx);
    let all_lines = render_markdown(HELP_CONTENT, area.width as usize, &colors);
    let visible_height = area.height as usize;
    let total = all_lines.len();

    // Update cached metrics for use by the next scroll action.
    help_model.last_total_lines.set(total);

    // Clamp scroll offset to valid range.
    let max_offset = total.saturating_sub(visible_height);
    let offset = help_model.scroll_offset.min(max_offset);

    let visible: Vec<Line<'static>> = all_lines
        .into_iter()
        .skip(offset)
        .take(visible_height)
        .collect();

    let bg_style = Style::default().bg(ctx.theme.base_100);
    Paragraph::new(visible).style(bg_style).render(area, buf);
}

/// Build `MarkdownColors` from the TUI theme.
fn markdown_colors(ctx: &TuiContext) -> MarkdownColors {
    MarkdownColors {
        text: ctx.theme.base_content,
        heading: ctx.theme.accent,
        code: ctx.theme.warning,
        dim: ctx.theme.neutral_content,
    }
}

/// Initialize the help page by resetting scroll state and pushing the scroll handler.
pub fn init_help_page(model: &mut Model) {
    model.help_page.scroll_offset = 0;
    model.help_page.last_total_lines.set(0);
    model.input_stack.push(Box::new(help_page_handler()));
}

/// Create the input handler for the help page.
///
/// Uses the shared `MarkdownScrollHandler` for scroll navigation
/// (j/k for lines, h/l/Ctrl+f/Ctrl+b for pages).
fn help_page_handler() -> MarkdownScrollHandler {
    MarkdownScrollHandler {
        handler_name: "help_page",
    }
}

/// Handle scroll-down for the help page.
pub fn help_scroll_down(model: &mut Model, delta: usize) {
    let total = model.help_page.last_total_lines.get();
    // Use a reasonable default visible height for clamping.
    let height = 20usize; // will be corrected on next render
    let max_offset = total.saturating_sub(height);
    model.help_page.scroll_offset = (model.help_page.scroll_offset + delta).min(max_offset);
}

/// Handle scroll-up for the help page.
pub fn help_scroll_up(model: &mut Model, delta: usize) {
    model.help_page.scroll_offset = model.help_page.scroll_offset.saturating_sub(delta);
}

/// Handle page-down for the help page.
pub fn help_page_down(model: &mut Model) {
    let total = model.help_page.last_total_lines.get();
    let page = 20usize; // default page size
    let max_offset = total.saturating_sub(page);
    model.help_page.scroll_offset = (model.help_page.scroll_offset + page).min(max_offset);
}

/// Handle page-up for the help page.
pub fn help_page_up(model: &mut Model) {
    let page = 20usize;
    model.help_page.scroll_offset = model.help_page.scroll_offset.saturating_sub(page);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_content_is_not_empty() {
        assert!(!HELP_CONTENT.is_empty());
    }

    #[test]
    fn init_resets_help_model() {
        let mut model = Model::initial();
        model.help_page.scroll_offset = 5;
        init_help_page(&mut model);
        assert_eq!(model.help_page.scroll_offset, 0);
    }

    #[test]
    fn scroll_down_increments_offset() {
        let mut model = Model::initial();
        model.help_page.last_total_lines.set(100);
        help_scroll_down(&mut model, 3);
        assert_eq!(model.help_page.scroll_offset, 3);
    }

    #[test]
    fn scroll_up_decrements_offset() {
        let mut model = Model::initial();
        model.help_page.scroll_offset = 10;
        help_scroll_up(&mut model, 3);
        assert_eq!(model.help_page.scroll_offset, 7);
    }

    #[test]
    fn scroll_up_clamps_to_zero() {
        let mut model = Model::initial();
        help_scroll_up(&mut model, 10);
        assert_eq!(model.help_page.scroll_offset, 0);
    }

    #[test]
    fn page_down_scrolls_by_page() {
        let mut model = Model::initial();
        model.help_page.last_total_lines.set(200);
        help_page_down(&mut model);
        assert_eq!(model.help_page.scroll_offset, 20);
    }

    #[test]
    fn page_up_scrolls_by_page() {
        let mut model = Model::initial();
        model.help_page.scroll_offset = 30;
        help_page_up(&mut model);
        assert_eq!(model.help_page.scroll_offset, 10);
    }

    #[test]
    fn handler_name() {
        let handler = help_page_handler();
        assert_eq!(handler.handler_name, "help_page");
    }
}
