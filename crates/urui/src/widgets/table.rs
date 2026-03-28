use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Row, StatefulWidget, Table, TableState};

use crate::context::TuiContext;

/// Theme-aware table widget.
///
/// Wraps ratatui's `Table` with colors pulled from the active theme:
/// - Header: `neutral` bg, `neutral_content` fg, bold
/// - Selected row: `primary` bg, `primary_content` fg
/// - Normal rows: alternating `base_100`/`base_200` bg, `base_content` fg
/// - Border: `base_300`, rounded or angular per `theme.border_rounded`
pub struct ThemedTable<'a> {
    pub headers: Vec<&'a str>,
    pub rows: Vec<Vec<String>>,
    pub selected: Option<usize>,
    pub widths: Vec<Constraint>,
    pub page_info: Option<String>,
}

impl<'a> ThemedTable<'a> {
    /// Render the themed table into the given area.
    pub fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        let theme = &ctx.theme;

        let header_cells: Vec<Cell> = self
            .headers
            .iter()
            .map(|h| {
                Cell::from(Span::styled(
                    *h,
                    Style::default()
                        .fg(theme.neutral_content)
                        .bg(theme.neutral)
                        .add_modifier(Modifier::BOLD),
                ))
            })
            .collect();
        let header_row = Row::new(header_cells)
            .style(Style::default().bg(theme.neutral).fg(theme.neutral_content));

        let data_rows: Vec<Row> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, row_data)| {
                let bg = if i % 2 == 0 {
                    theme.base_100
                } else {
                    theme.base_200
                };
                let cells: Vec<Cell> = row_data.iter().map(|c| Cell::from(c.as_str())).collect();
                Row::new(cells).style(Style::default().fg(theme.base_content).bg(bg))
            })
            .collect();

        let border_set = if theme.border_rounded {
            ratatui::symbols::border::ROUNDED
        } else {
            ratatui::symbols::border::PLAIN
        };

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.base_300))
            .border_set(border_set);

        if let Some(ref info) = self.page_info {
            block = block.title_bottom(Span::styled(
                info.clone(),
                Style::default().fg(theme.base_content),
            ));
        }

        let highlight_style = Style::default().bg(theme.primary).fg(theme.primary_content);

        let table = Table::new(data_rows, &self.widths)
            .header(header_row)
            .block(block)
            .column_spacing(0)
            .row_highlight_style(highlight_style);

        let mut state = TableState::default();
        state.select(self.selected);
        StatefulWidget::render(table, area, buf, &mut state);
    }
}
