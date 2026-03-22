use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::FooterCommand;

/// Render footer commands horizontally into the given area.
///
/// Key labels are shown in `secondary` color; descriptions in `base_content`.
pub fn render_footer(area: Rect, buf: &mut Buffer, ctx: &TuiContext, commands: &[FooterCommand]) {
    let theme = &ctx.theme;

    let mut spans: Vec<Span> = Vec::new();
    for (i, cmd) in commands.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            &cmd.key_label,
            Style::default().fg(theme.secondary),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            &cmd.description,
            Style::default().fg(theme.base_content),
        ));
    }

    let line = Line::from(spans);
    line.render(area, buf);
}
