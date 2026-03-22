use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::page::FooterCommand;
use crate::theme;
use crate::widgets::overlay::render_overlay;

/// Result of handling a key event in the settings overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsResult {
    /// The overlay consumed the event; stay open.
    Consumed,
    /// The user selected a theme; apply it.
    ThemeSelected(String),
    /// The overlay should close.
    Close,
}

/// Which level the settings overlay is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsLevel {
    /// Top-level settings menu.
    TopLevel,
    /// Theme picker with three columns.
    ThemePicker,
}

/// daisyUI theme classification: built-in themes known to be light.
const LIGHT_THEMES: &[&str] = &[
    "autumn",
    "bumblebee",
    "caramellatte",
    "cmyk",
    "corporate",
    "cupcake",
    "cyberpunk",
    "emerald",
    "fantasy",
    "garden",
    "lemonade",
    "light",
    "lofi",
    "nord",
    "pastel",
    "retro",
    "silk",
    "valentine",
    "winter",
    "wireframe",
    "acid",
];

/// Classify a built-in theme as light or dark.
fn is_light_theme(name: &str) -> bool {
    LIGHT_THEMES.contains(&name)
}

/// Column index in the theme picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeColumn {
    Light,
    Dark,
    Custom,
}

const COLUMNS: &[ThemeColumn] = &[ThemeColumn::Light, ThemeColumn::Dark, ThemeColumn::Custom];

/// State for the settings overlay.
pub struct SettingsOverlayState {
    /// Current display level.
    level: SettingsLevel,
    /// Top-level cursor position.
    top_cursor: usize,
    /// Active column in theme picker.
    active_column: usize,
    /// Per-column cursor positions in theme picker.
    column_cursors: [usize; 3],
    /// Light theme names (sorted).
    light_themes: Vec<String>,
    /// Dark theme names (sorted).
    dark_themes: Vec<String>,
    /// Custom theme names (sorted).
    custom_themes: Vec<String>,
    /// Config directory for persisting theme selection.
    config_dir: PathBuf,
}

impl SettingsOverlayState {
    /// Create a new settings overlay state.
    ///
    /// `custom_theme_names` are the keys from `[tui.themes.*]` in ur.toml.
    /// `config_dir` is used for persisting the selected theme.
    pub fn new(custom_theme_names: Vec<String>, config_dir: PathBuf) -> Self {
        let mut light_themes = Vec::new();
        let mut dark_themes = Vec::new();

        for &name in theme::BUILTIN_THEME_NAMES {
            if is_light_theme(name) {
                light_themes.push(name.to_string());
            } else {
                dark_themes.push(name.to_string());
            }
        }

        let mut custom_themes: Vec<String> = custom_theme_names;
        custom_themes.sort();

        Self {
            level: SettingsLevel::TopLevel,
            top_cursor: 0,
            active_column: 0,
            column_cursors: [0; 3],
            light_themes,
            dark_themes,
            custom_themes,
            config_dir,
        }
    }

