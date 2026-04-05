use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::components::scrollable_markdown::render_scrollable_markdown;
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
    render_scrollable_markdown(HELP_CONTENT, area, buf, ctx, &model.help_page.scroll);
}

/// Initialize the help page by resetting scroll state and pushing the scroll handler.
pub fn init_help_page(model: &mut Model) {
    model.help_page.scroll.scroll_offset = 0;
    model.help_page.scroll.last_body_height.set(0);
    model.help_page.scroll.last_total_lines.set(0);
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
        model.help_page.scroll.scroll_offset = 5;
        init_help_page(&mut model);
        assert_eq!(model.help_page.scroll.scroll_offset, 0);
    }

    #[test]
    fn handler_name() {
        let handler = help_page_handler();
        assert_eq!(handler.handler_name, "help_page");
    }
}
