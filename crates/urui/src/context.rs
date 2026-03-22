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
}