    /// Handle a raw key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> SettingsResult {
        match self.level {
            SettingsLevel::TopLevel => self.handle_top_level_key(key),
            SettingsLevel::ThemePicker => self.handle_theme_picker_key(key),
        }
    }

    fn handle_top_level_key(&mut self, key: KeyEvent) -> SettingsResult {
        match key.code {
            KeyCode::Esc => SettingsResult::Close,
            KeyCode::Char('1') | KeyCode::Char(' ') | KeyCode::Enter => {
                // Only option is Theme (index 0)
                self.level = SettingsLevel::ThemePicker;
                SettingsResult::Consumed
            }
            _ => SettingsResult::Consumed,
        }
    }

    fn handle_theme_picker_key(&mut self, key: KeyEvent) -> SettingsResult {
        match key.code {
            KeyCode::Esc => {
                self.level = SettingsLevel::TopLevel;
                SettingsResult::Consumed
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.move_column_left();
                SettingsResult::Consumed
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.move_column_right();
                SettingsResult::Consumed
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_cursor_down();
                SettingsResult::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_cursor_up();
                SettingsResult::Consumed
            }
            KeyCode::Char(' ') => {
                if let Some(name) = self.selected_theme_name() {
                    self.persist_theme(&name);
                    SettingsResult::ThemeSelected(name)
                } else {
                    SettingsResult::Consumed
                }
            }
            _ => SettingsResult::Consumed,
        }
    }

    fn move_column_left(&mut self) {
        if self.active_column > 0 {
            self.active_column -= 1;
            self.snap_cursor();
        }
    }

    fn move_column_right(&mut self) {
        if self.active_column < COLUMNS.len() - 1 {
            self.active_column += 1;
            self.snap_cursor();
        }
    }

    fn move_cursor_down(&mut self) {
        let count = self.active_column_len();
        if count > 0 && self.column_cursors[self.active_column] < count - 1 {
            self.column_cursors[self.active_column] += 1;
        }
    }

    fn move_cursor_up(&mut self) {
        if self.column_cursors[self.active_column] > 0 {
            self.column_cursors[self.active_column] -= 1;
        }
    }

    /// Snap the cursor to the last item if it exceeds the column length.
    fn snap_cursor(&mut self) {
        let count = self.active_column_len();
        if count == 0 {
            self.column_cursors[self.active_column] = 0;
        } else if self.column_cursors[self.active_column] >= count {
            self.column_cursors[self.active_column] = count - 1;
        }
    }

    fn active_column_len(&self) -> usize {
        self.column_items(COLUMNS[self.active_column]).len()
    }

    fn column_items(&self, col: ThemeColumn) -> &[String] {
        match col {
            ThemeColumn::Light => &self.light_themes,
            ThemeColumn::Dark => &self.dark_themes,
            ThemeColumn::Custom => &self.custom_themes,
        }
    }

    fn selected_theme_name(&self) -> Option<String> {
        let items = self.column_items(COLUMNS[self.active_column]);
        let cursor = self.column_cursors[self.active_column];
        items.get(cursor).cloned()
    }

    fn persist_theme(&self, theme_name: &str) {
        if let Err(e) = ur_config::save_theme_name(&self.config_dir, theme_name) {
            eprintln!("warning: failed to persist theme to ur.toml: {e}");
        }
    }

    /// Render the settings overlay.
    pub fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        match self.level {
            SettingsLevel::TopLevel => self.render_top_level(area, buf, ctx),
            SettingsLevel::ThemePicker => self.render_theme_picker(area, buf, ctx),
        }
    }

    fn render_top_level(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        let inner = render_overlay(area, buf, ctx, " Settings ", 40, 3);
        let theme = &ctx.theme;

        if inner.height > 0 {
            let row_area = Rect::new(inner.x, inner.y, inner.width, 1);
            let is_selected = self.top_cursor == 0;
            let style = if is_selected {
                Style::default().fg(theme.primary_content).bg(theme.primary)
            } else {
                Style::default().fg(theme.base_content).bg(theme.base_200)
            };
            buf.set_style(row_area, style);
            let line = Line::from(Span::raw(" 1 Theme")).style(style);
            line.render(row_area, buf);
        }
    }

    fn render_theme_picker(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        let max_rows = self
            .light_themes
            .len()
            .max(self.dark_themes.len())
            .max(self.custom_themes.len());
        // +3 for border top, header row, border bottom
        let height = (max_rows as u16 + 1).min(area.height.saturating_sub(4)) + 2;
        let width = 60u16.min(area.width.saturating_sub(4));
        let inner = render_overlay(area, buf, ctx, " Theme ", width, height);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let col_width = inner.width / 3;
        self.render_theme_column(inner, buf, ctx, 0, col_width);
        self.render_theme_column(inner, buf, ctx, 1, col_width);
        // Last column gets remaining width
        let last_width = inner.width - col_width * 2;
        self.render_theme_column(inner, buf, ctx, 2, last_width);
    }

    fn render_theme_column(
        &self,
        area: Rect,
        buf: &mut Buffer,
        ctx: &TuiContext,
        col_index: usize,
        col_width: u16,
    ) {
        let col = COLUMNS[col_index];
        let header = match col {
            ThemeColumn::Light => "Light",
            ThemeColumn::Dark => "Dark",
            ThemeColumn::Custom => "Custom",
        };
        let theme = &ctx.theme;
        let x = area.x + (col_index as u16) * (area.width / 3);
        let is_active_col = col_index == self.active_column;

        // Render header
        if area.height > 0 {
            let header_area = Rect::new(x, area.y, col_width, 1);
            let header_style = if is_active_col {
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.base_200)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.base_content)
                    .bg(theme.base_200)
                    .add_modifier(Modifier::DIM)
            };
            let line = Line::from(Span::raw(format!(" {header}"))).style(header_style);
            line.render(header_area, buf);
        }

        // Render items
        let items = self.column_items(col);
        let cursor = self.column_cursors[col_index];
        for (i, name) in items.iter().enumerate() {
            let row_y = area.y + 1 + i as u16;
            if row_y >= area.y + area.height {
                break;
            }
            let row_area = Rect::new(x, row_y, col_width, 1);
            let is_selected = is_active_col && i == cursor;

            let style = if is_selected {
                Style::default().fg(theme.primary_content).bg(theme.primary)
            } else {
                Style::default().fg(theme.base_content).bg(theme.base_200)
            };

            buf.set_style(row_area, style);
            let truncated = truncate_name(name, col_width.saturating_sub(2) as usize);
            let line = Line::from(Span::raw(format!(" {truncated}"))).style(style);
            line.render(row_area, buf);
        }
    }

    /// Footer commands to show when the settings overlay is open.
    pub fn footer_commands(&self) -> Vec<FooterCommand> {
        match self.level {
            SettingsLevel::TopLevel => vec![
                FooterCommand {
                    key_label: "1".to_string(),
                    description: "Theme".to_string(),
                    common: false,
                },
                FooterCommand {
                    key_label: "Esc".to_string(),
                    description: "Close".to_string(),
                    common: false,
                },
            ],
            SettingsLevel::ThemePicker => vec![
                FooterCommand {
                    key_label: "h/l".to_string(),
                    description: "Column".to_string(),
                    common: false,
                },
                FooterCommand {
                    key_label: "j/k".to_string(),
                    description: "Navigate".to_string(),
                    common: false,
                },
                FooterCommand {
                    key_label: "Space".to_string(),
                    description: "Apply".to_string(),
                    common: false,
                },
                FooterCommand {
                    key_label: "Esc".to_string(),
                    description: "Back".to_string(),
                    common: false,
                },
            ],
        }
    }
}

