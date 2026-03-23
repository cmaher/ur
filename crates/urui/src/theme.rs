use ratatui::style::Color;
use ur_config::TuiConfig;

/// Semantic color theme for the TUI.
///
/// Contains 20 named colors following the daisyUI semantic naming convention,
/// plus a `border_rounded` flag controlling border style. Built-in themes are
/// generated at compile time from `themes/themes.css`; custom themes from
/// `ur.toml` can partially override the dark defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub base_100: Color,
    pub base_200: Color,
    pub base_300: Color,
    pub base_content: Color,
    pub primary: Color,
    pub primary_content: Color,
    pub secondary: Color,
    pub secondary_content: Color,
    pub accent: Color,
    pub accent_content: Color,
    pub neutral: Color,
    pub neutral_content: Color,
    pub info: Color,
    pub info_content: Color,
    pub success: Color,
    pub success_content: Color,
    pub warning: Color,
    pub warning_content: Color,
    pub error: Color,
    pub error_content: Color,
    pub border_rounded: bool,
}

/// Private module to include the generated code which creates `Theme` structs
/// without `border_rounded`. We define a compatible struct here and re-export
/// through conversion functions.
mod generated {
    #![allow(clippy::too_many_lines)]
    use ratatui::style::Color;

    /// Color-only theme struct matching the generated code's struct literals
    /// (20 color fields, no `border_rounded`).
    #[derive(Debug, Clone)]
    pub struct Theme {
        pub base_100: Color,
        pub base_200: Color,
        pub base_300: Color,
        pub base_content: Color,
        pub primary: Color,
        pub primary_content: Color,
        pub secondary: Color,
        pub secondary_content: Color,
        pub accent: Color,
        pub accent_content: Color,
        pub neutral: Color,
        pub neutral_content: Color,
        pub info: Color,
        pub info_content: Color,
        pub success: Color,
        pub success_content: Color,
        pub warning: Color,
        pub warning_content: Color,
        pub error: Color,
        pub error_content: Color,
    }

    include!(concat!(env!("OUT_DIR"), "/builtin_themes.rs"));
}

/// Returns a built-in theme by name with `border_rounded` defaulting to `false`.
pub fn builtin_theme(name: &str) -> Option<Theme> {
    generated::builtin_theme(name).map(|g| Theme {
        base_100: g.base_100,
        base_200: g.base_200,
        base_300: g.base_300,
        base_content: g.base_content,
        primary: g.primary,
        primary_content: g.primary_content,
        secondary: g.secondary,
        secondary_content: g.secondary_content,
        accent: g.accent,
        accent_content: g.accent_content,
        neutral: g.neutral,
        neutral_content: g.neutral_content,
        info: g.info,
        info_content: g.info_content,
        success: g.success,
        success_content: g.success_content,
        warning: g.warning,
        warning_content: g.warning_content,
        error: g.error,
        error_content: g.error_content,
        border_rounded: false,
    })
}

/// All built-in theme names, sorted alphabetically.
pub const BUILTIN_THEME_NAMES: &[&str] = generated::BUILTIN_THEME_NAMES;

/// Returns a theme that uses ANSI color indices, delegating color rendering to
/// the terminal's own palette. Works in any terminal without truecolor support.
pub fn system_theme() -> Theme {
    Theme {
        base_100: Color::Reset,
        base_200: Color::Black,
        base_300: Color::DarkGray,
        base_content: Color::Reset,
        primary: Color::Blue,
        primary_content: Color::White,
        secondary: Color::Cyan,
        secondary_content: Color::White,
        accent: Color::Magenta,
        accent_content: Color::White,
        neutral: Color::Black,
        neutral_content: Color::DarkGray,
        info: Color::Cyan,
        info_content: Color::DarkGray,
        success: Color::Green,
        success_content: Color::White,
        warning: Color::Yellow,
        warning_content: Color::Black,
        error: Color::Red,
        error_content: Color::White,
        border_rounded: true,
    }
}

impl Theme {
    /// Resolve the active theme from configuration.
    ///
    /// Resolution order:
    /// 1. If the configured theme name matches a custom theme, use it (with
    ///    unspecified fields falling back to the dark built-in defaults).
    /// 2. If the name matches a built-in theme, use it directly.
    /// 3. Fall back to the "dark" built-in theme.
    pub fn resolve(config: &TuiConfig) -> Self {
        check_truecolor_support();

        let dark = builtin_theme("dark").expect("dark theme must exist in built-in themes");

        // 1. Check custom themes from config.
        if let Some(custom) = config.custom_themes.get(&config.theme_name) {
            return apply_custom_overrides(&dark, custom);
        }

        // 2. Check built-in themes.
        if let Some(builtin) = builtin_theme(&config.theme_name) {
            return builtin;
        }

        // 3. Fall back to dark.
        dark
    }
}

/// Parse a hex color string of the form `#rrggbb` into a ratatui `Color::Rgb`.
///
/// Returns `None` if the string is not exactly 7 characters or contains
/// non-hex digits.
pub fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

