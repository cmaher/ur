use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::theme;

use super::overlay::render_overlay;
use crate::input::FooterCommand;
use crate::model::{ActiveOverlay, Model, SettingsLevel};
use crate::msg::{Msg, OverlayMsg, SettingsDirection};

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

/// Handle a key event for the settings overlay.
///
/// All keys are captured (modal). At the top level, Esc closes and
/// 1/Enter/Space enters the theme picker. In the theme picker, h/l switch
/// columns, j/k navigate, Space applies, Esc goes back.
pub fn handle_key(key: KeyEvent) -> Msg {
    match key.code {
        KeyCode::Esc => Msg::Overlay(OverlayMsg::SettingsEsc),
        KeyCode::Char('1') | KeyCode::Enter | KeyCode::Char(' ') => {
            Msg::Overlay(OverlayMsg::SettingsActivate)
        }
        KeyCode::Char('j') | KeyCode::Down => Msg::Overlay(OverlayMsg::SettingsNavigate {
            direction: SettingsDirection::Down,
        }),
        KeyCode::Char('k') | KeyCode::Up => Msg::Overlay(OverlayMsg::SettingsNavigate {
            direction: SettingsDirection::Up,
        }),
        KeyCode::Char('h') | KeyCode::Left => Msg::Overlay(OverlayMsg::SettingsNavigate {
            direction: SettingsDirection::Left,
        }),
        KeyCode::Char('l') | KeyCode::Right => Msg::Overlay(OverlayMsg::SettingsNavigate {
            direction: SettingsDirection::Right,
        }),
        _ => Msg::Overlay(OverlayMsg::Consumed),
    }
}

/// Footer commands for the settings overlay.
pub fn footer_commands() -> Vec<FooterCommand> {
    vec![
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
            description: "Back/Close".to_string(),
            common: false,
        },
    ]
}

/// Render the settings overlay from the model state.
pub fn render_settings_overlay(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    if let Some(ActiveOverlay::Settings { level, .. }) = &model.active_overlay {
        match level {
            SettingsLevel::TopLevel => render_top_level(area, buf, ctx, model),
            SettingsLevel::ThemePicker => render_theme_picker(area, buf, ctx, model),
        }
    }
}

fn render_top_level(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let top_cursor = match &model.active_overlay {
        Some(ActiveOverlay::Settings { top_cursor, .. }) => *top_cursor,
        _ => return,
    };

    let inner = render_overlay(area, buf, ctx, " Settings ", 40, 3);
    let cur_theme = &ctx.theme;

    if inner.height > 0 {
        let row_area = Rect::new(inner.x, inner.y, inner.width, 1);
        let is_selected = top_cursor == 0;
        let style = if is_selected {
            Style::default()
                .fg(cur_theme.primary_content)
                .bg(cur_theme.primary)
        } else {
            Style::default()
                .fg(cur_theme.base_content)
                .bg(cur_theme.base_200)
        };
        buf.set_style(row_area, style);
        let line = Line::from(Span::raw(" 1 Theme")).style(style);
        line.render(row_area, buf);
    }
}

