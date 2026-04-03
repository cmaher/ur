use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::context::TuiContext;
use crate::input::FooterCommand;

/// Sort footer commands following the project convention:
/// 1. Capital-letter shortcuts (Shift+key) in alphabetical order
/// 2. Lowercase-letter shortcuts in alphabetical order
/// 3. Non-letter keys (Tab, Enter, Ctrl+C, etc.)
///
/// Only non-common (left-side) commands are sorted this way.
/// Common (right-side) commands preserve their original order.
fn sort_commands(commands: &mut [FooterCommand]) {
    commands.sort_by(|a, b| {
        let key_a = classify_key(&a.key_label);
        let key_b = classify_key(&b.key_label);
        key_a.cmp(&key_b)
    });
}

/// Classification key for sorting: (category, sort_key).
/// Category 0 = uppercase letter, 1 = lowercase letter, 2 = non-letter.
fn classify_key(label: &str) -> (u8, String) {
    let first_char = label.chars().next();
    match first_char {
        Some(c) if c.is_ascii_uppercase() && label.len() == 1 => (0, label.to_lowercase()),
        Some(c) if c.is_ascii_lowercase() && label.len() == 1 => (1, label.to_string()),
        _ if label == "Space" => (2, label.to_lowercase()),
        _ => (3, label.to_lowercase()),
    }
}

/// Calculate the rendered width of a group of footer commands.
fn commands_width(commands: &[&FooterCommand]) -> u16 {
    if commands.is_empty() {
        return 0;
    }
    let mut w: u16 = 0;
    for (i, cmd) in commands.iter().enumerate() {
        if i > 0 {
            w += 2; // separator "  "
        }
        w += cmd.key_label.len() as u16 + 1 + cmd.description.len() as u16;
    }
    w
}

/// Build spans for a group of footer commands.
fn build_spans(
    commands: &[&FooterCommand],
    secondary_color: ratatui::style::Color,
    content_color: ratatui::style::Color,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, cmd) in commands.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            cmd.key_label.clone(),
            Style::default().fg(secondary_color),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            cmd.description.clone(),
            Style::default().fg(content_color),
        ));
    }
    spans
}