/// Emit a warning to stderr if the terminal does not advertise 24-bit color
/// support via `COLORTERM`.
fn check_truecolor_support() {
    match std::env::var("COLORTERM") {
        Ok(val) if val == "truecolor" || val == "24bit" => {}
        _ => {
            eprintln!(
                "warning: COLORTERM is not set to 'truecolor' or '24bit'; \
                 theme colors may not render correctly"
            );
        }
    }
}

/// Apply a user-defined `ThemeColors` on top of the dark theme defaults.
///
/// Each `Option<String>` field in the custom theme, when `Some`, is parsed as
/// a hex color. Fields that are `None` or fail to parse retain the dark
/// default value.
fn apply_custom_overrides(dark: &Theme, custom: &ur_config::ThemeColors) -> Theme {
    let mut theme = dark.clone();

    /// Try to parse `opt` as a hex color and write it into `field` on success.
    fn apply(field: &mut Color, opt: &Option<String>) {
        if let Some(ref v) = *opt
            && let Some(c) = parse_hex_color(v)
        {
            *field = c;
        }
    }

    apply(&mut theme.base_100, &custom.bg);
    apply(&mut theme.base_content, &custom.fg);
    apply(&mut theme.base_200, &custom.border);
    apply(&mut theme.base_300, &custom.border_focused);
    if let Some(v) = custom.border_rounded {
        theme.border_rounded = v;
    }
    apply(&mut theme.primary, &custom.header_bg);
    apply(&mut theme.primary_content, &custom.header_fg);
    apply(&mut theme.secondary, &custom.selected_bg);
    apply(&mut theme.secondary_content, &custom.selected_fg);
    apply(&mut theme.neutral, &custom.status_bar_bg);
    apply(&mut theme.neutral_content, &custom.status_bar_fg);
    apply(&mut theme.error, &custom.error_fg);
    apply(&mut theme.warning, &custom.warning_fg);
    apply(&mut theme.success, &custom.success_fg);
    apply(&mut theme.info, &custom.info_fg);
    apply(&mut theme.accent, &custom.accent);
    apply(&mut theme.accent_content, &custom.highlight);
    apply(&mut theme.info_content, &custom.shadow);
    apply(&mut theme.warning_content, &custom.overlay_bg);

    theme
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_color_valid() {
        assert_eq!(parse_hex_color("#ff00ff"), Some(Color::Rgb(255, 0, 255)));
        assert_eq!(parse_hex_color("#000000"), Some(Color::Rgb(0, 0, 0)));
        assert_eq!(
            parse_hex_color("#1a2b3c"),
            Some(Color::Rgb(0x1a, 0x2b, 0x3c))
        );
    }

    #[test]
    fn parse_hex_color_invalid() {
        assert_eq!(parse_hex_color("ff00ff"), None); // missing #
        assert_eq!(parse_hex_color("#fff"), None); // too short
        assert_eq!(parse_hex_color("#gggggg"), None); // non-hex
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn builtin_dark_theme_exists() {
        assert!(builtin_theme("dark").is_some());
    }

    #[test]
    fn builtin_theme_names_contains_dark() {
        assert!(BUILTIN_THEME_NAMES.contains(&"dark"));
    }

    #[test]
    fn resolve_falls_back_to_dark() {
        let config = TuiConfig {
            theme_name: "nonexistent_theme_xyz".to_string(),
            ..TuiConfig::default()
        };
        let resolved = Theme::resolve(&config);
        let dark = builtin_theme("dark").unwrap();
        assert_eq!(resolved, dark);
    }

    #[test]
    fn resolve_uses_builtin_by_name() {
        // Pick a theme that is not "dark" (if available).
        if BUILTIN_THEME_NAMES.len() > 1 {
            let name = BUILTIN_THEME_NAMES.iter().find(|&&n| n != "dark").unwrap();
            let config = TuiConfig {
                theme_name: name.to_string(),
                ..TuiConfig::default()
            };
            let resolved = Theme::resolve(&config);
            let expected = builtin_theme(name).unwrap();
            assert_eq!(resolved, expected);
        }
    }

    #[test]
    fn resolve_custom_theme_overrides_dark() {
        use std::collections::HashMap;

        let custom = ur_config::ThemeColors {
            bg: Some("#112233".to_string()),
            border_rounded: Some(true),
            ..Default::default()
        };

        let mut custom_themes = HashMap::new();
        custom_themes.insert("mycustom".to_string(), custom);

        let config = TuiConfig {
            theme_name: "mycustom".to_string(),
            custom_themes,
            ..TuiConfig::default()
        };

        let resolved = Theme::resolve(&config);
        assert_eq!(resolved.base_100, Color::Rgb(0x11, 0x22, 0x33));
        assert!(resolved.border_rounded);

        // Non-overridden fields should match dark.
        let dark = builtin_theme("dark").unwrap();
        assert_eq!(resolved.primary, dark.primary);
    }
}
