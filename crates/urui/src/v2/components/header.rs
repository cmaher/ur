use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::v2::navigation::{NavigationModel, TabId};

/// Render a tab bar header into the given area.
///
/// Each tab is displayed as its label text. The active tab uses
/// `primary` bg / `primary_content` fg; inactive tabs use
/// `base_200` bg / `base_content` fg. Reuses v1 theme colors and
/// visual style.
pub fn render_header(area: Rect, buf: &mut Buffer, ctx: &TuiContext, nav: &NavigationModel) {
    let theme = &ctx.theme;

    // Fill the entire header row with base_200 background first.
    let bg_style = Style::default().bg(theme.base_200).fg(theme.base_content);
    buf.set_style(area, bg_style);

    let mut spans: Vec<Span> = TabId::all()
        .iter()
        .map(|&tab| {
            let label = tab.label();
            let shortcut = label
                .chars()
                .next()
                .unwrap_or(' ')
                .to_lowercase()
                .next()
                .unwrap_or(' ');
            let rest = &label[label.chars().next().map(|c| c.len_utf8()).unwrap_or(0)..];
            let text = format!(" ({}){}  ", shortcut, rest.to_lowercase());
            let style = if tab == nav.active_tab {
                Style::default().bg(theme.primary).fg(theme.primary_content)
            } else {
                Style::default().bg(theme.base_200).fg(theme.base_content)
            };
            Span::styled(text, style)
        })
        .collect();

    // If a project filter is active, right-align it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Keymap;
    use crate::theme::Theme;
    use ur_config::TuiConfig;

    fn make_ctx() -> TuiContext {
        let tui_config = TuiConfig::default();
        let theme = Theme::resolve(&tui_config);
        let keymap = Keymap::default();
        TuiContext {
            theme,
            keymap,
            projects: vec![],
            project_configs: std::collections::HashMap::new(),
            tui_config: TuiConfig::default(),
            config_dir: std::path::PathBuf::from("/tmp/test-urui"),
            project_filter: None,
        }
    }

    #[test]
    fn render_header_does_not_panic() {
        let ctx = make_ctx();
        let nav = NavigationModel::initial();
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_header(area, &mut buf, &ctx, &nav);
    }

    #[test]
    fn render_header_shows_tab_labels() {
        let ctx = make_ctx();
        let nav = NavigationModel::initial();
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_header(area, &mut buf, &ctx, &nav);

        let rendered: String = (0..area.width)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(rendered.contains("ickets"), "should contain tab label text");
        assert!(rendered.contains("lows"), "should contain flows tab");
        assert!(rendered.contains("orkers"), "should contain workers tab");
    }

    #[test]
    fn active_tab_uses_primary_colors() {
        let ctx = make_ctx();
        let nav = NavigationModel::initial(); // active_tab = Tickets
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_header(area, &mut buf, &ctx, &nav);

        // The first tab cell (x=0) should have primary background
        let cell = buf.cell((0, 0)).unwrap();
        assert_eq!(
            cell.bg, ctx.theme.primary,
            "active tab should use primary bg"
        );
    }

    #[test]
    fn inactive_tab_uses_base_colors() {
        let ctx = make_ctx();
        let nav = NavigationModel::initial(); // active_tab = Tickets
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_header(area, &mut buf, &ctx, &nav);

        // Find a cell in the "flows" tab area (well past the first tab)
        // First tab "(t)ickets  " is ~13 chars, so x=15 should be in flows
        let cell = buf.cell((15, 0)).unwrap();
        assert_eq!(
            cell.bg, ctx.theme.base_200,
            "inactive tab should use base_200 bg"
        );
    }

    #[test]
    fn render_header_with_project_filter() {
        let mut ctx = make_ctx();
        ctx.project_filter = Some("ur".to_string());
        let nav = NavigationModel::initial();
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_header(area, &mut buf, &ctx, &nav);

        let rendered: String = (0..area.width)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(rendered.contains("[ur]"), "should show project filter");
    }

    #[test]
    fn switching_active_tab_changes_highlight() {
        let ctx = make_ctx();
        let mut nav = NavigationModel::initial();
        nav.active_tab = TabId::Flows;
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_header(area, &mut buf, &ctx, &nav);

        // First tab should now be inactive (base_200)
        let cell = buf.cell((0, 0)).unwrap();
        assert_eq!(
            cell.bg, ctx.theme.base_200,
            "tickets tab should be inactive"
        );
    }
}
