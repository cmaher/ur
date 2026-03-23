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

    let mut spans: Vec<Span> = tabs
        .iter()
        .map(|tab| {
            let label_lower = tab.label.to_lowercase();
            let text = format!(" ({}){}  ", tab.shortcut, &label_lower[1..]);
            let style = if tab.id == active {
                Style::default().bg(theme.primary).fg(theme.primary_content)
            } else {
                Style::default().bg(theme.base_200).fg(theme.base_content)
            };
            Span::styled(text, style)
        })
        .collect();

    // Fill the entire header row with base_200 background first.
    let bg_style = Style::default().bg(theme.base_200).fg(theme.base_content);
    buf.set_style(area, bg_style);

    // If a project filter is active, right-align it by inserting a filler span.
    if let Some(ref proj) = ctx.project_filter {
        let label = format!(" [{proj}] ");
        let tabs_width: usize = spans.iter().map(|s| s.width()).sum();
        let label_width = label.len();
        let total_width = area.width as usize;
        let gap = total_width.saturating_sub(tabs_width + label_width);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(
            label,
            Style::default()
                .bg(theme.secondary)
                .fg(theme.secondary_content),
        ));
    }

    let line = Line::from(spans);
    line.render(area, buf);
}
