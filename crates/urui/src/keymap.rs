use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ur_config::KeymapOverrides;

use crate::page::TabId;

/// Semantic actions resolved from raw key events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    NavigateUp,
    NavigateDown,
    PageLeft,
    PageRight,
    Select,
    Back,
    Quit,
    SwitchTab(TabId),
    Refresh,
    Filter,
    SetPriority,
    Dispatch,
}

/// A resolved key binding: modifier flags + key code.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

/// Maps raw `KeyEvent`s to semantic `Action`s.
///
/// The default keymap provides vim-style and arrow-key bindings.
/// When a `KeymapOverrides` config is applied via [`Keymap::from_config`],
/// all defaults are fully replaced — there is no merging.
#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: HashMap<KeyBinding, Action>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        insert_navigation_bindings(&mut bindings);
        insert_fixed_action_bindings(&mut bindings);
        Self { bindings }
    }
}

/// Insert vim-style and arrow-key navigation bindings (up/down/left/right).
fn insert_navigation_bindings(bindings: &mut HashMap<KeyBinding, Action>) {
    // navigate_up = [k, Up]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
        },
        Action::NavigateUp,
    );
    bindings.insert(
        KeyBinding {
            code: KeyCode::Up,
            modifiers: KeyModifiers::NONE,
        },
        Action::NavigateUp,
    );

    // navigate_down = [j, Down]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
        },
        Action::NavigateDown,
    );
    bindings.insert(
        KeyBinding {
            code: KeyCode::Down,
            modifiers: KeyModifiers::NONE,
        },
        Action::NavigateDown,
    );

    // navigate_left = [h, Left]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('h'),
            modifiers: KeyModifiers::NONE,
        },
        Action::PageLeft,
    );
    bindings.insert(
        KeyBinding {
            code: KeyCode::Left,
            modifiers: KeyModifiers::NONE,
        },
        Action::PageLeft,
    );

    // navigate_right = [l, Right]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::NONE,
        },
        Action::PageRight,
    );
    bindings.insert(
        KeyBinding {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
        },
        Action::PageRight,
    );
}

/// Insert fixed action bindings: tabs, refresh, filter, priority, dispatch,
/// select, back, quit. These are not overridable via KeymapOverrides.
fn insert_fixed_action_bindings(bindings: &mut HashMap<KeyBinding, Action>) {
    // tab_tickets = [t]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('t'),
            modifiers: KeyModifiers::NONE,
        },
        Action::SwitchTab(TabId::Tickets),
    );

    // tab_flows = [f]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('f'),
            modifiers: KeyModifiers::NONE,
        },
        Action::SwitchTab(TabId::Flows),
    );

    // refresh = [r]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('r'),
            modifiers: KeyModifiers::NONE,
        },
        Action::Refresh,
    );

    // filter = [*]
    // Note: `*` is not uppercase, so normalize_modifiers strips SHIFT.
    // Store with NONE to match the normalized key event.
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('*'),
            modifiers: KeyModifiers::NONE,
        },
        Action::Filter,
    );

    // set_priority = [P]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('P'),
            modifiers: KeyModifiers::SHIFT,
        },
        Action::SetPriority,
    );

    // dispatch = [D]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('D'),
            modifiers: KeyModifiers::SHIFT,
        },
        Action::Dispatch,
    );

    // select = [Enter]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
        },
        Action::Select,
    );

    // back = [q]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::NONE,
        },
        Action::Back,
    );

    // quit = [Q]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('Q'),
            modifiers: KeyModifiers::SHIFT,
        },
        Action::Quit,
    );
}

impl Keymap {
    /// Build a keymap from user-supplied config overrides.
    ///
    /// This **fully replaces** the default keymap — only bindings explicitly
    /// specified in the overrides will be present.
    pub fn from_config(overrides: KeymapOverrides) -> Self {
        let mut bindings = HashMap::new();

        insert_bindings(&mut bindings, overrides.scroll_up, Action::NavigateUp);
        insert_bindings(&mut bindings, overrides.scroll_down, Action::NavigateDown);
        insert_bindings(&mut bindings, overrides.page_up, Action::PageLeft);
        insert_bindings(&mut bindings, overrides.page_down, Action::PageRight);
        insert_bindings(&mut bindings, overrides.select, Action::Select);
        insert_bindings(&mut bindings, overrides.cancel, Action::Back);
        insert_bindings(&mut bindings, overrides.quit, Action::Quit);

        // Tab switching is not part of KeymapOverrides; preserve defaults
        // for SwitchTab actions when building from config.
        bindings.insert(
            KeyBinding {
                code: KeyCode::Char('t'),
                modifiers: KeyModifiers::NONE,
            },
            Action::SwitchTab(TabId::Tickets),
        );
        bindings.insert(
            KeyBinding {
                code: KeyCode::Char('f'),
                modifiers: KeyModifiers::NONE,
            },
            Action::SwitchTab(TabId::Flows),
        );

        // Refresh, Filter, SetPriority, Dispatch are not part of
        // KeymapOverrides; preserve defaults when building from config.
        bindings.insert(
            KeyBinding {
                code: KeyCode::Char('r'),
                modifiers: KeyModifiers::NONE,
            },
            Action::Refresh,
        );
        bindings.insert(
            KeyBinding {
                code: KeyCode::Char('*'),
                modifiers: KeyModifiers::NONE,
            },
            Action::Filter,
        );
        bindings.insert(
            KeyBinding {
                code: KeyCode::Char('P'),
                modifiers: KeyModifiers::SHIFT,
            },
            Action::SetPriority,
        );
        bindings.insert(
            KeyBinding {
                code: KeyCode::Char('D'),
                modifiers: KeyModifiers::SHIFT,
            },
            Action::Dispatch,
        );

        Self { bindings }
    }

