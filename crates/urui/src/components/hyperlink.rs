use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// Cached result of terminal hyperlink capability detection.
static SUPPORTS_OSC8: OnceLock<bool> = OnceLock::new();

/// Returns true when the active terminal advertises OSC 8 hyperlink support.
///
/// The result is cached after the first call. Override via `URUI_HYPERLINKS=on|off`.
///
/// Detection order:
/// 1. `URUI_HYPERLINKS=on` → true; `URUI_HYPERLINKS=off` → false.
/// 2. `TERM_PROGRAM` ∈ {`iTerm.app`, `WezTerm`, `vscode`, `ghostty`} → true.
/// 3. `TERM` contains `kitty`, `ghostty`, or `alacritty` → true.
/// 4. `VTE_VERSION` parses as u32 ≥ 5000 → true.
/// 5. Otherwise → false.
pub fn supports_osc8() -> bool {
    *SUPPORTS_OSC8.get_or_init(detect_osc8)
}

fn detect_osc8() -> bool {
    if let Ok(val) = std::env::var("URUI_HYPERLINKS") {
        match val.to_lowercase().as_str() {
            "on" => return true,
            "off" => return false,
            _ => {}
        }
    }

    if let Ok(term_program) = std::env::var("TERM_PROGRAM")
        && matches!(
            term_program.as_str(),
            "iTerm.app" | "WezTerm" | "vscode" | "ghostty"
        )
    {
        return true;
    }

    if let Ok(term) = std::env::var("TERM")
        && (term.contains("kitty") || term.contains("ghostty") || term.contains("alacritty"))
    {
        return true;
    }

    if let Ok(vte) = std::env::var("VTE_VERSION")
        && let Ok(v) = vte.parse::<u32>()
        && v >= 5000
    {
        return true;
    }

    false
}

