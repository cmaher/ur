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
