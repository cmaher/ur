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

    if n > 0 {
        // Cell 0: the full OSC 8-wrapped display text in a single symbol.
        // The terminal renders it as a clickable hyperlink spanning n visible columns.
        // Ratatui sees this cell as "2-wide" in the diff and auto-skips cell 1,
        // so we explicitly mark cells 1..n-1 as skip=true to keep them out of
        // the diff (their visual content comes from cell 0's symbol).
        let display: String = chars.iter().collect();
        let symbol = format!("\x1b]8;;{url}\x07{display}\x1b]8;;\x07");
        buf[(rect.x, y)].set_symbol(&symbol).set_style(style);

        for x in (rect.x + 1)..(rect.x + n as u16) {
            buf[(x, y)].set_symbol("").set_style(style);
            buf[(x, y)].skip = true;
        }
    }

    // Trailing cells: plain spaces so background/style renders correctly.
    for x in (rect.x + n as u16)..(rect.x + rect.width) {
        buf[(x, y)].set_symbol(" ").set_style(style);
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
    use std::sync::Mutex;

    // Env-var tests mutate process-global state; serialize to prevent races.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("URUI_HYPERLINKS", "on") };
        let result = detect_osc8();
        unsafe { std::env::remove_var("URUI_HYPERLINKS") };
        assert!(result);
    }

    #[test]
    fn detect_osc8_off_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::remove_var("TERM");
            std::env::remove_var("VTE_VERSION");
            std::env::set_var("URUI_HYPERLINKS", "off");
        }
        let result = detect_osc8();
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
    fn osc8_cell0_has_full_display_in_symbol() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(8, 1);
        let rect = Rect::new(0, 0, 8, 1);
        render_osc8_test(&mut buf, rect, url, "hello", Color::White, Color::Black);

        let expected = format!("\x1b]8;;{url}\x07hello\x1b]8;;\x07");
        assert_eq!(
            buf[(0u16, 0u16)].symbol(),
            expected,
            "cell 0 should contain full OSC 8-wrapped display text"
        );
    }

    #[test]
    fn osc8_inner_cells_are_skip() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(8, 1);
        let rect = Rect::new(0, 0, 8, 1);
        render_osc8_test(&mut buf, rect, url, "hello", Color::White, Color::Black);

        for x in 1u16..5 {
            assert!(buf[(x, 0)].skip, "cell {x} should be skip=true");
        }
    }

    #[test]
    fn osc8_trailing_cells_are_spaces_not_skip() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(8, 1);
        let rect = Rect::new(0, 0, 8, 1);
        // "hello" = 5 chars → cells 5..7 are trailing spaces.
        render_osc8_test(&mut buf, rect, url, "hello", Color::White, Color::Black);

        for x in 5u16..8 {
            assert_eq!(buf[(x, 0)].symbol(), " ", "cell {x} should be a space");
            assert!(!buf[(x, 0)].skip, "cell {x} should not be skip");
        }
    }

    #[test]
    fn osc8_single_char_no_skip_cells() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(4, 1);
        let rect = Rect::new(0, 0, 4, 1);
        render_osc8_test(&mut buf, rect, url, "X", Color::White, Color::Black);

        let expected = format!("\x1b]8;;{url}\x07X\x1b]8;;\x07");
        assert_eq!(buf[(0u16, 0u16)].symbol(), expected);
        // No skip cells since n=1 means cells 1..n-1 is empty range.
        for x in 1u16..4 {
            assert!(!buf[(x, 0)].skip, "cell {x} should not be skip");
            assert_eq!(
                buf[(x, 0)].symbol(),
                " ",
                "cell {x} should be trailing space"
            );
        }
    }

    #[test]
    fn osc8_truncates_to_rect_width() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(3, 1);
        let rect = Rect::new(0, 0, 3, 1);
        render_osc8_test(&mut buf, rect, url, "hello", Color::White, Color::Black);

        // Truncated to "hel" (3 chars) — cell 0 gets full wrapped symbol.
        let expected = format!("\x1b]8;;{url}\x07hel\x1b]8;;\x07");
        assert_eq!(
            buf[(0u16, 0u16)].symbol(),
            expected,
            "cell 0 truncated symbol"
        );
        // Cells 1..2 are skip, no trailing spaces.
        assert!(buf[(1u16, 0u16)].skip);
        assert!(buf[(2u16, 0u16)].skip);
    }

    #[test]
    fn osc8_uses_bel_not_st() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(5, 1);
        let rect = Rect::new(0, 0, 5, 1);
        render_osc8_test(&mut buf, rect, url, "abc", Color::White, Color::Black);

        let sym = buf[(0u16, 0u16)].symbol();
        assert!(
            !sym.contains("\x1b\\"),
            "should not use ESC\\ ST: {:?}",
            sym
        );
        assert!(sym.contains('\x07'), "should use BEL terminator: {:?}", sym);
    }

    #[test]
    fn osc8_cell0_style_applied() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(6, 1);
        let rect = Rect::new(0, 0, 6, 1);
        render_osc8_test(&mut buf, rect, url, "test", Color::Green, Color::Magenta);

        let cell = &buf[(0u16, 0u16)];
        assert_eq!(cell.fg, Color::Green);
        assert_eq!(cell.bg, Color::Magenta);
    }

    #[test]
    fn osc8_trailing_cells_carry_style() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(8, 1);
        let rect = Rect::new(0, 0, 8, 1);
        render_osc8_test(&mut buf, rect, url, "test", Color::Green, Color::Magenta);

        // "test" = 4 chars; cells 4..7 are trailing spaces.
        for x in 4u16..8 {
            assert_eq!(buf[(x, 0)].fg, Color::Green, "cell {x} fg");
            assert_eq!(buf[(x, 0)].bg, Color::Magenta, "cell {x} bg");
        }
    }

    #[test]
    fn osc8_empty_display_fills_spaces() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(4, 1);
        let rect = Rect::new(0, 0, 4, 1);
        render_osc8_test(&mut buf, rect, url, "", Color::White, Color::Black);

        for x in 0..4u16 {
            assert_eq!(buf[(x, 0)].symbol(), " ");
            assert!(!buf[(x, 0)].skip);
        }
    }

    #[test]
    fn osc8_diff_trailing_spaces_in_output() {
        // Regression: trailing spaces (beyond display text) must appear in
        // buffer diff so style changes (e.g. row selection) propagate correctly.
        let url = "https://github.com/paxos/ur/pull/1";
        let area = Rect::new(0, 0, 10, 1);
        let prev = Buffer::empty(area);
        let mut next = Buffer::empty(area);
        let rect = Rect::new(0, 0, 10, 1);
        render_osc8_test(&mut next, rect, url, "hello", Color::White, Color::Black);

        let diff = prev.diff(&next);
        let xs: Vec<u16> = diff.iter().map(|(x, _, _)| *x).collect();
        // Cell 0 in diff; cells 1..4 excluded (skip=true); cells 5..9 in diff.
        assert!(xs.contains(&0u16));
        for x in 1u16..5 {
            assert!(!xs.contains(&x), "skip cell {x} must not be in diff");
        }
        for x in 5u16..10 {
            assert!(xs.contains(&x), "trailing space cell {x} must be in diff");
        }
    }

    #[test]
    fn osc8_fg_bg_applied() {
        let url = "https://github.com/paxos/ur/pull/1";
        let mut buf = make_buf(3, 1);
        let rect = Rect::new(0, 0, 3, 1);
        render_osc8_test(&mut buf, rect, url, "hi", Color::Red, Color::Blue);

        assert_eq!(buf[(0u16, 0u16)].fg, Color::Red);
        assert_eq!(buf[(0u16, 0u16)].bg, Color::Blue);
        // Cell 1 is skip=true, cell 2 is trailing space — both carry style.
        for x in 1u16..3 {
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
