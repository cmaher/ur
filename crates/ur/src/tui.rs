use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::Serialize;
use tracing::{debug, info};

use crate::output::OutputManager;

/// Top-level `ur tui` subcommand.
#[derive(Subcommand)]
pub enum TuiArgs {
    /// Manage TUI themes
    Theme {
        #[command(subcommand)]
        command: ThemeCommands,
    },
}

/// `ur tui theme` subcommands.
#[derive(Subcommand)]
pub enum ThemeCommands {
    /// List all available themes (built-in + custom)
    List,
    /// Create a new custom theme
    Create(CreateThemeArgs),
    /// Update an existing custom theme
    Update(UpdateThemeArgs),
    /// Delete a custom theme
    Delete {
        /// Theme name to delete
        name: String,
    },
    /// Select the active theme
    Select {
        /// Theme name to activate
        name: String,
    },
}

#[derive(clap::Args)]
pub struct CreateThemeArgs {
    /// Unique name for the new theme
    pub name: String,
    #[command(flatten)]
    pub colors: ThemeColorFlags,
}

#[derive(clap::Args)]
pub struct UpdateThemeArgs {
    /// Name of the custom theme to update
    pub name: String,
    #[command(flatten)]
    pub colors: ThemeColorFlags,
}

/// Color flags corresponding to ThemeColors fields.
#[derive(clap::Args)]
pub struct ThemeColorFlags {
    /// Background color (hex, e.g. "#1a1b26")
    #[arg(long)]
    pub bg: Option<String>,
    /// Foreground color (hex)
    #[arg(long)]
    pub fg: Option<String>,
    /// Border color (hex)
    #[arg(long)]
    pub border: Option<String>,
    /// Focused border color (hex)
    #[arg(long)]
    pub border_focused: Option<String>,
    /// Use rounded borders
    #[arg(long)]
    pub border_rounded: Option<bool>,
    /// Header background color (hex)
    #[arg(long)]
    pub header_bg: Option<String>,
    /// Header foreground color (hex)
    #[arg(long)]
    pub header_fg: Option<String>,
    /// Selected item background color (hex)
    #[arg(long)]
    pub selected_bg: Option<String>,
    /// Selected item foreground color (hex)
    #[arg(long)]
    pub selected_fg: Option<String>,
    /// Status bar background color (hex)
    #[arg(long)]
    pub status_bar_bg: Option<String>,
    /// Status bar foreground color (hex)
    #[arg(long)]
    pub status_bar_fg: Option<String>,
    /// Error foreground color (hex)
    #[arg(long)]
    pub error_fg: Option<String>,
    /// Warning foreground color (hex)
    #[arg(long)]
    pub warning_fg: Option<String>,
    /// Success foreground color (hex)
    #[arg(long)]
    pub success_fg: Option<String>,
    /// Info foreground color (hex)
    #[arg(long)]
    pub info_fg: Option<String>,
    /// Muted foreground color (hex)
    #[arg(long)]
    pub muted_fg: Option<String>,
    /// Accent color (hex)
    #[arg(long)]
    pub accent: Option<String>,
    /// Highlight color (hex)
    #[arg(long)]
    pub highlight: Option<String>,
    /// Shadow color (hex)
    #[arg(long)]
    pub shadow: Option<String>,
    /// Overlay background color (hex)
    #[arg(long)]
    pub overlay_bg: Option<String>,
}

// ── JSON output structs ──

#[derive(Serialize)]
struct ThemeInfo {
    name: String,
    category: String,
    selected: bool,
}

#[derive(Serialize)]
struct ThemeCreated {
    name: String,
}

#[derive(Serialize)]
struct ThemeUpdated {
    name: String,
}

#[derive(Serialize)]
struct ThemeDeleted {
    name: String,
}

#[derive(Serialize)]
struct ThemeSelected {
    name: String,
}

/// Handle all `ur tui` subcommands.
pub fn handle(command: TuiArgs, config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    match command {
        TuiArgs::Theme { command } => handle_theme(command, config, output),
    }
}

fn handle_theme(
    command: ThemeCommands,
    config: &ur_config::Config,
    output: &OutputManager,
) -> Result<()> {
    match command {
        ThemeCommands::List => theme_list(config, output),
        ThemeCommands::Create(args) => theme_create(config, &args, output),
        ThemeCommands::Update(args) => theme_update(config, &args, output),
        ThemeCommands::Delete { name } => theme_delete(config, &name, output),
        ThemeCommands::Select { name } => theme_select(config, &name, output),
    }
}

