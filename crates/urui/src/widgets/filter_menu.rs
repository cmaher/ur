use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::FooterCommand;
use crate::widgets::overlay::render_overlay;

/// The filter categories available in the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterCategory {
    Status,
    Priority,
    Project,
    ShowChildren,
}

const CATEGORIES: &[FilterCategory] = &[
    FilterCategory::Status,
    FilterCategory::Priority,
    FilterCategory::Project,
    FilterCategory::ShowChildren,
];

const STATUS_OPTIONS: &[&str] = &["open", "in_progress", "closed"];
const PRIORITY_OPTIONS: &[i64] = &[0, 1, 2, 3, 4];

/// Result of handling a key event in the filter menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterMenuResult {
    /// The menu consumed the event; stay open.
    Consumed,
    /// The menu should close.
    Close,
}

/// Persisted filter selections applied to the ticket list.
#[derive(Debug, Clone)]
pub struct TicketFilters {
    /// Which statuses are enabled. When empty, all are shown.
    pub statuses: Vec<String>,
    /// Which priorities are enabled. When empty, all are shown.
    pub priorities: Vec<i64>,
    /// Which projects are enabled. When empty, all are shown.
    pub projects: Vec<String>,
    /// Whether to show tickets that have a parent_id (children).
    pub show_children: bool,
}

impl Default for TicketFilters {
    fn default() -> Self {
        Self {
            statuses: vec!["open".to_string(), "in_progress".to_string()],
            priorities: vec![],
            projects: vec![],
            show_children: false,
        }
    }
}

impl TicketFilters {
    /// Create filters from persisted config, falling back to defaults for unset fields.
    pub fn from_config(config: &ur_config::TicketFilterConfig) -> Self {
        let defaults = Self::default();
        Self {
            statuses: config.statuses.clone().unwrap_or(defaults.statuses),
            projects: config.projects.clone().unwrap_or(defaults.projects),
            priorities: defaults.priorities,
            show_children: defaults.show_children,
        }
    }

    /// Convert current filters to a config representation for persistence.
    pub fn to_config(&self) -> ur_config::TicketFilterConfig {
        ur_config::TicketFilterConfig {
            statuses: Some(self.statuses.clone()),
            projects: Some(self.projects.clone()),
        }
    }
}

/// State for the filter menu overlay.
pub struct FilterMenuState {
    /// Current cursor position (0-indexed in the top-level category list).
    cursor: usize,
    /// Currently expanded category, if any.
    expanded: Option<FilterCategory>,
    /// Cursor within the expanded submenu (0-indexed).
    sub_cursor: usize,
    /// Available project names from config.
    project_names: Vec<String>,
}

impl FilterMenuState {
    pub fn new(project_names: Vec<String>) -> Self {
        Self {
            cursor: 0,
            expanded: None,
            sub_cursor: 0,
            project_names,
        }
    }

