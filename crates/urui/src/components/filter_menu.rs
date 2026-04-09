use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;

use super::overlay::render_overlay;
use crate::input::FooterCommand;
use crate::model::{ActiveOverlay, FilterCategory, Model, TicketFilters};
use crate::msg::{Msg, OverlayMsg};

/// All filter categories in display order.
pub const CATEGORIES: &[FilterCategory] = &[
    FilterCategory::Status,
    FilterCategory::Priority,
    FilterCategory::Project,
    FilterCategory::ShowChildren,
];

pub const STATUS_OPTIONS: &[&str] = &["open", "in_progress", "closed"];
pub const PRIORITY_OPTIONS: &[i64] = &[0, 1, 2, 3, 4];

/// Handle a key event for the filter menu overlay.
///
/// All keys are captured (modal). j/k navigate, Enter/Space activate/toggle,
/// Esc closes.
pub fn handle_key(key: KeyEvent) -> Msg {
    match key.code {
        KeyCode::Esc => Msg::Overlay(OverlayMsg::FilterMenuClosed),
        KeyCode::Char('j') | KeyCode::Down => {
            Msg::Overlay(OverlayMsg::FilterMenuNavigate { delta: 1 })
        }
        KeyCode::Char('k') | KeyCode::Up => {
            Msg::Overlay(OverlayMsg::FilterMenuNavigate { delta: -1 })
        }
        KeyCode::Enter | KeyCode::Char(' ') => Msg::Overlay(OverlayMsg::FilterMenuActivate),
        KeyCode::Char(c) if c.is_ascii_digit() => {
            Msg::Overlay(OverlayMsg::FilterMenuQuickToggle { digit: c })
        }
        _ => Msg::Overlay(OverlayMsg::Consumed),
    }
}

/// Footer commands for the filter menu overlay.
pub fn footer_commands() -> Vec<FooterCommand> {
    vec![
        FooterCommand {
            key_label: "j/k".to_string(),
            description: "Navigate".to_string(),
            common: false,
        },
        FooterCommand {
            key_label: "Space".to_string(),
            description: "Toggle".to_string(),
            common: false,
        },
        FooterCommand {
            key_label: "Esc".to_string(),
            description: "Close".to_string(),
            common: false,
        },
    ]
}

/// Render the filter menu overlay from the model state.
pub fn render_filter_menu(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let (cursor, expanded, sub_cursor) = match &model.active_overlay {
        Some(ActiveOverlay::FilterMenu {
            cursor,
            expanded,
            sub_cursor,
        }) => (*cursor, *expanded, *sub_cursor),
        _ => return,
    };

    let height = calc_height(expanded, &ctx.projects);
    let width = 40u16;
    let inner = render_overlay(area, buf, ctx, " Filters ", width, height);

    if let Some(cat) = expanded {
        render_submenu(inner, buf, ctx, cat, sub_cursor, &model.ticket_filters);
    } else {
        render_categories(inner, buf, ctx, cursor, &model.ticket_filters);
    }
}

fn calc_height(expanded: Option<FilterCategory>, projects: &[String]) -> u16 {
    let content_lines = if let Some(cat) = expanded {
        submenu_count(cat, projects) + 1 // +1 for back header
    } else {
        CATEGORIES.len()
    };
    (content_lines as u16) + 2 // +2 for borders
}

/// Returns the number of items in a submenu for a given category.
pub fn submenu_count(cat: FilterCategory, projects: &[String]) -> usize {
    match cat {
        FilterCategory::Status => STATUS_OPTIONS.len(),
        FilterCategory::Priority => PRIORITY_OPTIONS.len(),
        FilterCategory::Project => projects.len(),
        FilterCategory::ShowChildren => 0,
    }
}

fn render_categories(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    cursor: usize,
    filters: &TicketFilters,
) {
    let theme = &ctx.theme;
    for (i, cat) in CATEGORIES.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let row_area = Rect::new(area.x, area.y + i as u16, area.width, 1);
        let (label, summary) = category_label_summary(*cat, filters);
        let is_selected = i == cursor;

        let style = if is_selected {
            Style::default().fg(theme.primary_content).bg(theme.primary)
        } else {
            Style::default().fg(theme.base_content).bg(theme.base_200)
        };

        buf.set_style(row_area, style);
        let num = i + 1;
        let text = format!(" {num} {label}: {summary}");
        let line = Line::from(Span::raw(text)).style(style);
        line.render(row_area, buf);
    }
}