    /// Resolve a raw `KeyEvent` to a semantic `Action`, if any binding matches.
    pub fn resolve(&self, event: KeyEvent) -> Option<Action> {
        let binding = KeyBinding {
            code: event.code,
            modifiers: normalize_modifiers(event),
        };
        self.bindings.get(&binding).cloned()
    }
}

/// Normalize modifiers for matching purposes.
///
/// Crossterm reports `SHIFT` for uppercase characters on some platforms,
/// but not others. We normalize so that `Char('Q')` always carries `SHIFT`.
fn normalize_modifiers(event: KeyEvent) -> KeyModifiers {
    let mut mods = event.modifiers;
    if let KeyCode::Char(c) = event.code {
        if c.is_ascii_uppercase() {
            mods |= KeyModifiers::SHIFT;
        } else {
            mods -= KeyModifiers::SHIFT;
        }
    }
    mods
}

/// Insert parsed key bindings for an action from an optional config field.
fn insert_bindings(
    bindings: &mut HashMap<KeyBinding, Action>,
    keys: Option<Vec<String>>,
    action: Action,
) {
    if let Some(key_strs) = keys {
        for s in &key_strs {
            if let Some(kb) = parse_key_binding(s) {
                bindings.insert(kb, action.clone());
            }
        }
    }
}

