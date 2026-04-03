use std::collections::HashMap;
use std::path::PathBuf;

use ur_config::{ProjectConfig, TuiConfig};

use crate::keymap::Keymap;
use crate::theme::Theme;

/// Cross-cutting context passed to all TUI render functions.
///
/// Holds the resolved theme and keymap so that widgets can access colors and
/// key bindings without threading individual parameters through every call.
#[derive(Debug, Clone)]
pub struct TuiContext {
    pub theme: Theme,
    #[allow(dead_code)]
    pub keymap: Keymap,
    /// Project keys from ur.toml configuration, sorted alphabetically.
    pub projects: Vec<String>,
    /// Full project configurations keyed by project key, used for dispatch.
    #[allow(dead_code)]
    pub project_configs: HashMap<String, ProjectConfig>,
    /// TUI configuration (needed for theme resolution on swap).
    pub tui_config: TuiConfig,
    /// Root config directory for persisting settings.
    #[allow(dead_code)]
    pub config_dir: PathBuf,
    /// When set, scopes the TUI to a single project key.
    pub project_filter: Option<String>,
}

impl TuiContext {
    /// Swap the active theme by name.
    ///
    /// Updates the theme_name in the TUI config and re-resolves the theme.
    pub fn swap_theme(&mut self, name: &str) {
        self.tui_config.theme_name = name.to_string();
        self.theme = Theme::resolve(&self.tui_config);
    }
}
