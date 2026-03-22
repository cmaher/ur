use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::FooterCommand;

/// Calculate the rendered width of a group of footer commands.
fn commands_width(commands: &[&FooterCommand]) -> u16 {
    if commands.is_empty() {
        return 0;
    }
    let mut w: u16 = 0;
    for (i, cmd) in commands.iter().enumerate() {
        if i > 0 {
            w += 2; // separator "  "
        }
        w += cmd.key_label.len() as u16 + 1 + cmd.description.len() as u16;
    }
    w
}

/// Build spans for a group of footer commands.
fn build_spans(
    commands: &[&FooterCommand],
    secondary_color: ratatui::style::Color,
    content_color: ratatui::style::Color,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, cmd) in commands.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            cmd.key_label.clone(),
            Style::default().fg(secondary_color),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            cmd.description.clone(),
            Style::default().fg(content_color),
        ));
    }
    spans
}

/// Render footer commands horizontally into the given area.
///
/// Page-specific commands (common=false) are rendered on the left.
/// Common commands (common=true) are rendered on the right side,
/// and are hidden entirely if there is not enough space.
pub fn render_footer(area: Rect, buf: &mut Buffer, ctx: &TuiContext, commands: &[FooterCommand]) {
    let theme = &ctx.theme;

    // Fill the entire footer row with neutral background first.
    let bg_style = Style::default().bg(theme.neutral).fg(theme.neutral_content);
    buf.set_style(area, bg_style);

    let left_cmds: Vec<&FooterCommand> = commands.iter().filter(|c| !c.common).collect();
    let right_cmds: Vec<&FooterCommand> = commands.iter().filter(|c| c.common).collect();

    let left_width = commands_width(&left_cmds);
    let right_width = commands_width(&right_cmds);

    // Always render left commands
    if !left_cmds.is_empty() {
        let spans = build_spans(&left_cmds, theme.secondary, theme.base_content);
        let line = Line::from(spans);
        line.render(area, buf);
    }

    // Render right commands only if they fit (with 2-char gap from left)
    let gap = if left_cmds.is_empty() { 0u16 } else { 2u16 };
    if !right_cmds.is_empty() && left_width + gap + right_width <= area.width {
        let right_x = area.x + area.width - right_width;
        let right_area = Rect::new(right_x, area.y, right_width, 1);
        let spans = build_spans(&right_cmds, theme.secondary, theme.base_content);
        let line = Line::from(spans);
        line.render(right_area, buf);
    }
}