/// Render footer commands collected from the input stack.
///
/// Collects commands from the input stack's `footer_commands()`, sorts
/// non-common commands by the project convention (capitals first, then
/// lowercase, then non-letter keys), and renders them:
/// - Left side: page-specific commands (common=false)
/// - Far right: "?" Commands hint (always visible)
/// - Right side: remaining common/global commands, shown if they fit
pub fn render_footer(area: Rect, buf: &mut Buffer, ctx: &TuiContext, commands: &[FooterCommand]) {
    let theme = &ctx.theme;

    // Use same background as the table header (neutral bg / neutral_content fg).
    let bg_style = Style::default().bg(theme.neutral).fg(theme.neutral_content);
    buf.set_style(area, bg_style);

    // Split into left (non-common) and right (common) groups.
    let mut left_cmds: Vec<FooterCommand> =
        commands.iter().filter(|c| !c.common).cloned().collect();
    sort_commands(&mut left_cmds);
    let left_refs: Vec<&FooterCommand> = left_cmds.iter().collect();

    // Separate the "?" command from other common commands — it's always shown.
    let help_cmd: Vec<&FooterCommand> = commands
        .iter()
        .filter(|c| c.common && c.key_label == "?")
        .collect();
    let other_right_refs: Vec<&FooterCommand> = commands
        .iter()
        .filter(|c| c.common && c.key_label != "?")
        .collect();

    let left_width = commands_width(&left_refs);
    let help_width = commands_width(&help_cmd);
    let other_right_width = commands_width(&other_right_refs);

    // Always render left commands
    if !left_refs.is_empty() {
        let spans = build_spans(&left_refs, theme.primary_content, theme.neutral_content);
        let line = Line::from(spans);
        line.render(area, buf);
    }

    let gap = if left_refs.is_empty() { 0u16 } else { 2u16 };

    // Always render "?" right-aligned if it fits on its own
    if !help_cmd.is_empty() && left_width + gap + help_width <= area.width {
        // Check if the other common commands also fit (with separator before "?")
        let separator = if other_right_refs.is_empty() {
            0u16
        } else {
            2u16
        };
        let full_right_width = other_right_width + separator + help_width;
        if !other_right_refs.is_empty() && left_width + gap + full_right_width <= area.width {
            // All common commands fit — render them all together
            let mut all_right: Vec<&FooterCommand> = other_right_refs;
            all_right.extend(&help_cmd);
            let right_x = area.x + area.width - full_right_width;
            let right_area = Rect::new(right_x, area.y, full_right_width, 1);
            let spans = build_spans(&all_right, theme.primary_content, theme.neutral_content);
            let line = Line::from(spans);
            line.render(right_area, buf);
        } else {
            // Only "?" fits — render just the help hint
            let right_x = area.x + area.width - help_width;
            let right_area = Rect::new(right_x, area.y, help_width, 1);
            let spans = build_spans(&help_cmd, theme.primary_content, theme.neutral_content);
            let line = Line::from(spans);
            line.render(right_area, buf);
        }
    } else if !other_right_refs.is_empty() && left_width + gap + other_right_width <= area.width {
        // No "?" command but other common commands fit
        let right_x = area.x + area.width - other_right_width;
        let right_area = Rect::new(right_x, area.y, other_right_width, 1);
        let spans = build_spans(
            &other_right_refs,
            theme.primary_content,
            theme.neutral_content,
        );
        let line = Line::from(spans);
        line.render(right_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{GlobalHandler, InputHandler, InputStack};
    use crate::keymap::Keymap;
    use crate::theme::Theme;
    use ur_config::TuiConfig;

    fn make_ctx() -> TuiContext {
        let tui_config = TuiConfig::default();
        let theme = Theme::resolve(&tui_config);
        let keymap = Keymap::default();
        TuiContext {
            theme,
            keymap,
            projects: vec![],
            project_configs: std::collections::HashMap::new(),
            tui_config: TuiConfig::default(),
            config_dir: std::path::PathBuf::from("/tmp/test-urui"),
            project_filter: None,
        }
    }

    fn cmd(key: &str, desc: &str, common: bool) -> FooterCommand {
        FooterCommand {
            key_label: key.to_string(),
            description: desc.to_string(),
            common,
        }
    }

    #[test]
    fn render_footer_does_not_panic() {
        let ctx = make_ctx();
        let commands = vec![cmd("q", "Quit", false), cmd("Tab", "Switch", true)];
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_footer(area, &mut buf, &ctx, &commands);
    }

    #[test]
    fn render_footer_with_empty_commands() {
        let ctx = make_ctx();
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_footer(area, &mut buf, &ctx, &[]);
    }

    #[test]
    fn sort_commands_capitals_before_lowercase() {
        let mut cmds = vec![
            cmd("d", "Delete", false),
            cmd("C", "Create", false),
            cmd("a", "Action", false),
            cmd("A", "Archive", false),
        ];
        sort_commands(&mut cmds);
        assert_eq!(cmds[0].key_label, "A");
        assert_eq!(cmds[1].key_label, "C");
        assert_eq!(cmds[2].key_label, "a");
        assert_eq!(cmds[3].key_label, "d");
    }

    #[test]
    fn sort_commands_non_letter_keys_last() {
        let mut cmds = vec![
            cmd("Tab", "Switch", false),
            cmd("a", "Action", false),
            cmd("Enter", "Confirm", false),
            cmd("A", "Archive", false),
        ];
        sort_commands(&mut cmds);
        assert_eq!(cmds[0].key_label, "A");
        assert_eq!(cmds[1].key_label, "a");
        // Non-letter keys come last, sorted among themselves
        assert!(cmds[2].key_label == "Enter" || cmds[2].key_label == "Tab");
        assert!(cmds[3].key_label == "Enter" || cmds[3].key_label == "Tab");
    }

    #[test]
    fn footer_collects_from_input_stack() {
        let mut stack = InputStack::default();
        stack.push(Box::new(GlobalHandler));

        let commands = stack.footer_commands();
        // GlobalHandler provides Q, q, Tab, t/f/w, Esc, Settings - all common
        assert!(commands.len() >= 4);
        assert!(commands.iter().all(|c| c.common));
    }

    #[test]
    fn footer_collects_from_multiple_handlers() {
        use crate::input::InputResult;
        use crossterm::event::KeyEvent;

        struct PageHandler;
        impl InputHandler for PageHandler {
            fn handle_key(&self, _key: KeyEvent) -> InputResult {
                InputResult::Bubble
            }
            fn footer_commands(&self) -> Vec<FooterCommand> {
                vec![cmd("C", "Create", false), cmd("d", "Delete", false)]
            }
            fn name(&self) -> &str {
                "page"
            }
        }

        let mut stack = InputStack::default();
        stack.push(Box::new(GlobalHandler));
        stack.push(Box::new(PageHandler));

        let commands = stack.footer_commands();
        // Global (7 common: Q, q, Tab, t/f/w, Esc, Settings, Commands) + Page (2 non-common)
        assert_eq!(commands.len(), 9);

        let non_common: Vec<&FooterCommand> = commands.iter().filter(|c| !c.common).collect();
        assert_eq!(non_common.len(), 2);

        let common: Vec<&FooterCommand> = commands.iter().filter(|c| c.common).collect();
        assert_eq!(common.len(), 7);
    }

    #[test]
    fn footer_updates_when_handler_pushed_and_popped() {
        use crate::input::InputResult;
        use crossterm::event::KeyEvent;

        struct OverlayHandler;
        impl InputHandler for OverlayHandler {
            fn handle_key(&self, _key: KeyEvent) -> InputResult {
                InputResult::Bubble
            }
            fn footer_commands(&self) -> Vec<FooterCommand> {
                vec![cmd("Enter", "Confirm", false)]
            }
            fn name(&self) -> &str {
                "overlay"
            }
        }

        let mut stack = InputStack::default();
        stack.push(Box::new(GlobalHandler));

        let commands_before = stack.footer_commands();
        let count_before = commands_before.len();

        // Push overlay handler
        stack.push(Box::new(OverlayHandler));
        let commands_with_overlay = stack.footer_commands();
        assert_eq!(commands_with_overlay.len(), count_before + 1);

        // Pop overlay handler
        stack.pop();
        let commands_after = stack.footer_commands();
        assert_eq!(commands_after.len(), count_before);
    }

    #[test]
    fn classify_key_ordering() {
        // Uppercase single char = category 0
        assert_eq!(classify_key("A").0, 0);
        assert_eq!(classify_key("Z").0, 0);
        // Lowercase single char = category 1
        assert_eq!(classify_key("a").0, 1);
        assert_eq!(classify_key("z").0, 1);
        // Space = category 2 (before other non-letter keys)
        assert_eq!(classify_key("Space").0, 2);
        // Multi-char or non-letter = category 3
        assert_eq!(classify_key("Tab").0, 3);
        assert_eq!(classify_key("Ctrl+C").0, 3);
        assert_eq!(classify_key("Enter").0, 3);
    }

    #[test]
    fn commands_width_empty() {
        assert_eq!(commands_width(&[]), 0);
    }

    #[test]
    fn commands_width_single() {
        let c = cmd("q", "Quit", false);
        // "q" (1) + " " (1) + "Quit" (4) = 6
        assert_eq!(commands_width(&[&c]), 6);
    }

    #[test]
    fn commands_width_multiple() {
        let c1 = cmd("q", "Quit", false);
        let c2 = cmd("Tab", "Switch", false);
        // c1: 1+1+4 = 6, c2: 3+1+6 = 10, separator: 2
        assert_eq!(commands_width(&[&c1, &c2]), 18);
    }
}