fn render_theme_picker(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let (active_column, column_cursors, light_themes, dark_themes, custom_themes) =
        match &model.active_overlay {
            Some(ActiveOverlay::Settings {
                active_column,
                column_cursors,
                light_themes,
                dark_themes,
                custom_themes,
                ..
            }) => (
                *active_column,
                *column_cursors,
                light_themes,
                dark_themes,
                custom_themes,
            ),
            _ => return,
        };

    let max_rows = light_themes
        .len()
        .max(dark_themes.len())
        .max(custom_themes.len());
    let height = (max_rows as u16 + 1).min(area.height.saturating_sub(4)) + 2;
    let width = 60u16.min(area.width.saturating_sub(4));
    let inner = render_overlay(area, buf, ctx, " Theme ", width, height);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let col_width = inner.width / 3;
    render_theme_column(
        inner,
        buf,
        ctx,
        0,
        col_width,
        active_column,
        column_cursors,
        light_themes,
    );
    render_theme_column(
        inner,
        buf,
        ctx,
        1,
        col_width,
        active_column,
        column_cursors,
        dark_themes,
    );
    let last_width = inner.width - col_width * 2;
    render_theme_column(
        inner,
        buf,
        ctx,
        2,
        last_width,
        active_column,
        column_cursors,
        custom_themes,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_theme_column(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    col_index: usize,
    col_width: u16,
    active_column: usize,
    column_cursors: [usize; 3],
    items: &[String],
) {
    let header = match col_index {
        0 => "Light",
        1 => "Dark",
        _ => "Custom",
    };
    let cur_theme = &ctx.theme;
    let x = area.x + (col_index as u16) * (area.width / 3);
    let is_active_col = col_index == active_column;

    // Render header
    if area.height > 0 {
        let header_area = Rect::new(x, area.y, col_width, 1);
        let header_style = if is_active_col {
            Style::default()
                .fg(cur_theme.accent)
                .bg(cur_theme.base_200)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(cur_theme.base_content)
                .bg(cur_theme.base_200)
                .add_modifier(Modifier::DIM)
        };
        let line = Line::from(Span::raw(format!(" {header}"))).style(header_style);
        line.render(header_area, buf);
    }

    // Render items
    let cursor = column_cursors[col_index];
    for (i, name) in items.iter().enumerate() {
        let row_y = area.y + 1 + i as u16;
        if row_y >= area.y + area.height {
            break;
        }
        let row_area = Rect::new(x, row_y, col_width, 1);
        let is_selected = is_active_col && i == cursor;

        let style = if is_selected {
            Style::default()
                .fg(cur_theme.primary_content)
                .bg(cur_theme.primary)
        } else {
            Style::default()
                .fg(cur_theme.base_content)
                .bg(cur_theme.base_200)
        };

        buf.set_style(row_area, style);
        let truncated = truncate_name(name, col_width.saturating_sub(2) as usize);
        let line = Line::from(Span::raw(format!(" {truncated}"))).style(style);
        line.render(row_area, buf);
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

/// Build the initial settings overlay state with classified themes.
pub fn build_settings_state(custom_theme_names: Vec<String>) -> ActiveOverlay {
    let mut light_themes = Vec::new();
    let mut dark_themes = Vec::new();

    for &name in theme::BUILTIN_THEME_NAMES {
        if is_light_theme(name) {
            light_themes.push(name.to_string());
        } else {
            dark_themes.push(name.to_string());
        }
    }

    let mut custom_themes = custom_theme_names;
    custom_themes.sort();

    ActiveOverlay::Settings {
        level: SettingsLevel::TopLevel,
        top_cursor: 0,
        active_column: 0,
        column_cursors: [0; 3],
        light_themes,
        dark_themes,
        custom_themes,
    }
}

/// Returns the currently selected theme name from settings state, if any.
pub fn selected_theme_name(
    active_column: usize,
    column_cursors: [usize; 3],
    light_themes: &[String],
    dark_themes: &[String],
    custom_themes: &[String],
) -> Option<String> {
    let items = match active_column {
        0 => light_themes,
        1 => dark_themes,
        2 => custom_themes,
        _ => return None,
    };
    let cursor = column_cursors[active_column];
    items.get(cursor).cloned()
}

/// Snap a column cursor to the valid range for the given column's items.
pub fn snap_cursor(cursor: usize, item_count: usize) -> usize {
    if item_count == 0 {
        0
    } else {
        cursor.min(item_count - 1)
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
    fn handle_key_esc_produces_settings_esc() {
        assert!(matches!(
            handle_key(key(KeyCode::Esc)),
            Msg::Overlay(OverlayMsg::SettingsEsc)
        ));
    }

    #[test]
    fn handle_key_1_activates() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('1'))),
            Msg::Overlay(OverlayMsg::SettingsActivate)
        ));
    }

    #[test]
    fn handle_key_space_activates() {
        assert!(matches!(
            handle_key(key(KeyCode::Char(' '))),
            Msg::Overlay(OverlayMsg::SettingsActivate)
        ));
    }

    #[test]
    fn handle_key_j_navigates_down() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('j'))),
            Msg::Overlay(OverlayMsg::SettingsNavigate {
                direction: SettingsDirection::Down,
            })
        ));
    }

    #[test]
    fn handle_key_h_navigates_left() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('h'))),
            Msg::Overlay(OverlayMsg::SettingsNavigate {
                direction: SettingsDirection::Left,
            })
        ));
    }

    #[test]
    fn handle_key_unknown_consumed() {
        assert!(matches!(
            handle_key(key(KeyCode::Char('x'))),
            Msg::Overlay(OverlayMsg::Consumed)
        ));
    }

    #[test]
    fn build_settings_state_classifies_themes() {
        let state = build_settings_state(vec!["mycustom".to_string()]);
        match state {
            ActiveOverlay::Settings {
                light_themes,
                dark_themes,
                custom_themes,
                ..
            } => {
                assert!(!light_themes.is_empty());
                assert!(!dark_themes.is_empty());
                assert_eq!(custom_themes, vec!["mycustom".to_string()]);
            }
            _ => panic!("expected Settings variant"),
        }
    }

    #[test]
    fn selected_theme_name_valid() {
        let light = vec!["light".to_string(), "cupcake".to_string()];
        let dark = vec!["dark".to_string()];
        let custom = vec![];
        let name = selected_theme_name(0, [1, 0, 0], &light, &dark, &custom);
        assert_eq!(name, Some("cupcake".to_string()));
    }

    #[test]
    fn selected_theme_name_out_of_range() {
        let light = vec!["light".to_string()];
        let dark = vec![];
        let custom = vec![];
        let name = selected_theme_name(1, [0, 5, 0], &light, &dark, &custom);
        assert_eq!(name, None);
    }

    #[test]
    fn snap_cursor_empty() {
        assert_eq!(snap_cursor(5, 0), 0);
    }

    #[test]
    fn snap_cursor_within_range() {
        assert_eq!(snap_cursor(2, 5), 2);
    }

    #[test]
    fn snap_cursor_clamps() {
        assert_eq!(snap_cursor(10, 3), 2);
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
    fn light_theme_classification() {
        assert!(is_light_theme("light"));
        assert!(is_light_theme("cupcake"));
        assert!(!is_light_theme("dark"));
        assert!(!is_light_theme("dracula"));
    }

    #[test]
    fn footer_commands_present() {
        let cmds = footer_commands();
        assert!(cmds.iter().any(|c| c.description == "Apply"));
        assert!(cmds.iter().any(|c| c.description == "Column"));
    }
}