/// Parse `https://github.com/{owner}/{repo}/pull/{N}` → `Some("owner/repo#N")`.
///
/// Returns `None` for any URL not matching this exact shape (issues, GitLab,
/// trailing junk after the PR number, etc.). Tolerates a trailing slash or
/// `?query` after the PR number.
pub fn format_pr_short(url: &str) -> Option<String> {
    let path = url.strip_prefix("https://github.com/")?;

    let mut parts = path.splitn(4, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let kind = parts.next()?;
    let rest = parts.next()?;

    if kind != "pull" {
        return None;
    }

    // Extract the PR number, tolerating a trailing slash or query string.
    let number_str = rest.split(['/', '?', '#']).next()?;

    if number_str.is_empty() {
        return None;
    }

    // Ensure the number is purely numeric.
    if !number_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    Some(format!("{}/{}#{}", owner, repo, number_str))
}

/// Render `display` into the cells of `rect` with OSC 8 wrapping pointing at `url`.
///
/// Truncates `display` to `rect.width` columns. If `!supports_osc8()`, writes plain
/// text without escape sequences. Cells beyond the display text are filled with spaces
/// using the same `fg`/`bg` style.
pub fn render_hyperlink(
    rect: Rect,
    buf: &mut Buffer,
    url: &str,
    display: &str,
    fg: Color,
    bg: Color,
) {
    let style = Style::default().fg(fg).bg(bg);
    let width = rect.width as usize;

    if width == 0 || rect.height == 0 {
        return;
    }

    // Collect display chars, truncated to rect.width.
    let chars: Vec<char> = display.chars().take(width).collect();
    let y = rect.y;

    if supports_osc8() {
        render_hyperlink_osc8(rect, buf, url, &chars, style, y);
    } else {
        render_plain(rect, buf, &chars, style, y);
    }
}

fn render_hyperlink_osc8(
    rect: Rect,
    buf: &mut Buffer,
    url: &str,
    chars: &[char],
    style: Style,
    y: u16,
) {
    let n = chars.len();

    for (i, x) in (rect.x..rect.x + rect.width).enumerate() {
        if i < n {
            let ch = chars[i];
            let symbol = if n == 1 {
                // Single visible cell: carries both open and close wrappers.
                format!("\x1b]8;;{url}\x1b\\{ch}\x1b]8;;\x1b\\")
            } else if i == 0 {
                // First cell: open wrapper before the visible char.
                format!("\x1b]8;;{url}\x1b\\{ch}")
            } else if i == n - 1 {
                // Last cell: close wrapper after the visible char.
                format!("{ch}\x1b]8;;\x1b\\")
            } else {
                // Middle cells: plain visible char.
                ch.to_string()
            };
            buf[(x, y)].set_symbol(&symbol).set_style(style);
        } else {
            // Pad remaining cells with spaces.
            buf[(x, y)].set_symbol(" ").set_style(style);
        }
    }
}

fn render_plain(rect: Rect, buf: &mut Buffer, chars: &[char], style: Style, y: u16) {
    let n = chars.len();
    for (i, x) in (rect.x..rect.x + rect.width).enumerate() {
        if i < n {
            buf[(x, y)]
                .set_symbol(&chars[i].to_string())
                .set_style(style);
        } else {
            buf[(x, y)].set_symbol(" ").set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    // ── format_pr_short ────────────────────────────────────────────────────────

    #[test]
    fn pr_short_happy_path() {
        assert_eq!(
            format_pr_short("https://github.com/paxos/ur/pull/324"),
            Some("paxos/ur#324".to_string())
        );
    }

    #[test]
    fn pr_short_trailing_slash() {
        assert_eq!(
            format_pr_short("https://github.com/paxos/ur/pull/324/"),
            Some("paxos/ur#324".to_string())
        );
    }

    #[test]
    fn pr_short_query_string() {
        assert_eq!(
            format_pr_short("https://github.com/paxos/ur/pull/324?foo=bar"),
            Some("paxos/ur#324".to_string())
        );
    }

    #[test]
    fn pr_short_issues_url_returns_none() {
        assert_eq!(
            format_pr_short("https://github.com/paxos/ur/issues/10"),
            None
        );
    }

    #[test]
    fn pr_short_gitlab_url_returns_none() {
        assert_eq!(
            format_pr_short("https://gitlab.com/paxos/ur/merge_requests/5"),
            None
        );
    }

    #[test]
    fn pr_short_no_pull_segment_returns_none() {
        assert_eq!(format_pr_short("https://github.com/paxos/ur"), None);
    }

    #[test]
    fn pr_short_non_numeric_number_returns_none() {
        assert_eq!(
            format_pr_short("https://github.com/paxos/ur/pull/abc"),
            None
        );
    }

    #[test]
    fn pr_short_empty_number_returns_none() {
        assert_eq!(format_pr_short("https://github.com/paxos/ur/pull/"), None);
    }

    // ── supports_osc8 environment overrides ───────────────────────────────────
    //
    // We cannot reliably test the OnceLock cache in a multi-test environment, so
    // we test the underlying detector function directly.

    #[test]
    fn detect_osc8_on_override() {
        // SAFETY: single-threaded test environment; no other threads read this var.
        unsafe { std::env::set_var("URUI_HYPERLINKS", "on") };
        let result = detect_osc8();
        // SAFETY: same as above.
        unsafe { std::env::remove_var("URUI_HYPERLINKS") };
        assert!(result);
    }

    #[test]
    fn detect_osc8_off_override() {
        // Remove any vars that would otherwise trigger detection.
        // SAFETY: single-threaded test environment.
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::remove_var("TERM");
            std::env::remove_var("VTE_VERSION");
            std::env::set_var("URUI_HYPERLINKS", "off");
        }
        let result = detect_osc8();
        // SAFETY: same as above.
        unsafe { std::env::remove_var("URUI_HYPERLINKS") };
        assert!(!result);
    }

    // ── render_hyperlink (OSC 8 off) ──────────────────────────────────────────

    fn make_buf(w: u16, h: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, w, h))
    }

    fn render_plain_test(
        buf: &mut Buffer,
        rect: Rect,
        _url: &str,
        display: &str,
        fg: Color,
        bg: Color,
    ) {
        // Force the plain-text path directly without relying on the OnceLock.
        let style = Style::default().fg(fg).bg(bg);
        let width = rect.width as usize;
        if width == 0 || rect.height == 0 {
            return;
        }
        let chars: Vec<char> = display.chars().take(width).collect();
        let y = rect.y;
        render_plain(rect, buf, &chars, style, y);
    }

    fn render_osc8_test(
        buf: &mut Buffer,
        rect: Rect,
        url: &str,
        display: &str,
        fg: Color,
        bg: Color,
    ) {
        let style = Style::default().fg(fg).bg(bg);
        let width = rect.width as usize;
        if width == 0 || rect.height == 0 {
            return;
        }
        let chars: Vec<char> = display.chars().take(width).collect();
        let y = rect.y;
        render_hyperlink_osc8(rect, buf, url, &chars, style, y);
    }

    #[test]
    fn plain_no_escape_chars() {
        let mut buf = make_buf(5, 1);
        let rect = Rect::new(0, 0, 5, 1);
        render_plain_test(
            &mut buf,
            rect,
            "http://example.com",
            "hello",
            Color::White,
            Color::Black,
        );
        for x in 0..5u16 {
            let sym = buf[(x, 0)].symbol();
            assert!(!sym.contains('\x1b'), "cell {x} contains ESC: {:?}", sym);
        }
        let text: String = (0..5u16)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert_eq!(text, "hello");
    }

    #[test]
    fn plain_truncates_display() {
        let mut buf = make_buf(3, 1);
        let rect = Rect::new(0, 0, 3, 1);
        render_plain_test(
            &mut buf,
            rect,
            "http://example.com",
            "hello",
            Color::White,
            Color::Black,
        );
        let text: String = (0..3u16)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert_eq!(text, "hel");
    }

    #[test]
    fn plain_empty_display_fills_spaces() {
        let mut buf = make_buf(4, 1);
        let rect = Rect::new(0, 0, 4, 1);
        render_plain_test(
            &mut buf,
            rect,
            "http://example.com",
            "",
            Color::White,
            Color::Black,
        );
        for x in 0..4u16 {
            assert_eq!(buf[(x, 0)].symbol(), " ");
        }
    }

    #[test]
    fn plain_fg_bg_applied() {
        let mut buf = make_buf(3, 1);
        let rect = Rect::new(0, 0, 3, 1);
        render_plain_test(
            &mut buf,
            rect,
            "http://example.com",
            "hi ",
            Color::Red,
            Color::Blue,
        );
        for x in 0..3u16 {
            let cell = &buf[(x, 0)];
            assert_eq!(cell.fg, Color::Red);
            assert_eq!(cell.bg, Color::Blue);
        }
    }

    // ── render_hyperlink_osc8 ─────────────────────────────────────────────────

    #[test]
    fn osc8_first_cell_starts_with_open_sequence() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(5, 1);
        let rect = Rect::new(0, 0, 5, 1);
        render_osc8_test(&mut buf, rect, url, "hello", Color::White, Color::Black);

        let first = buf[(0u16, 0u16)].symbol();
        let expected_open = format!("\x1b]8;;{url}\x1b\\");
        assert!(
            first.starts_with(&expected_open),
            "first cell should start with OSC open: {:?}",
            first
        );
    }

    #[test]
    fn osc8_last_cell_ends_with_close_sequence() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(5, 1);
        let rect = Rect::new(0, 0, 5, 1);
        render_osc8_test(&mut buf, rect, url, "hello", Color::White, Color::Black);

        let last = buf[(4u16, 0u16)].symbol();
        let expected_close = "\x1b]8;;\x1b\\";
        assert!(
            last.ends_with(expected_close),
            "last cell should end with OSC close: {:?}",
            last
        );
    }

    #[test]
    fn osc8_middle_cells_are_plain() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(5, 1);
        let rect = Rect::new(0, 0, 5, 1);
        render_osc8_test(&mut buf, rect, url, "hello", Color::White, Color::Black);

        // Cells 1..=3 are middle cells and must not contain ESC.
        for x in 1u16..=3 {
            let sym = buf[(x, 0)].symbol();
            assert!(
                !sym.contains('\x1b'),
                "middle cell {x} contains ESC: {:?}",
                sym
            );
        }
    }

    #[test]
    fn osc8_single_cell_has_both_wrappers() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(1, 1);
        let rect = Rect::new(0, 0, 1, 1);
        render_osc8_test(&mut buf, rect, url, "X", Color::White, Color::Black);

        let sym = buf[(0u16, 0u16)].symbol();
        let expected_open = format!("\x1b]8;;{url}\x1b\\");
        let expected_close = "\x1b]8;;\x1b\\";
        assert!(
            sym.starts_with(&expected_open),
            "single cell should start with open: {:?}",
            sym
        );
        assert!(
            sym.ends_with(expected_close),
            "single cell should end with close: {:?}",
            sym
        );
    }

    #[test]
    fn osc8_trailing_cells_are_spaces() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(6, 1);
        let rect = Rect::new(0, 0, 6, 1);
        // "hi" is 2 chars but rect is 6 wide → 4 trailing spaces.
        render_osc8_test(&mut buf, rect, url, "hi", Color::White, Color::Black);

        for x in 2u16..6 {
            assert_eq!(buf[(x, 0)].symbol(), " ", "cell {x} should be a space");
        }
    }

    #[test]
    fn osc8_truncates_to_rect_width() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(3, 1);
        let rect = Rect::new(0, 0, 3, 1);
        render_osc8_test(&mut buf, rect, url, "hello", Color::White, Color::Black);

        // Only 3 display chars should appear (h, e, l).
        // The 3rd char (index 2) is the last and should have the close wrapper.
        let last = buf[(2u16, 0u16)].symbol();
        assert!(
            last.ends_with("\x1b]8;;\x1b\\"),
            "truncated last cell should carry close: {:?}",
            last
        );
    }

    #[test]
    fn osc8_empty_display_fills_spaces() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(4, 1);
        let rect = Rect::new(0, 0, 4, 1);
        render_osc8_test(&mut buf, rect, url, "", Color::White, Color::Black);

        for x in 0..4u16 {
            assert_eq!(buf[(x, 0)].symbol(), " ");
        }
    }

    #[test]
    fn osc8_fg_bg_applied() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(3, 1);
        let rect = Rect::new(0, 0, 3, 1);
        render_osc8_test(&mut buf, rect, url, "hi ", Color::Red, Color::Blue);

        for x in 0..3u16 {
            let cell = &buf[(x, 0)];
            assert_eq!(cell.fg, Color::Red);
            assert_eq!(cell.bg, Color::Blue);
        }
    }

    #[test]
    fn zero_width_rect_does_not_panic() {
        let mut buf = make_buf(0, 0);
        let rect = Rect::new(0, 0, 0, 0);
        // Must not panic regardless of OSC8 mode.
        render_plain_test(
            &mut buf,
            rect,
            "http://example.com",
            "hello",
            Color::White,
            Color::Black,
        );
        render_osc8_test(
            &mut buf,
            rect,
            "http://example.com",
            "hello",
            Color::White,
            Color::Black,
        );
    }
}