/// Truncate a name to fit within a given width.
fn truncate_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        name.to_string()
    } else if max > 2 {
        format!("{}..", &name[..max - 2])
    } else {
        name[..max].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> SettingsOverlayState {
        SettingsOverlayState::new(vec!["mycustom".to_string()], PathBuf::from("/tmp/test"))
    }

    #[test]
    fn initial_state_is_top_level() {
        let state = make_state();
        assert_eq!(state.level, SettingsLevel::TopLevel);
    }

    #[test]
    fn top_level_esc_closes() {
        let mut state = make_state();
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, SettingsResult::Close);
    }

    #[test]
    fn top_level_1_enters_theme_picker() {
        let mut state = make_state();
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char('1'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, SettingsResult::Consumed);
        assert_eq!(state.level, SettingsLevel::ThemePicker);
    }

    #[test]
    fn top_level_space_enters_theme_picker() {
        let mut state = make_state();
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, SettingsResult::Consumed);
        assert_eq!(state.level, SettingsLevel::ThemePicker);
    }

    #[test]
    fn theme_picker_esc_returns_to_top() {
        let mut state = make_state();
        state.level = SettingsLevel::ThemePicker;
        let r = state.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, SettingsResult::Consumed);
        assert_eq!(state.level, SettingsLevel::TopLevel);
    }

    #[test]
    fn theme_picker_navigate_columns() {
        let mut state = make_state();
        state.level = SettingsLevel::ThemePicker;
        assert_eq!(state.active_column, 0);

        // Move right
        state.handle_key(KeyEvent::new(
            KeyCode::Char('l'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.active_column, 1);

        // Move right again
        state.handle_key(KeyEvent::new(
            KeyCode::Right,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.active_column, 2);

        // Can't go beyond 2
        state.handle_key(KeyEvent::new(
            KeyCode::Char('l'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.active_column, 2);

        // Move left
        state.handle_key(KeyEvent::new(
            KeyCode::Char('h'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.active_column, 1);
    }

    #[test]
    fn theme_picker_navigate_within_column() {
        let mut state = make_state();
        state.level = SettingsLevel::ThemePicker;
        assert!(!state.light_themes.is_empty());

        // Move down
        state.handle_key(KeyEvent::new(
            KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.column_cursors[0], 1);

        // Move up
        state.handle_key(KeyEvent::new(
            KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.column_cursors[0], 0);

        // Can't go above 0
        state.handle_key(KeyEvent::new(
            KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(state.column_cursors[0], 0);
    }

    #[test]
    fn column_cursor_snaps_when_switching() {
        let mut state = make_state();
        state.level = SettingsLevel::ThemePicker;

        // Move deep into light column
        for _ in 0..50 {
            state.handle_key(KeyEvent::new(
                KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        let light_cursor = state.column_cursors[0];
        assert!(light_cursor > 0);

        // Switch to custom column (which has only 1 item)
        state.active_column = 2;
        state.column_cursors[2] = 999; // artificially high
        state.snap_cursor();
        assert_eq!(state.column_cursors[2], 0); // snapped to last (index 0)
    }

    #[test]
    fn space_selects_theme() {
        let mut state = make_state();
        state.level = SettingsLevel::ThemePicker;
        let expected = state.light_themes[0].clone();

        let r = state.handle_key(KeyEvent::new(
            KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(r, SettingsResult::ThemeSelected(expected));
    }

    #[test]
    fn light_theme_classification() {
        assert!(is_light_theme("light"));
        assert!(is_light_theme("cupcake"));
        assert!(!is_light_theme("dark"));
        assert!(!is_light_theme("dracula"));
    }

    #[test]
    fn truncate_name_short() {
        assert_eq!(truncate_name("hello", 10), "hello");
    }

    #[test]
    fn truncate_name_long() {
        assert_eq!(truncate_name("caramellatte", 8), "carame..");
    }

    #[test]
    fn footer_commands_top_level() {
        let state = make_state();
        let cmds = state.footer_commands();
        assert!(cmds.iter().any(|c| c.description == "Theme"));
    }

    #[test]
    fn footer_commands_theme_picker() {
        let mut state = make_state();
        state.level = SettingsLevel::ThemePicker;
        let cmds = state.footer_commands();
        assert!(cmds.iter().any(|c| c.description == "Apply"));
        assert!(cmds.iter().any(|c| c.description == "Column"));
    }
}