/// Parse a human-readable key binding string into a `KeyBinding`.
///
/// Supported formats:
/// - Single character: `"k"` -> `Char('k')`
/// - Named keys: `"Up"`, `"Down"`, `"Left"`, `"Right"`, `"Enter"`, `"Space"`,
///   `"Tab"`, `"Backspace"`, `"Delete"`, `"Esc"`, `"Home"`, `"End"`,
///   `"PageUp"`, `"PageDown"`
/// - Modifier prefixes: `"C-c"` -> Ctrl+c, `"S-a"` -> Shift+a
/// - Function keys: `"F1"` through `"F12"`
pub fn parse_key_binding(s: &str) -> Option<KeyBinding> {
    // Check for modifier prefix: "C-..." for Ctrl, "S-..." for Shift
    if let Some(rest) = s.strip_prefix("C-") {
        let inner = parse_key_binding(rest)?;
        return Some(KeyBinding {
            code: inner.code,
            modifiers: inner.modifiers | KeyModifiers::CONTROL,
        });
    }
    if let Some(rest) = s.strip_prefix("S-") {
        let inner = parse_key_binding(rest)?;
        return Some(KeyBinding {
            code: inner.code,
            modifiers: inner.modifiers | KeyModifiers::SHIFT,
        });
    }

    // Named keys
    let (code, modifiers) = match s {
        "Up" => (KeyCode::Up, KeyModifiers::NONE),
        "Down" => (KeyCode::Down, KeyModifiers::NONE),
        "Left" => (KeyCode::Left, KeyModifiers::NONE),
        "Right" => (KeyCode::Right, KeyModifiers::NONE),
        "Enter" => (KeyCode::Enter, KeyModifiers::NONE),
        "Space" => (KeyCode::Char(' '), KeyModifiers::NONE),
        "Tab" => (KeyCode::Tab, KeyModifiers::NONE),
        "Backspace" => (KeyCode::Backspace, KeyModifiers::NONE),
        "Delete" => (KeyCode::Delete, KeyModifiers::NONE),
        "Esc" => (KeyCode::Esc, KeyModifiers::NONE),
        "Home" => (KeyCode::Home, KeyModifiers::NONE),
        "End" => (KeyCode::End, KeyModifiers::NONE),
        "PageUp" => (KeyCode::PageUp, KeyModifiers::NONE),
        "PageDown" => (KeyCode::PageDown, KeyModifiers::NONE),
        "F1" => (KeyCode::F(1), KeyModifiers::NONE),
        "F2" => (KeyCode::F(2), KeyModifiers::NONE),
        "F3" => (KeyCode::F(3), KeyModifiers::NONE),
        "F4" => (KeyCode::F(4), KeyModifiers::NONE),
        "F5" => (KeyCode::F(5), KeyModifiers::NONE),
        "F6" => (KeyCode::F(6), KeyModifiers::NONE),
        "F7" => (KeyCode::F(7), KeyModifiers::NONE),
        "F8" => (KeyCode::F(8), KeyModifiers::NONE),
        "F9" => (KeyCode::F(9), KeyModifiers::NONE),
        "F10" => (KeyCode::F(10), KeyModifiers::NONE),
        "F11" => (KeyCode::F(11), KeyModifiers::NONE),
        "F12" => (KeyCode::F(12), KeyModifiers::NONE),
        other => {
            // Single character
            let mut chars = other.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                // Unknown multi-char string
                return None;
            }
            let mods = if c.is_ascii_uppercase() {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            (KeyCode::Char(c), mods)
        }
    };

    Some(KeyBinding { code, modifiers })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_char() {
        let kb = parse_key_binding("k").unwrap();
        assert_eq!(kb.code, KeyCode::Char('k'));
        assert_eq!(kb.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_uppercase_char() {
        let kb = parse_key_binding("Q").unwrap();
        assert_eq!(kb.code, KeyCode::Char('Q'));
        assert_eq!(kb.modifiers, KeyModifiers::SHIFT);
    }

    #[test]
    fn parse_named_keys() {
        assert_eq!(parse_key_binding("Up").unwrap().code, KeyCode::Up);
        assert_eq!(parse_key_binding("Down").unwrap().code, KeyCode::Down);
        assert_eq!(parse_key_binding("Left").unwrap().code, KeyCode::Left);
        assert_eq!(parse_key_binding("Right").unwrap().code, KeyCode::Right);
        assert_eq!(parse_key_binding("Enter").unwrap().code, KeyCode::Enter);
        assert_eq!(parse_key_binding("Tab").unwrap().code, KeyCode::Tab);
        assert_eq!(parse_key_binding("Esc").unwrap().code, KeyCode::Esc);
    }

    #[test]
    fn parse_space() {
        let kb = parse_key_binding("Space").unwrap();
        assert_eq!(kb.code, KeyCode::Char(' '));
        assert_eq!(kb.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_ctrl_modifier() {
        let kb = parse_key_binding("C-c").unwrap();
        assert_eq!(kb.code, KeyCode::Char('c'));
        assert_eq!(kb.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_shift_modifier() {
        let kb = parse_key_binding("S-a").unwrap();
        assert_eq!(kb.code, KeyCode::Char('a'));
        assert_eq!(kb.modifiers, KeyModifiers::SHIFT);
    }

    #[test]
    fn parse_function_key() {
        let kb = parse_key_binding("F1").unwrap();
        assert_eq!(kb.code, KeyCode::F(1));
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse_key_binding("UnknownKey").is_none());
    }

    #[test]
    fn default_keymap_navigate_up() {
        let keymap = Keymap::default();
        let event_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        let event_up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(keymap.resolve(event_k), Some(Action::NavigateUp));
        assert_eq!(keymap.resolve(event_up), Some(Action::NavigateUp));
    }

    #[test]
    fn default_keymap_navigate_down() {
        let keymap = Keymap::default();
        let event_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let event_down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(keymap.resolve(event_j), Some(Action::NavigateDown));
        assert_eq!(keymap.resolve(event_down), Some(Action::NavigateDown));
    }

    #[test]
    fn default_keymap_page_left_right() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
            Some(Action::PageLeft),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            Some(Action::PageLeft),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)),
            Some(Action::PageRight),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            Some(Action::PageRight),
        );
    }

    #[test]
    fn default_keymap_tabs() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)),
            Some(Action::SwitchTab(TabId::Tickets)),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)),
            Some(Action::SwitchTab(TabId::Flows)),
        );
    }

    #[test]
    fn default_keymap_select_back_quit() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::Select),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Action::Back),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT)),
            Some(Action::Quit),
        );
    }

    #[test]
    fn default_keymap_unbound_returns_none() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)),
            None,
        );
    }

    #[test]
    fn from_config_replaces_defaults() {
        let overrides = KeymapOverrides {
            scroll_up: Some(vec!["w".into()]),
            scroll_down: Some(vec!["s".into()]),
            quit: Some(vec!["C-c".into()]),
            ..KeymapOverrides::default()
        };

        let keymap = Keymap::from_config(overrides);

        // Custom bindings work
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)),
            Some(Action::NavigateUp),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)),
            Some(Action::NavigateDown),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit),
        );

        // Default bindings for replaced actions are gone
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)),
            None,
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
            None,
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT)),
            None,
        );

        // Tab bindings preserved as defaults in from_config
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)),
            Some(Action::SwitchTab(TabId::Tickets)),
        );

        // Refresh, Filter, SetPriority, Dispatch preserved as defaults
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
            Some(Action::Refresh),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('*'), KeyModifiers::SHIFT)),
            Some(Action::Filter), // SHIFT stripped by normalize_modifiers for non-uppercase
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT)),
            Some(Action::SetPriority),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT)),
            Some(Action::Dispatch),
        );
    }

    #[test]
    fn default_keymap_refresh() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
            Some(Action::Refresh),
        );
    }

    #[test]
    fn default_keymap_filter() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('*'), KeyModifiers::SHIFT)),
            Some(Action::Filter), // SHIFT stripped by normalize_modifiers for non-uppercase
        );
    }

    #[test]
    fn default_keymap_set_priority() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT)),
            Some(Action::SetPriority),
        );
    }

    #[test]
    fn default_keymap_dispatch() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT)),
            Some(Action::Dispatch),
        );
    }
}
