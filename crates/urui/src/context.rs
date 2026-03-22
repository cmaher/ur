use std::collections::HashMap;

use ur_config::ProjectConfig;

use crate::keymap::Keymap;
use crate::theme::Theme;

/// Cross-cutting context passed to all TUI render functions.
///
/// Holds the resolved theme and keymap so that widgets can access colors and
/// key bindings without threading individual parameters through every call.
#[derive(Debug, Clone)]
pub struct TuiContext {
    pub theme: Theme,
    pub keymap: Keymap,
    /// Project keys from ur.toml configuration, sorted alphabetically.
    pub projects: Vec<String>,
    /// Full project configurations keyed by project key, used for dispatch.
    pub project_configs: HashMap<String, ProjectConfig>,
}