fn theme_list(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    debug!("listing themes");

    let mut themes: Vec<ThemeInfo> = Vec::new();

    // Add built-in themes
    for &name in ur_config::BUILTIN_THEME_NAMES {
        let category = builtin_category(name);
        themes.push(ThemeInfo {
            name: name.to_string(),
            category: category.to_string(),
            selected: config.tui.theme_name == name && !config.tui.custom_themes.contains_key(name),
        });
    }

    // Add custom themes
    let mut custom_names: Vec<&String> = config.tui.custom_themes.keys().collect();
    custom_names.sort();
    for name in custom_names {
        themes.push(ThemeInfo {
            name: name.clone(),
            category: "custom".to_string(),
            selected: config.tui.theme_name == *name,
        });
    }

    output.print_items(&themes, |items| {
        let mut out = String::new();
        for t in items {
            let marker = if t.selected { " *" } else { "" };
            out.push_str(&format!(
                "{name}  ({category}){marker}\n",
                name = t.name,
                category = t.category,
                marker = marker,
            ));
        }
        if out.ends_with('\n') {
            out.pop();
        }
        out
    });

    Ok(())
}

/// Classify a built-in theme as "light" or "dark" based on its name.
///
/// daisyUI themes that use a light background are listed here; everything
/// else is classified as "dark".
fn builtin_category(name: &str) -> &'static str {
    match name {
        "light" | "cupcake" | "bumblebee" | "emerald" | "corporate" | "retro" | "cyberpunk"
        | "valentine" | "garden" | "lofi" | "pastel" | "fantasy" | "wireframe" | "cmyk"
        | "autumn" | "acid" | "lemonade" | "winter" | "caramellatte" | "silk" => "light",
        _ => "dark",
    }
}

fn theme_create(
    config: &ur_config::Config,
    args: &CreateThemeArgs,
    output: &OutputManager,
) -> Result<()> {
    let name = &args.name;
    info!(name = %name, "creating custom theme");

    // Validate name uniqueness
    if ur_config::is_builtin_theme(name) {
        bail!("cannot create theme '{name}': name conflicts with a built-in theme");
    }
    if config.tui.custom_themes.contains_key(name) {
        bail!("theme '{name}' already exists — use 'ur tui theme update' to modify it");
    }

    // Build the theme table from flags and write to ur.toml
    write_theme_to_config(config, name, &args.colors)?;

    info!(name = %name, "custom theme created");
    if output.is_json() {
        output.print_success(&ThemeCreated { name: name.clone() });
    } else {
        println!("Created theme '{name}'");
    }
    Ok(())
}

fn theme_update(
    config: &ur_config::Config,
    args: &UpdateThemeArgs,
    output: &OutputManager,
) -> Result<()> {
    let name = &args.name;
    info!(name = %name, "updating custom theme");

    if ur_config::is_builtin_theme(name) {
        bail!("cannot update theme '{name}': built-in themes cannot be modified");
    }
    if !config.tui.custom_themes.contains_key(name) {
        bail!("theme '{name}' not found — use 'ur tui theme create' to create it");
    }

    // Merge: read existing values, overlay with provided flags
    let existing = &config.tui.custom_themes[name];
    let merged = merge_theme_flags(existing, &args.colors);
    write_theme_to_config(config, name, &merged)?;

    info!(name = %name, "custom theme updated");
    if output.is_json() {
        output.print_success(&ThemeUpdated { name: name.clone() });
    } else {
        println!("Updated theme '{name}'");
    }
    Ok(())
}

fn theme_delete(config: &ur_config::Config, name: &str, output: &OutputManager) -> Result<()> {
    info!(name = %name, "deleting custom theme");

    if ur_config::is_builtin_theme(name) {
        bail!("cannot delete theme '{name}': built-in themes cannot be removed");
    }
    if !config.tui.custom_themes.contains_key(name) {
        bail!("theme '{name}' not found");
    }

    let toml_path = config.config_dir.join("ur.toml");
    let contents = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("failed to read {}", toml_path.display()))?;

    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", toml_path.display()))?;

    // Remove [tui.themes.<name>]
    if let Some(tui) = doc.get_mut("tui").and_then(|t| t.as_table_mut())
        && let Some(themes) = tui.get_mut("themes").and_then(|t| t.as_table_mut())
    {
        themes.remove(name);
    }

    // If the deleted theme was selected, revert to "dark"
    if config.tui.theme_name == name {
        ensure_tui_table(&mut doc);
        doc["tui"]["theme"] = toml_edit::value(ur_config::DEFAULT_TUI_THEME);
        if !output.is_json() {
            println!("Active theme was '{name}'; reverted to 'dark'");
        }
    }

    std::fs::write(&toml_path, doc.to_string())
        .with_context(|| format!("failed to write {}", toml_path.display()))?;

    info!(name = %name, "custom theme deleted");
    if output.is_json() {
        output.print_success(&ThemeDeleted {
            name: name.to_string(),
        });
    } else {
        println!("Deleted theme '{name}'");
    }
    Ok(())
}