fn render_submenu(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    cat: FilterCategory,
    sub_cursor: usize,
    filters: &TicketFilters,
) {
    let theme = &ctx.theme;

    // Render header line
    if area.height > 0 {
        let header_area = Rect::new(area.x, area.y, area.width, 1);
        let header_style = Style::default()
            .fg(theme.accent)
            .bg(theme.base_200)
            .add_modifier(Modifier::BOLD);
        let cat_name = match cat {
            FilterCategory::Status => "Status",
            FilterCategory::Priority => "Priority",
            FilterCategory::Project => "Project",
            FilterCategory::ShowChildren => "Show Children",
        };
        let header_line = Line::from(Span::raw(format!(" {cat_name}"))).style(header_style);
        header_line.render(header_area, buf);
    }

    let count = submenu_count(cat, &ctx.projects);
    for i in 0..count {
        let row_idx = (i + 1) as u16;
        if row_idx >= area.height {
            break;
        }
        let row_area = Rect::new(area.x, area.y + row_idx, area.width, 1);
        let (label, checked, quick_key) = submenu_item_info(cat, i, filters, &ctx.projects);

        let is_selected = i == sub_cursor;
        let style = if is_selected {
            Style::default().fg(theme.primary_content).bg(theme.primary)
        } else {
            Style::default().fg(theme.base_content).bg(theme.base_200)
        };

        buf.set_style(row_area, style);
        let check = if checked { "[x]" } else { "[ ]" };
        let text = format!(" {quick_key} {check} {label}");
        let line = Line::from(Span::raw(text)).style(style);
        line.render(row_area, buf);
    }
}

fn submenu_item_info(
    cat: FilterCategory,
    index: usize,
    filters: &TicketFilters,
    projects: &[String],
) -> (String, bool, String) {
    match cat {
        FilterCategory::Status => {
            let val = STATUS_OPTIONS[index];
            let checked = filters.statuses.contains(&val.to_string());
            (val.to_string(), checked, format!("{}", index + 1))
        }
        FilterCategory::Priority => {
            let val = PRIORITY_OPTIONS[index];
            let checked = filters.priorities.contains(&val);
            (format!("P{val}"), checked, format!("{val}"))
        }
        FilterCategory::Project => {
            let val = &projects[index];
            let checked = filters.projects.contains(val);
            (val.clone(), checked, format!("{}", index + 1))
        }
        FilterCategory::ShowChildren => (String::new(), false, String::new()),
    }
}

fn category_label_summary(cat: FilterCategory, filters: &TicketFilters) -> (&'static str, String) {
    match cat {
        FilterCategory::Status => {
            let summary = if filters.statuses.is_empty() {
                "all".to_string()
            } else {
                filters.statuses.join(", ")
            };
            ("Status", summary)
        }
        FilterCategory::Priority => {
            let summary = if filters.priorities.is_empty() {
                "all".to_string()
            } else {
                filters
                    .priorities
                    .iter()
                    .map(|p| format!("P{p}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            ("Priority", summary)
        }
        FilterCategory::Project => {
            let summary = if filters.projects.is_empty() {
                "all".to_string()
            } else {
                filters.projects.join(", ")
            };
            ("Project", summary)
        }
        FilterCategory::ShowChildren => {
            let summary = if filters.show_children { "on" } else { "off" };
            ("Show Children", summary.to_string())
        }
    }
}

/// Toggle a value in a vec: remove if present, add if absent.
pub fn toggle_vec<T: PartialEq>(vec: &mut Vec<T>, value: T) {
    if let Some(pos) = vec.iter().position(|v| *v == value) {
        vec.remove(pos);
    } else {
        vec.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn handle_key_esc() {
        assert!(matches!(
            handle_key(key(KeyCode::Esc)),
            Msg::Overlay(OverlayMsg::FilterMenuClosed)
        ));
    }

    #[test]
    fn handle_key_j() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('j'))),
            Msg::Overlay(OverlayMsg::FilterMenuNavigate { delta: 1 })
        ));
    }

    #[test]
    fn handle_key_space_activate() {
        assert!(matches!(
            handle_key(key(KeyCode::Char(' '))),
            Msg::Overlay(OverlayMsg::FilterMenuActivate)
        ));
    }

    #[test]
    fn handle_key_digit() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('2'))),
            Msg::Overlay(OverlayMsg::FilterMenuQuickToggle { digit: '2' })
        ));
    }

    #[test]
    fn handle_key_unknown() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('x'))),
            Msg::Overlay(OverlayMsg::Consumed)
        ));
    }

    #[test]
    fn toggle_vec_adds_and_removes() {
        let mut v = vec![1, 2, 3];
        toggle_vec(&mut v, 2);
        assert_eq!(v, vec![1, 3]);
        toggle_vec(&mut v, 4);
        assert_eq!(v, vec![1, 3, 4]);
    }

    #[test]
    fn default_filters() {
        let f = TicketFilters::default();
        assert_eq!(
            f.statuses,
            vec!["open".to_string(), "in_progress".to_string()]
        );
        assert!(f.priorities.is_empty());
        assert!(!f.show_children);
    }

    #[test]
    fn category_label_summary_status() {
        let filters = TicketFilters::default();
        let (label, summary) = category_label_summary(FilterCategory::Status, &filters);
        assert_eq!(label, "Status");
        assert!(summary.contains("open"));
    }

    #[test]
    fn category_label_summary_show_children() {
        let filters = TicketFilters::default();
        let (label, summary) = category_label_summary(FilterCategory::ShowChildren, &filters);
        assert_eq!(label, "Show Children");
        assert_eq!(summary, "off");
    }

    #[test]
    fn footer_commands_present() {
        let cmds = footer_commands();
        assert!(cmds.iter().any(|c| c.description == "Toggle"));
        assert!(cmds.iter().any(|c| c.description == "Close"));
    }
}