    /// Handle a raw key event, mutating filters as needed.
    /// Returns whether the menu was consumed or should close.
    pub fn handle_key(&mut self, key: KeyEvent, filters: &mut TicketFilters) -> FilterMenuResult {
        match key.code {
            KeyCode::Esc => FilterMenuResult::Close,
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_down();
                FilterMenuResult::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_up();
                FilterMenuResult::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate(filters);
                FilterMenuResult::Consumed
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                self.quick_toggle(c, filters);
                FilterMenuResult::Consumed
            }
            _ => FilterMenuResult::Consumed,
        }
    }

    fn move_down(&mut self) {
        if let Some(cat) = self.expanded {
            let count = self.submenu_count(cat);
            if count > 0 && self.sub_cursor < count - 1 {
                self.sub_cursor += 1;
            }
        } else if self.cursor < CATEGORIES.len() - 1 {
            self.cursor += 1;
        }
    }

    fn move_up(&mut self) {
        if self.expanded.is_some() {
            if self.sub_cursor > 0 {
                self.sub_cursor -= 1;
            }
        } else if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn activate(&mut self, filters: &mut TicketFilters) {
        if let Some(cat) = self.expanded {
            // Toggle the item at sub_cursor
            self.toggle_item(cat, self.sub_cursor, filters);
        } else {
            let cat = CATEGORIES[self.cursor];
            if cat == FilterCategory::ShowChildren {
                // Direct toggle, no submenu
                filters.show_children = !filters.show_children;
            } else {
                // Expand the category
                self.expanded = Some(cat);
                self.sub_cursor = 0;
            }
        }
    }

    fn quick_toggle(&mut self, c: char, filters: &mut TicketFilters) {
        // At top level, number keys expand/activate categories (1-indexed)
        if self.expanded.is_none() {
            let digit = (c as u8 - b'0') as usize;
            if digit >= 1 && digit <= CATEGORIES.len() {
                self.cursor = digit - 1;
                self.activate(filters);
            }
            return;
        }
        let Some(cat) = self.expanded else {
            return;
        };
        let digit = (c as u8 - b'0') as usize;
        let index = match cat {
            // Priority uses 0-4 as quick keys directly
            FilterCategory::Priority => {
                if digit <= 4 {
                    digit
                } else {
                    return;
                }
            }
            // Others use 1-N mapping (1-indexed)
            _ => {
                if digit == 0 {
                    return;
                }
                digit - 1
            }
        };
        if index < self.submenu_count(cat) {
            self.toggle_item(cat, index, filters);
        }
    }

    fn toggle_item(&self, cat: FilterCategory, index: usize, filters: &mut TicketFilters) {
        match cat {
            FilterCategory::Status => {
                let value = STATUS_OPTIONS[index].to_string();
                toggle_vec(&mut filters.statuses, value);
            }
            FilterCategory::Priority => {
                let value = PRIORITY_OPTIONS[index];
                toggle_vec(&mut filters.priorities, value);
            }
            FilterCategory::Project => {
                if index < self.project_names.len() {
                    let value = self.project_names[index].clone();
                    toggle_vec(&mut filters.projects, value);
                }
            }
            FilterCategory::ShowChildren => {}
        }
    }

    fn submenu_count(&self, cat: FilterCategory) -> usize {
        match cat {
            FilterCategory::Status => STATUS_OPTIONS.len(),
            FilterCategory::Priority => PRIORITY_OPTIONS.len(),
            FilterCategory::Project => self.project_names.len(),
            FilterCategory::ShowChildren => 0,
        }
    }

    /// Render the filter menu as an overlay.
    pub fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext, filters: &TicketFilters) {
        let height = self.calc_height();
        let width = 40u16;
        let inner = render_overlay(area, buf, ctx, " Filters ", width, height);

        if let Some(cat) = self.expanded {
            self.render_submenu(inner, buf, ctx, cat, filters);
        } else {
            self.render_categories(inner, buf, ctx, filters);
        }
    }

    fn calc_height(&self) -> u16 {
        let content_lines = if let Some(cat) = self.expanded {
            self.submenu_count(cat) + 1 // +1 for back header
        } else {
            CATEGORIES.len()
        };
        // +2 for top and bottom border
        (content_lines as u16) + 2
    }

    fn render_categories(
        &self,
        area: Rect,
        buf: &mut Buffer,
        ctx: &TuiContext,
        filters: &TicketFilters,
    ) {
        let theme = &ctx.theme;
        for (i, cat) in CATEGORIES.iter().enumerate() {
            if i as u16 >= area.height {
                break;
            }
            let row_area = Rect::new(area.x, area.y + i as u16, area.width, 1);
            let (label, summary) = category_label_summary(*cat, filters);
            let is_selected = i == self.cursor;

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
        &self,
        area: Rect,
        buf: &mut Buffer,
        ctx: &TuiContext,
        cat: FilterCategory,
        filters: &TicketFilters,
    ) {
        let theme = &ctx.theme;

        // Render header line (category name)
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

        // Render items
        let count = self.submenu_count(cat);
        for i in 0..count {
            let row_idx = (i + 1) as u16; // +1 for header
            if row_idx >= area.height {
                break;
            }
            let row_area = Rect::new(area.x, area.y + row_idx, area.width, 1);

            let (label, checked, quick_key) = self.submenu_item_info(cat, i, filters);

            let is_selected = i == self.sub_cursor;
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

    /// Returns (label, is_checked, quick_key_display) for a submenu item.
    fn submenu_item_info(
        &self,
        cat: FilterCategory,
        index: usize,
        filters: &TicketFilters,
    ) -> (String, bool, String) {
        match cat {
            FilterCategory::Status => {
                let val = STATUS_OPTIONS[index];
                let checked = filters.statuses.contains(&val.to_string());
                // 1-indexed quick keys
                (val.to_string(), checked, format!("{}", index + 1))
            }
            FilterCategory::Priority => {
                let val = PRIORITY_OPTIONS[index];
                let checked = filters.priorities.contains(&val);
                // 0-4 direct quick keys
                (format!("P{val}"), checked, format!("{val}"))
            }
            FilterCategory::Project => {
                let val = &self.project_names[index];
                let checked = filters.projects.contains(val);
                // 1-indexed quick keys
                (val.clone(), checked, format!("{}", index + 1))
            }
            FilterCategory::ShowChildren => {
                // Should not be called for ShowChildren
                (String::new(), false, String::new())
            }
        }
    }

    /// Footer commands to show when the filter menu is open.
    pub fn footer_commands(&self) -> Vec<FooterCommand> {
        if self.expanded.is_some() {
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
        } else {
            vec![
                FooterCommand {
                    key_label: "j/k".to_string(),
                    description: "Navigate".to_string(),
                    common: false,
                },
                FooterCommand {
                    key_label: "Enter".to_string(),
                    description: "Expand".to_string(),
                    common: false,
                },
                FooterCommand {
                    key_label: "Esc".to_string(),
                    description: "Close".to_string(),
                    common: false,
                },
            ]
        }
    }

    /// Close the expanded submenu, returning to top-level. Returns true if
    /// a submenu was actually closed, false if already at top level.
    pub fn collapse(&mut self) -> bool {
        if self.expanded.is_some() {
            self.expanded = None;
            true
        } else {
            false
        }
    }
}

/// Return the display label and current summary for a filter category.
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
fn toggle_vec<T: PartialEq>(vec: &mut Vec<T>, value: T) {
    if let Some(pos) = vec.iter().position(|v| *v == value) {
        vec.remove(pos);
    } else {
        vec.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filters() {
        let f = TicketFilters::default();
        assert_eq!(
            f.statuses,
            vec!["open".to_string(), "in_progress".to_string()]
        );
        assert!(f.priorities.is_empty());
        assert!(f.projects.is_empty());
        assert!(!f.show_children);
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
    fn filter_menu_navigate() {
        let mut state = FilterMenuState::new(vec!["proj1".into()]);
        let mut filters = TicketFilters::default();

        // Move down
        let r = state.handle_key(
            KeyEvent::new(KeyCode::Char('j'), crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert_eq!(r, FilterMenuResult::Consumed);
        assert_eq!(state.cursor, 1);

        // Move up
        state.handle_key(
            KeyEvent::new(KeyCode::Char('k'), crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn filter_menu_expand_and_toggle() {
        let mut state = FilterMenuState::new(vec!["proj1".into()]);
        let mut filters = TicketFilters {
            statuses: vec!["open".to_string()],
            ..TicketFilters::default()
        };

        // Cursor is on Status (index 0). Press Enter to expand.
        state.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert_eq!(state.expanded, Some(FilterCategory::Status));

        // Toggle "open" (already selected, so removes it)
        state.handle_key(
            KeyEvent::new(KeyCode::Char(' '), crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert!(filters.statuses.is_empty());

        // Toggle "open" back
        state.handle_key(
            KeyEvent::new(KeyCode::Char(' '), crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert_eq!(filters.statuses, vec!["open".to_string()]);
    }

    #[test]
    fn filter_menu_close_on_esc() {
        let mut state = FilterMenuState::new(vec![]);
        let mut filters = TicketFilters::default();

        let r = state.handle_key(
            KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert_eq!(r, FilterMenuResult::Close);
    }

    #[test]
    fn filter_menu_collapse_submenu() {
        let mut state = FilterMenuState::new(vec![]);
        let mut filters = TicketFilters::default();

        // Expand Status
        state.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert!(state.expanded.is_some());

        // Collapse
        assert!(state.collapse());
        assert!(state.expanded.is_none());

        // Second collapse returns false
        assert!(!state.collapse());
    }

    #[test]
    fn quick_toggle_priority() {
        let mut state = FilterMenuState::new(vec![]);
        let mut filters = TicketFilters::default();

        // Move to Priority (index 1) and expand
        state.cursor = 1;
        state.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert_eq!(state.expanded, Some(FilterCategory::Priority));

        // Quick toggle P0 with '0' key
        state.handle_key(
            KeyEvent::new(KeyCode::Char('0'), crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert_eq!(filters.priorities, vec![0]);

        // Quick toggle P2 with '2' key
        state.handle_key(
            KeyEvent::new(KeyCode::Char('2'), crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert_eq!(filters.priorities, vec![0, 2]);
    }

    #[test]
    fn quick_toggle_status_uses_1_indexed() {
        let mut state = FilterMenuState::new(vec![]);
        let mut filters = TicketFilters {
            statuses: vec![],
            ..TicketFilters::default()
        };

        // Expand Status
        state.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );

        // Quick toggle '2' should toggle "in_progress" (index 1)
        state.handle_key(
            KeyEvent::new(KeyCode::Char('2'), crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert!(filters.statuses.contains(&"in_progress".to_string()));
    }

    #[test]
    fn show_children_toggle() {
        let mut state = FilterMenuState::new(vec![]);
        let mut filters = TicketFilters::default();
        assert!(!filters.show_children);

        // Move to ShowChildren (index 3)
        state.cursor = 3;
        state.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut filters,
        );
        assert!(filters.show_children);
        // ShowChildren is a direct toggle, no submenu
        assert!(state.expanded.is_none());
    }

    #[test]
    fn footer_commands_differ_by_level() {
        let state = FilterMenuState::new(vec![]);
        let cmds = state.footer_commands();
        assert!(cmds.iter().any(|c| c.description == "Expand"));

        let mut state2 = FilterMenuState::new(vec![]);
        state2.expanded = Some(FilterCategory::Status);
        let cmds2 = state2.footer_commands();
        assert!(cmds2.iter().any(|c| c.description == "Toggle"));
    }
}