fn theme_select(config: &ur_config::Config, name: &str, output: &OutputManager) -> Result<()> {
    info!(name = %name, "selecting theme");

    // Theme must exist (built-in or custom)
    if !ur_config::is_builtin_theme(name) && !config.tui.custom_themes.contains_key(name) {
        bail!("theme '{name}' not found — create it first or choose a built-in theme");
    }

    let toml_path = config.config_dir.join("ur.toml");
    let contents = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("failed to read {}", toml_path.display()))?;

    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", toml_path.display()))?;

    ensure_tui_table(&mut doc);
    doc["tui"]["theme"] = toml_edit::value(name);

    std::fs::write(&toml_path, doc.to_string())
        .with_context(|| format!("failed to write {}", toml_path.display()))?;

    info!(name = %name, "theme selected");
    if output.is_json() {
        output.print_success(&ThemeSelected {
            name: name.to_string(),
        });
    } else {
        println!("Selected theme '{name}'");
    }
    Ok(())
}

// ── Helpers ──

/// Ensure the `[tui]` table exists in the document.
fn ensure_tui_table(doc: &mut toml_edit::DocumentMut) {
    if !doc.contains_key("tui") {
        doc["tui"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
}

/// Write a theme's color flags into the `[tui.themes.<name>]` section of ur.toml.
fn write_theme_to_config(
    config: &ur_config::Config,
    name: &str,
    colors: &ThemeColorFlags,
) -> Result<()> {
    let toml_path = config.config_dir.join("ur.toml");
    let contents = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("failed to read {}", toml_path.display()))?;

    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", toml_path.display()))?;

    ensure_tui_table(&mut doc);

    // Ensure [tui.themes] exists
    let tui = doc["tui"].as_table_mut().expect("tui is a table");
    if !tui.contains_key("themes") {
        tui.insert("themes", toml_edit::Item::Table(toml_edit::Table::new()));
    }

    let theme_table = build_theme_table(colors);

    let themes = tui
        .get_mut("themes")
        .and_then(|t| t.as_table_mut())
        .expect("themes is a table");
    themes.insert(name, toml_edit::Item::Table(theme_table));

    std::fs::write(&toml_path, doc.to_string())
        .with_context(|| format!("failed to write {}", toml_path.display()))?;

    Ok(())
}

/// Build a toml_edit Table from theme color flags (only set fields are included).
fn build_theme_table(colors: &ThemeColorFlags) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();

    if let Some(ref v) = colors.bg {
        table.insert("bg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.fg {
        table.insert("fg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.border {
        table.insert("border", toml_edit::value(v));
    }
    if let Some(ref v) = colors.border_focused {
        table.insert("border_focused", toml_edit::value(v));
    }
    if let Some(v) = colors.border_rounded {
        table.insert("border_rounded", toml_edit::value(v));
    }
    if let Some(ref v) = colors.header_bg {
        table.insert("header_bg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.header_fg {
        table.insert("header_fg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.selected_bg {
        table.insert("selected_bg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.selected_fg {
        table.insert("selected_fg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.status_bar_bg {
        table.insert("status_bar_bg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.status_bar_fg {
        table.insert("status_bar_fg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.error_fg {
        table.insert("error_fg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.warning_fg {
        table.insert("warning_fg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.success_fg {
        table.insert("success_fg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.info_fg {
        table.insert("info_fg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.muted_fg {
        table.insert("muted_fg", toml_edit::value(v));
    }
    if let Some(ref v) = colors.accent {
        table.insert("accent", toml_edit::value(v));
    }
    if let Some(ref v) = colors.highlight {
        table.insert("highlight", toml_edit::value(v));
    }
    if let Some(ref v) = colors.shadow {
        table.insert("shadow", toml_edit::value(v));
    }
    if let Some(ref v) = colors.overlay_bg {
        table.insert("overlay_bg", toml_edit::value(v));
    }

    table
}

/// Merge existing ThemeColors with new flag values. Flags take precedence when set.
fn merge_theme_flags(
    existing: &ur_config::ThemeColors,
    flags: &ThemeColorFlags,
) -> ThemeColorFlags {
    ThemeColorFlags {
        bg: flags.bg.clone().or_else(|| existing.bg.clone()),
        fg: flags.fg.clone().or_else(|| existing.fg.clone()),
        border: flags.border.clone().or_else(|| existing.border.clone()),
        border_focused: flags
            .border_focused
            .clone()
            .or_else(|| existing.border_focused.clone()),
        border_rounded: flags.border_rounded.or(existing.border_rounded),
        header_bg: flags
            .header_bg
            .clone()
            .or_else(|| existing.header_bg.clone()),
        header_fg: flags
            .header_fg
            .clone()
            .or_else(|| existing.header_fg.clone()),
        selected_bg: flags
            .selected_bg
            .clone()
            .or_else(|| existing.selected_bg.clone()),
        selected_fg: flags
            .selected_fg
            .clone()
            .or_else(|| existing.selected_fg.clone()),
        status_bar_bg: flags
            .status_bar_bg
            .clone()
            .or_else(|| existing.status_bar_bg.clone()),
        status_bar_fg: flags
            .status_bar_fg
            .clone()
            .or_else(|| existing.status_bar_fg.clone()),
        error_fg: flags.error_fg.clone().or_else(|| existing.error_fg.clone()),
        warning_fg: flags
            .warning_fg
            .clone()
            .or_else(|| existing.warning_fg.clone()),
        success_fg: flags
            .success_fg
            .clone()
            .or_else(|| existing.success_fg.clone()),
        info_fg: flags.info_fg.clone().or_else(|| existing.info_fg.clone()),
        muted_fg: flags.muted_fg.clone().or_else(|| existing.muted_fg.clone()),
        accent: flags.accent.clone().or_else(|| existing.accent.clone()),
        highlight: flags
            .highlight
            .clone()
            .or_else(|| existing.highlight.clone()),
        shadow: flags.shadow.clone().or_else(|| existing.shadow.clone()),
        overlay_bg: flags
            .overlay_bg
            .clone()
            .or_else(|| existing.overlay_bg.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_config(toml_content: &str) -> (TempDir, ur_config::Config) {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ur.toml"), toml_content).unwrap();
        let config = ur_config::Config::load_from(tmp.path()).unwrap();
        (tmp, config)
    }

    fn json_output() -> OutputManager {
        OutputManager::from_args(Some("json"))
    }

    #[test]
    fn list_shows_builtins_and_custom() {
        let (_tmp, config) = setup_config(
            r##"
[tui]
theme = "dark"

[tui.themes.mytheme]
bg = "#112233"
"##,
        );
        // Just verify it doesn't panic
        let output = json_output();
        theme_list(&config, &output).unwrap();
    }

    #[test]
    fn create_rejects_builtin_name() {
        let (_tmp, config) = setup_config("");
        let args = CreateThemeArgs {
            name: "dark".to_string(),
            colors: ThemeColorFlags {
                bg: None,
                fg: None,
                border: None,
                border_focused: None,
                border_rounded: None,
                header_bg: None,
                header_fg: None,
                selected_bg: None,
                selected_fg: None,
                status_bar_bg: None,
                status_bar_fg: None,
                error_fg: None,
                warning_fg: None,
                success_fg: None,
                info_fg: None,
                muted_fg: None,
                accent: None,
                highlight: None,
                shadow: None,
                overlay_bg: None,
            },
        };
        let output = json_output();
        let err = theme_create(&config, &args, &output).unwrap_err();
        assert!(err.to_string().contains("built-in"));
    }

    #[test]
    fn create_and_select_roundtrip() {
        let (tmp, config) = setup_config("");
        let output = json_output();

        // Create
        let args = CreateThemeArgs {
            name: "mytest".to_string(),
            colors: ThemeColorFlags {
                bg: Some("#aabbcc".to_string()),
                fg: None,
                border: None,
                border_focused: None,
                border_rounded: Some(true),
                header_bg: None,
                header_fg: None,
                selected_bg: None,
                selected_fg: None,
                status_bar_bg: None,
                status_bar_fg: None,
                error_fg: None,
                warning_fg: None,
                success_fg: None,
                info_fg: None,
                muted_fg: None,
                accent: None,
                highlight: None,
                shadow: None,
                overlay_bg: None,
            },
        };
        theme_create(&config, &args, &output).unwrap();

        // Reload and verify
        let config2 = ur_config::Config::load_from(tmp.path()).unwrap();
        assert!(config2.tui.custom_themes.contains_key("mytest"));
        assert_eq!(
            config2.tui.custom_themes["mytest"].bg.as_deref(),
            Some("#aabbcc")
        );
        assert_eq!(
            config2.tui.custom_themes["mytest"].border_rounded,
            Some(true)
        );

        // Select
        theme_select(&config2, "mytest", &output).unwrap();
        let config3 = ur_config::Config::load_from(tmp.path()).unwrap();
        assert_eq!(config3.tui.theme_name, "mytest");
    }

    #[test]
    fn delete_reverts_selection() {
        let (tmp, config) = setup_config(
            r##"
[tui]
theme = "mytheme"

[tui.themes.mytheme]
bg = "#112233"
"##,
        );
        let output = json_output();
        theme_delete(&config, "mytheme", &output).unwrap();

        let config2 = ur_config::Config::load_from(tmp.path()).unwrap();
        assert!(!config2.tui.custom_themes.contains_key("mytheme"));
        assert_eq!(config2.tui.theme_name, "dark");
    }

    #[test]
    fn delete_rejects_builtin() {
        let (_tmp, config) = setup_config("");
        let output = json_output();
        let err = theme_delete(&config, "dark", &output).unwrap_err();
        assert!(err.to_string().contains("built-in"));
    }

    #[test]
    fn update_rejects_builtin() {
        let (_tmp, config) = setup_config("");
        let args = UpdateThemeArgs {
            name: "dark".to_string(),
            colors: ThemeColorFlags {
                bg: Some("#000000".to_string()),
                fg: None,
                border: None,
                border_focused: None,
                border_rounded: None,
                header_bg: None,
                header_fg: None,
                selected_bg: None,
                selected_fg: None,
                status_bar_bg: None,
                status_bar_fg: None,
                error_fg: None,
                warning_fg: None,
                success_fg: None,
                info_fg: None,
                muted_fg: None,
                accent: None,
                highlight: None,
                shadow: None,
                overlay_bg: None,
            },
        };
        let output = json_output();
        let err = theme_update(&config, &args, &output).unwrap_err();
        assert!(err.to_string().contains("built-in"));
    }

    #[test]
    fn update_merges_fields() {
        let (tmp, config) = setup_config(
            r##"
[tui.themes.mine]
bg = "#111111"
fg = "#222222"
"##,
        );
        let output = json_output();
        let args = UpdateThemeArgs {
            name: "mine".to_string(),
            colors: ThemeColorFlags {
                bg: Some("#aaaaaa".to_string()),
                fg: None,
                border: None,
                border_focused: None,
                border_rounded: None,
                header_bg: None,
                header_fg: None,
                selected_bg: None,
                selected_fg: None,
                status_bar_bg: None,
                status_bar_fg: None,
                error_fg: None,
                warning_fg: None,
                success_fg: None,
                info_fg: None,
                muted_fg: None,
                accent: None,
                highlight: None,
                shadow: None,
                overlay_bg: None,
            },
        };
        theme_update(&config, &args, &output).unwrap();

        let config2 = ur_config::Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            config2.tui.custom_themes["mine"].bg.as_deref(),
            Some("#aaaaaa")
        );
        assert_eq!(
            config2.tui.custom_themes["mine"].fg.as_deref(),
            Some("#222222")
        );
    }

    #[test]
    fn select_rejects_unknown_theme() {
        let (_tmp, config) = setup_config("");
        let output = json_output();
        let err = theme_select(&config, "nonexistent", &output).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn create_rejects_duplicate_custom() {
        let (_tmp, config) = setup_config(
            r##"
[tui.themes.existing]
bg = "#000000"
"##,
        );
        let output = json_output();
        let args = CreateThemeArgs {
            name: "existing".to_string(),
            colors: ThemeColorFlags {
                bg: None,
                fg: None,
                border: None,
                border_focused: None,
                border_rounded: None,
                header_bg: None,
                header_fg: None,
                selected_bg: None,
                selected_fg: None,
                status_bar_bg: None,
                status_bar_fg: None,
                error_fg: None,
                warning_fg: None,
                success_fg: None,
                info_fg: None,
                muted_fg: None,
                accent: None,
                highlight: None,
                shadow: None,
                overlay_bg: None,
            },
        };
        let err = theme_create(&config, &args, &output).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn select_accepts_builtin() {
        let (tmp, _config) = setup_config("");
        let config = ur_config::Config::load_from(tmp.path()).unwrap();
        let output = json_output();
        theme_select(&config, "nord", &output).unwrap();
        let config2 = ur_config::Config::load_from(tmp.path()).unwrap();
        assert_eq!(config2.tui.theme_name, "nord");
    }

    #[test]
    fn builtin_category_classification() {
        assert_eq!(builtin_category("light"), "light");
        assert_eq!(builtin_category("dark"), "dark");
        assert_eq!(builtin_category("cupcake"), "light");
        assert_eq!(builtin_category("dracula"), "dark");
        assert_eq!(builtin_category("nord"), "dark");
    }
}
