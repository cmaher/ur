use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::TabId;

/// A tab descriptor for the header bar.
pub struct TabInfo {
    pub id: TabId,
    pub label: String,
    pub shortcut: char,
}

/// Render a tab bar header into the given area.
///
/// Each tab is displayed as `(x)label` where `x` is the shortcut character.
/// The active tab uses `primary` bg / `primary_content` fg; inactive tabs use
/// `base_200` bg / `base_content` fg.
pub fn render_header(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    tabs: &[TabInfo],
    active: TabId,
) {
    let theme = &ctx.theme;

    let spans: Vec<Span> = tabs
        .iter()
        .map(|tab| {
            let text = format!(" ({}){}  ", tab.shortcut, tab.label);
            let style = if tab.id == active {
                Style::default()
                    .bg(theme.primary)
                    .fg(theme.primary_content)
            } else {
                Style::default()
                    .bg(theme.base_200)
                    .fg(theme.base_content)
            };
            Span::styled(text, style)
        })
        .collect();

    let line = Line::from(spans);
    line.render(area, buf);
}
