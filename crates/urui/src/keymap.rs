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
    CloseTicket,
    OpenTicket,
    CancelFlow,
    OpenSettings,
    CreateTicket,
    LaunchDesign,
    OpenActivities,
    DispatchAll,
    OpenDescription,
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

    insert_ctrl_page_bindings(bindings);
}

/// Insert Ctrl-F (page forward) and Ctrl-B (page backward) bindings.
///
/// These are always present regardless of user `KeymapOverrides`.
fn insert_ctrl_page_bindings(bindings: &mut HashMap<KeyBinding, Action>) {
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('f'),
            modifiers: KeyModifiers::CONTROL,
        },
        Action::PageRight,
    );
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('b'),
            modifiers: KeyModifiers::CONTROL,
        },
        Action::PageLeft,
    );
}

/// Insert fixed action bindings: tabs, refresh, filter, settings.
/// These are not overridable via KeymapOverrides.
fn insert_fixed_action_bindings(bindings: &mut HashMap<KeyBinding, Action>) {
    insert_tab_and_ui_bindings(bindings);
    insert_ticket_action_bindings(bindings);
}

/// Insert tab switching, refresh, filter, and settings bindings.
fn insert_tab_and_ui_bindings(bindings: &mut HashMap<KeyBinding, Action>) {
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

    // tab_workers = [w]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('w'),
            modifiers: KeyModifiers::NONE,
        },
        Action::SwitchTab(TabId::Workers),
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

    // open_settings = [,]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char(','),
            modifiers: KeyModifiers::NONE,
        },
        Action::OpenSettings,
    );
}

/// Insert ticket and workflow action bindings: select, back, quit, priority,
/// dispatch, close, open, create, launch design.
fn insert_ticket_action_bindings(bindings: &mut HashMap<KeyBinding, Action>) {
    // select = [Enter, Space]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
        },
        Action::Select,
    );
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::NONE,
        },
        Action::Select,
    );

    // back = [Esc]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
        },
        Action::Back,
    );

    // back (also) = [q]
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

    // close_ticket = [X]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('X'),
            modifiers: KeyModifiers::SHIFT,
        },
        Action::CloseTicket,
    );

    // open_ticket = [O]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('O'),
            modifiers: KeyModifiers::SHIFT,
        },
        Action::OpenTicket,
    );

    // create_ticket = [C]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('C'),
            modifiers: KeyModifiers::SHIFT,
        },
        Action::CreateTicket,
    );

    // launch_design = [S]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('S'),
            modifiers: KeyModifiers::SHIFT,
        },
        Action::LaunchDesign,
    );

    // open_activities = [a]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
        },
        Action::OpenActivities,
    );

    // dispatch_all = [A]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('A'),
            modifiers: KeyModifiers::SHIFT,
        },
        Action::DispatchAll,
    );

    // open_description = [d]
    bindings.insert(
        KeyBinding {
            code: KeyCode::Char('d'),
            modifiers: KeyModifiers::NONE,
        },
        Action::OpenDescription,
    );
}

/// Insert ticket action bindings that are NOT overridable via `KeymapOverrides`.
/// Select, Back, and Quit are handled via overrides; the rest are always present.
fn insert_non_overridable_ticket_bindings(bindings: &mut HashMap<KeyBinding, Action>) {
    for (ch, mods, action) in [
        ('P', KeyModifiers::SHIFT, Action::SetPriority),
        ('D', KeyModifiers::SHIFT, Action::Dispatch),
        ('X', KeyModifiers::SHIFT, Action::CloseTicket),
        ('O', KeyModifiers::SHIFT, Action::OpenTicket),
        ('C', KeyModifiers::SHIFT, Action::CreateTicket),
        ('S', KeyModifiers::SHIFT, Action::LaunchDesign),
        ('a', KeyModifiers::NONE, Action::OpenActivities),
        ('A', KeyModifiers::SHIFT, Action::DispatchAll),
        ('d', KeyModifiers::NONE, Action::OpenDescription),
    ] {
        bindings.insert(
            KeyBinding {
                code: KeyCode::Char(ch),
                modifiers: mods,
            },
            action,
        );
    }
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

        insert_ctrl_page_bindings(&mut bindings);
        // Tab switching and UI bindings are not overridable.
        insert_tab_and_ui_bindings(&mut bindings);
        // Ticket action bindings that are not part of KeymapOverrides.
        insert_non_overridable_ticket_bindings(&mut bindings);

        Self { bindings }
    }

    /// Returns a display label for the primary key bound to the given action.
    ///
    /// Prefers short character labels over named keys (arrows, etc.).
    pub fn label_for(&self, action: &Action) -> String {
        let mut char_label: Option<String> = None;
        let mut named_label: Option<String> = None;

        let matching = self
            .bindings
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(kb, _)| kb);

        for kb in matching {
            let label = key_binding_display(kb);
            if label.is_empty() {
                continue;
            }
            let is_char = matches!(kb.code, KeyCode::Char(_));
            if is_char
                && char_label
                    .as_ref()
                    .is_none_or(|cur| label.len() < cur.len())
            {
                char_label = Some(label);
            } else if !is_char && named_label.is_none() {
                named_label = Some(label);
            }
        }

        char_label.or(named_label).unwrap_or_default()
    }

    /// Returns a combined display label for two related actions (e.g. "h/l"
    /// for PageLeft/PageRight).
    pub fn combined_label(&self, a1: &Action, a2: &Action) -> String {
        let l1 = self.label_for(a1);
        let l2 = self.label_for(a2);
        if l1.is_empty() && l2.is_empty() {
            String::new()
        } else if l1.is_empty() {
            l2
        } else if l2.is_empty() {
            l1
        } else {
            format!("{l1}/{l2}")
        }
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

/// Convert a `KeyBinding` to a human-readable display string.
fn key_binding_display(kb: &KeyBinding) -> String {
    let base = match kb.code {
        KeyCode::Char(' ') => "Space",
        KeyCode::Char(c) => {
            if kb.modifiers.contains(KeyModifiers::CONTROL) {
                return format!("C-{c}");
            }
            return c.to_string();
        }
        KeyCode::Up => "Up",
        KeyCode::Down => "Down",
        KeyCode::Left => "Left",
        KeyCode::Right => "Right",
        KeyCode::Enter => "Enter",
        KeyCode::Esc => "Esc",
        KeyCode::Tab => "Tab",
        KeyCode::Backspace => "Backspace",
        KeyCode::Delete => "Delete",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::F(n) => return format!("F{n}"),
        _ => return String::new(),
    };
    base.to_string()
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
            keymap.resolve(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
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
            scroll_up: Some(vec!["i".into()]),
            scroll_down: Some(vec!["s".into()]),
            quit: Some(vec!["C-c".into()]),
            ..KeymapOverrides::default()
        };

        let keymap = Keymap::from_config(overrides);

        // Custom bindings work
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE)),
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

    #[test]
    fn default_keymap_tab_workers() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)),
            Some(Action::SwitchTab(TabId::Workers)),
        );
    }

    #[test]
    fn default_keymap_create_ticket() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT)),
            Some(Action::CreateTicket),
        );
    }

    #[test]
    fn from_config_preserves_workers_tab_and_create_ticket() {
        let overrides = KeymapOverrides::default();
        let keymap = Keymap::from_config(overrides);

        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)),
            Some(Action::SwitchTab(TabId::Workers)),
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT)),
            Some(Action::CreateTicket),
        );
    }
}
