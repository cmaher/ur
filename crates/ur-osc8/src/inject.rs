//! Byte-stream OSC 8 URL injector with ANSI-escape awareness.

use regex::bytes::Regex;
use std::sync::OnceLock;

/// Maximum number of bytes we will hold in the pending URL buffer before
/// flushing it verbatim. This bounds memory use and guarantees forward
/// progress when the input looks URL-like forever.
pub const MAX_PENDING: usize = 4096;

/// Parser modes for the ANSI state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Regular text. URL scanning is active here.
    Text,
    /// Just saw `ESC` (0x1b), waiting for the next byte to disambiguate.
    Esc,
    /// Inside a CSI sequence (`ESC [ ... final`). Final byte is 0x40..=0x7E.
    Csi,
    /// Inside an OSC sequence (`ESC ] ...`). Terminated by `BEL` or `ESC \`.
    Osc,
    /// Inside an OSC sequence and just saw an `ESC` — waiting for `\` (ST).
    OscEsc,
}

/// State held across `push` calls so chunk boundaries do not break URL
/// detection or ANSI escape parsing.
#[derive(Debug, Clone)]
pub struct InjectorState {
    /// Current parser mode.
    mode: Mode,
    /// Buffer of pending text bytes that might be (part of) a URL. These
    /// bytes have not yet been emitted to `out`.
    pending: Vec<u8>,
    /// True while we are inside a hyperlink span opened by `ESC ] 8 ; ... ST`
    /// and before the matching `ESC ] 8 ; ; ST` close. URLs inside such a
    /// span must not be wrapped.
    in_osc8: bool,
    /// Accumulates the payload of the current OSC sequence (used to decide
    /// whether it is an OSC 8 open/close and to update `in_osc8`).
    osc_buf: Vec<u8>,
    /// Accumulates parameter/intermediate bytes of the current CSI sequence
    /// until the final byte arrives. Held (not emitted) so we can defer the
    /// wrap/don't-wrap decision for `pending` until we know what the CSI
    /// does.
    csi_params: Vec<u8>,
}

impl InjectorState {
    fn fresh() -> Self {
        InjectorState {
            mode: Mode::Text,
            pending: Vec::new(),
            in_osc8: false,
            osc_buf: Vec::new(),
            csi_params: Vec::new(),
        }
    }
}

/// Pure streaming OSC 8 URL injector.
///
/// Feed arbitrary byte chunks in with [`Injector::push`]; call
/// [`Injector::flush`] at end-of-stream (or whenever you want held trailing
/// bytes emitted verbatim).
#[derive(Debug, Clone)]
pub struct Injector {
    /// Parser state carried across chunks.
    state: InjectorState,
}

impl Default for Injector {
    fn default() -> Self {
        Injector::new()
    }
}

impl Injector {
    /// Create a fresh injector in the initial (plain-text) state.
    pub fn new() -> Self {
        Injector {
            state: InjectorState::fresh(),
        }
    }

    /// Push a chunk of input bytes through the injector, appending rewritten
    /// output to `out`.
    pub fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &b in input {
            self.step(b, out);
        }
    }

    /// Emit any held trailing bytes verbatim. After this call the pending
    /// buffer is empty. Does not change `in_osc8` / `mode`, so it is safe to
    /// continue pushing more bytes afterwards, though mid-escape flushes will
    /// lose no state — only the text-mode `pending` buffer is held.
    pub fn flush(&mut self, out: &mut Vec<u8>) {
        if !self.state.pending.is_empty() {
            emit_pending_as_text(&mut self.state, out);
        }
    }

    fn step(&mut self, b: u8, out: &mut Vec<u8>) {
        match self.state.mode {
            Mode::Text => step_text(&mut self.state, b, out),
            Mode::Esc => step_esc(&mut self.state, b, out),
            Mode::Csi => step_csi(&mut self.state, b, out),
            Mode::Osc => step_osc(&mut self.state, b, out),
            Mode::OscEsc => step_osc_esc(&mut self.state, b, out),
        }
    }
}

/// Regex used to locate URLs inside a committed text buffer.
///
/// We restrict to `http://` and `https://` URLs and a conservative trailing
/// character class so we do not grab adjacent punctuation. Trailing `.`, `,`,
/// `)`, `]`, `;`, `:`, `!`, `?`, `'`, `"` are not included so typical
/// sentence-ending punctuation is left outside the link.
fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"https?://[A-Za-z0-9\-._~:/?#\[\]@!$&'()*+,;=%]*[A-Za-z0-9\-_~/#@$&*+=%]")
            .expect("url regex compiles")
    })
}

/// Returns true if `b` is a byte that could appear inside our URL regex. Used
/// to decide whether to keep accumulating `pending` in text mode.
fn is_url_byte(b: u8) -> bool {
    matches!(b,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' |
        b'-' | b'.' | b'_' | b'~' | b':' | b'/' | b'?' | b'#' |
        b'[' | b']' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' |
        b'*' | b'+' | b',' | b';' | b'=' | b'%'
    )
}

/// Returns true if the bytes in `pending` contain or could still grow into a
/// URL match. We use this to decide whether the buffer is worth holding
/// across a chunk boundary vs. just emitting it as plain text immediately.
fn could_contain_url(pending: &[u8]) -> bool {
    // A URL starts with `http://` or `https://`. Look for any suffix of
    // pending that is a (possibly-incomplete) prefix of one of those.
    const PREFIXES: [&[u8]; 2] = [b"http://", b"https://"];
    for start in 0..pending.len() {
        let tail = &pending[start..];
        for p in &PREFIXES {
            let n = tail.len().min(p.len());
            if tail[..n] == p[..n] {
                return true;
            }
        }
    }
    false
}

/// Drive one byte in `Text` mode. URL scanning happens here.
fn step_text(state: &mut InjectorState, b: u8, out: &mut Vec<u8>) {
    if b == 0x1b {
        // Entering escape. Hold the pending text buffer — we cannot decide
        // whether to wrap its URL contents yet because a cursor-move CSI
        // could invalidate the accumulator. The decision is made when the
        // CSI/OSC finishes (see `finish_csi`).
        state.mode = Mode::Esc;
        return;
    }

    // Any non-URL byte forces a scan of the current pending buffer.
    if !is_url_byte(b) {
        // The non-URL byte itself is part of the text stream but is not a
        // URL character, so it can safely be appended after emitting any
        // wrapped URLs inside `pending`.
        state.pending.push(b);
        emit_pending_as_text(state, out);
        return;
    }

    state.pending.push(b);

    // Bound memory: if we grew past the limit and the buffer still could
    // contain a URL (or not), flush as plain bytes to make forward progress.
    if state.pending.len() > MAX_PENDING {
        flush_pending_verbatim(state, out);
        return;
    }

    // If nothing in `pending` could even become a URL, there is no reason to
    // hold it — emit as plain text.
    if !could_contain_url(&state.pending) {
        flush_pending_verbatim(state, out);
    }
}

/// Emit `pending` to `out`, wrapping any complete URL matches in OSC 8 if we
/// are not already inside an OSC 8 span. Clears `pending`.
fn emit_pending_as_text(state: &mut InjectorState, out: &mut Vec<u8>) {
    if state.pending.is_empty() {
        return;
    }

    if state.in_osc8 {
        // Already inside a hyperlink — do not wrap.
        out.extend_from_slice(&state.pending);
        state.pending.clear();
        return;
    }

    let buf = std::mem::take(&mut state.pending);
    let re = url_regex();
    let mut last_end = 0usize;
    for m in re.find_iter(&buf) {
        out.extend_from_slice(&buf[last_end..m.start()]);
        write_wrapped_url(out, m.as_bytes());
        last_end = m.end();
    }
    out.extend_from_slice(&buf[last_end..]);
}

/// Emit `pending` as plain bytes (no URL scanning). Used when we know the
/// buffer cannot contain a URL, or when bounded-buffer overflow forces us to
/// give up on holding more.
fn flush_pending_verbatim(state: &mut InjectorState, out: &mut Vec<u8>) {
    if state.pending.is_empty() {
        return;
    }
    out.extend_from_slice(&state.pending);
    state.pending.clear();
}

/// Write `url` wrapped in an OSC 8 hyperlink pair.
fn write_wrapped_url(out: &mut Vec<u8>, url: &[u8]) {
    // ESC ] 8 ; ; <url> ESC \
    out.extend_from_slice(b"\x1b]8;;");
    out.extend_from_slice(url);
    out.extend_from_slice(b"\x1b\\");
    // visible URL text
    out.extend_from_slice(url);
    // ESC ] 8 ; ; ESC \  (close)
    out.extend_from_slice(b"\x1b]8;;\x1b\\");
}

/// Drive one byte in `Esc` mode (just saw 0x1b).
fn step_esc(state: &mut InjectorState, b: u8, out: &mut Vec<u8>) {
    match b {
        b'[' => {
            // Entering CSI. Do NOT emit pending yet — we'll decide when the
            // CSI finishes whether it was a cursor-move (abort pending URL
            // accumulator) or benign (like SGR `m`, safe to wrap).
            state.mode = Mode::Csi;
        }
        b']' => {
            // Entering OSC. Flush pending as text (with URL wrapping) — OSC
            // does not relocate the cursor, so any URL that was in progress
            // is still valid to wrap.
            emit_pending_as_text(state, out);
            out.extend_from_slice(b"\x1b]");
            state.osc_buf.clear();
            state.mode = Mode::Osc;
        }
        _ => {
            // Some other two-byte escape. Most non-CSI/non-OSC escapes do
            // not relocate the cursor in a way that would invalidate a URL
            // in progress, but to stay on the safe side we flush pending
            // as text (wrapping any complete URLs) before emitting the
            // escape verbatim.
            emit_pending_as_text(state, out);
            out.push(0x1b);
            out.push(b);
            state.mode = Mode::Text;
        }
    }
}

/// Drive one byte inside a CSI sequence.
fn step_csi(state: &mut InjectorState, b: u8, out: &mut Vec<u8>) {
    // CSI final byte is 0x40..=0x7E.
    if (0x40..=0x7e).contains(&b) {
        // Finalize. The final byte determines how we treat the pending
        // URL accumulator:
        //  - SGR (`m`) is color / style only and does not move the cursor,
        //    so any URL in `pending` is still contiguous on the wire; wrap.
        //  - Anything else may move the cursor, clear lines, etc. — a URL
        //    that spans such a sequence is almost certainly a false merge
        //    across a screen redraw, so flush pending verbatim (no wrap).
        if b == b'm' {
            emit_pending_as_text(state, out);
        } else {
            flush_pending_verbatim(state, out);
        }
        out.extend_from_slice(b"\x1b[");
        out.extend_from_slice(&state.csi_params);
        out.push(b);
        state.csi_params.clear();
        state.mode = Mode::Text;
    } else {
        state.csi_params.push(b);
    }
}

/// Drive one byte inside an OSC payload.
fn step_osc(state: &mut InjectorState, b: u8, out: &mut Vec<u8>) {
    match b {
        0x07 => {
            // BEL terminator.
            out.push(b);
            finalize_osc(state);
            state.mode = Mode::Text;
        }
        0x1b => {
            // Possible ESC \ (ST) terminator. Do not emit yet — wait for the
            // next byte to decide.
            state.mode = Mode::OscEsc;
        }
        _ => {
            out.push(b);
            state.osc_buf.push(b);
        }
    }
}

/// Drive one byte after seeing `ESC` inside an OSC payload.
fn step_osc_esc(state: &mut InjectorState, b: u8, out: &mut Vec<u8>) {
    if b == b'\\' {
        // ESC \ is the ST terminator.
        out.extend_from_slice(b"\x1b\\");
        finalize_osc(state);
        state.mode = Mode::Text;
    } else {
        // Not a terminator — the ESC was literal data inside the OSC. Emit it
        // plus the next byte and stay in OSC mode.
        out.push(0x1b);
        out.push(b);
        state.osc_buf.push(0x1b);
        state.osc_buf.push(b);
        state.mode = Mode::Osc;
    }
}

/// Update `in_osc8` based on a just-terminated OSC payload. An OSC 8 payload
/// begins with `8;` — the URL is after the second `;`. An empty URL (the
/// close form `8;;`) ends the hyperlink span; any other OSC 8 opens one.
fn finalize_osc(state: &mut InjectorState) {
    let payload = std::mem::take(&mut state.osc_buf);
    // Must start with "8;" to be OSC 8.
    if payload.starts_with(b"8;") {
        // Find second ';' after the "8;".
        if let Some(second_semi) = payload[2..].iter().position(|&c| c == b';') {
            let url_start = 2 + second_semi + 1;
            let url = &payload[url_start..];
            state.in_osc8 = !url.is_empty();
        } else {
            // Malformed OSC 8 — treat like a close.
            state.in_osc8 = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> Vec<u8> {
        let mut inj = Injector::new();
        let mut out = Vec::new();
        for c in chunks {
            inj.push(c, &mut out);
        }
        inj.flush(&mut out);
        out
    }

    fn wrap(url: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        write_wrapped_url(&mut v, url);
        v
    }

    #[test]
    fn plain_text_passes_through_byte_identical() {
        let input = b"hello world, this is just text with punctuation!\n\tmore text.";
        let got = run(&[input]);
        assert_eq!(got, input);
    }

    #[test]
    fn no_url_no_changes_multibyte_boundaries() {
        let input = b"line one\nline two\nline three\n";
        // Single chunk
        assert_eq!(run(&[input]), input);
        // Split in several places
        assert_eq!(run(&[&input[..5], &input[5..13], &input[13..]]), input);
    }

    #[test]
    fn single_url_in_plain_text_is_wrapped() {
        let input = b"see https://example.com/foo for details";
        let got = run(&[input]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"see ");
        expected.extend_from_slice(&wrap(b"https://example.com/foo"));
        expected.extend_from_slice(b" for details");
        assert_eq!(got, expected);
    }

    #[test]
    fn http_url_wrapped() {
        let got = run(&[b"visit http://example.org/x\n"]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"visit ");
        expected.extend_from_slice(&wrap(b"http://example.org/x"));
        expected.extend_from_slice(b"\n");
        assert_eq!(got, expected);
    }

    #[test]
    fn url_split_across_two_push_calls_is_wrapped_once() {
        // Split in the middle of the host.
        let got = run(&[b"go to https://exam", b"ple.com/path and done"]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"go to ");
        expected.extend_from_slice(&wrap(b"https://example.com/path"));
        expected.extend_from_slice(b" and done");
        assert_eq!(got, expected);
    }

    #[test]
    fn url_split_inside_scheme_is_wrapped_once() {
        // Split in the middle of the scheme.
        let got = run(&[b"see htt", b"ps://example.com/ok here"]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"see ");
        expected.extend_from_slice(&wrap(b"https://example.com/ok"));
        expected.extend_from_slice(b" here");
        assert_eq!(got, expected);
    }

    #[test]
    fn url_bytes_inside_csi_are_not_wrapped() {
        // CSI sequence that contains the literal bytes of a URL-looking
        // string (contrived — CSI never actually contains these, but the
        // parser must not care what is inside).
        let input = b"red\x1b[31mhttps://nope.example/\x1b[0m tail";
        let got = run(&[input]);
        // The string inside CSI is only URL-looking if we interpret it as
        // text; but since we enter CSI at `\x1b[`, everything up to the
        // CSI final byte (`m`) is part of the CSI and must not be wrapped.
        // Here the CSI is `\x1b[31m`, so `https://nope.example/` is actually
        // in text mode. Build a test where the URL-looking bytes are
        // genuinely inside the CSI parameter bytes.
        //
        // Construct: ESC [ <url-ish> H   (H is a cursor-move CSI final)
        let _ = got; // discard, real assertion below.

        let input2 = b"\x1b[https://x.example/Hafter";
        let got2 = run(&[input2]);
        // All of `\x1b[https://x.example/H` must pass through verbatim
        // (unwrapped) because it's a CSI sequence. The CSI final is 'H'.
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x1b[https://x.example/H");
        expected.extend_from_slice(b"after");
        assert_eq!(got2, expected);
    }

    #[test]
    fn url_bytes_inside_osc_are_not_wrapped() {
        // OSC 0 (set window title) containing a URL-looking string, BEL
        // terminated.
        let input = b"\x1b]0;https://title.example/\x07after";
        let got = run(&[input]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x1b]0;https://title.example/\x07");
        expected.extend_from_slice(b"after");
        assert_eq!(got, expected);
    }

    #[test]
    fn url_already_in_osc8_is_not_double_wrapped() {
        // Open OSC 8 with URL, emit URL-looking body, close OSC 8.
        let mut input = Vec::new();
        input.extend_from_slice(b"prefix ");
        input.extend_from_slice(b"\x1b]8;;https://example.com/\x1b\\");
        input.extend_from_slice(b"https://example.com/"); // visible text
        input.extend_from_slice(b"\x1b]8;;\x1b\\");
        input.extend_from_slice(b" suffix");
        let got = run(&[&input]);
        assert_eq!(got, input);
    }

    #[test]
    fn url_already_in_osc8_bel_terminator_not_double_wrapped() {
        // Same but BEL-terminated OSC 8 forms.
        let mut input = Vec::new();
        input.extend_from_slice(b"\x1b]8;;https://example.com/\x07");
        input.extend_from_slice(b"https://example.com/");
        input.extend_from_slice(b"\x1b]8;;\x07");
        let got = run(&[&input]);
        assert_eq!(got, input);
    }

    #[test]
    fn csi_cursor_move_mid_url_aborts_accumulator() {
        // Start of a URL, then a cursor-home CSI before it can complete.
        // The partial URL bytes must pass through verbatim, and nothing
        // should be wrapped.
        let input = b"https://exam\x1b[Hrest";
        let got = run(&[input]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"https://exam");
        expected.extend_from_slice(b"\x1b[H");
        expected.extend_from_slice(b"rest");
        assert_eq!(got, expected);
    }

    #[test]
    fn csi_param_cursor_move_mid_url_aborts_accumulator() {
        let input = b"https://exam\x1b[10;20Hrest";
        let got = run(&[input]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"https://exam");
        expected.extend_from_slice(b"\x1b[10;20H");
        expected.extend_from_slice(b"rest");
        assert_eq!(got, expected);
    }

    #[test]
    fn pending_buffer_is_bounded_and_overflows_flush_verbatim() {
        // Craft an input that looks URL-ish enough to keep buffering but
        // never terminates. A long string of URL-safe bytes after
        // `https://` will keep pending growing.
        let mut input = Vec::new();
        input.extend_from_slice(b"https://");
        input.extend(std::iter::repeat_n(b'a', MAX_PENDING * 2));
        let got = run(&[&input]);
        // After overflow, bytes should be emitted verbatim — so the output
        // must contain the raw scheme+host somewhere without an OSC 8
        // wrapper. We check that the first MAX_PENDING+1 bytes of the raw
        // input appear verbatim in the output (ensuring the overflow path
        // took effect) and that there is no OSC 8 open byte sequence.
        assert!(
            !got.windows(5).any(|w| w == b"\x1b]8;;"),
            "overflow buffer must not be wrapped in OSC 8"
        );
        assert_eq!(got, input, "overflowed bytes must be emitted verbatim");
    }

    #[test]
    fn flush_emits_trailing_url_at_end_of_stream() {
        // The URL runs to EOF with no trailing delimiter. flush() must
        // still wrap it.
        let got = run(&[b"tail https://example.com/end"]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"tail ");
        expected.extend_from_slice(&wrap(b"https://example.com/end"));
        assert_eq!(got, expected);
    }

    #[test]
    fn trailing_punctuation_not_wrapped_in_link() {
        // A URL followed immediately by a period should wrap only the URL
        // portion, leaving the period outside the link.
        let got = run(&[b"go https://example.com/path. done"]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"go ");
        expected.extend_from_slice(&wrap(b"https://example.com/path"));
        expected.extend_from_slice(b". done");
        assert_eq!(got, expected);
    }

    #[test]
    fn multiple_urls_in_one_chunk() {
        let got = run(&[b"a https://x.example/1 b http://y.example/2 c"]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"a ");
        expected.extend_from_slice(&wrap(b"https://x.example/1"));
        expected.extend_from_slice(b" b ");
        expected.extend_from_slice(&wrap(b"http://y.example/2"));
        expected.extend_from_slice(b" c");
        assert_eq!(got, expected);
    }

    #[test]
    fn esc_split_across_push_is_handled() {
        // ESC at end of one chunk, `[` at start of next.
        let got = run(&[b"pre\x1b", b"[0mpost"]);
        assert_eq!(got, b"pre\x1b[0mpost");
    }

    #[test]
    fn osc_st_split_across_push() {
        // ESC inside OSC split from the `\`.
        let got = run(&[b"\x1b]0;title\x1b", b"\\tail"]);
        assert_eq!(got, b"\x1b]0;title\x1b\\tail");
    }

    #[test]
    fn osc8_state_persists_across_chunks() {
        // Open OSC 8 in one chunk, URL-looking body in another, close in a
        // third — nothing inside should be wrapped.
        let mut input = Vec::new();
        input.extend_from_slice(b"\x1b]8;;https://example.com/\x1b\\");
        input.extend_from_slice(b"click me https://inside.example/ ok");
        input.extend_from_slice(b"\x1b]8;;\x1b\\");
        let got = run(&[
            b"\x1b]8;;https://example.com/\x1b\\",
            b"click me https://inside.example/ ok",
            b"\x1b]8;;\x1b\\",
        ]);
        assert_eq!(got, input);
    }

    #[test]
    fn literal_esc_inside_osc_payload_is_preserved() {
        // ESC followed by non-'\' inside an OSC should be treated as literal
        // data (though unusual) and the OSC continues until BEL.
        let input = b"\x1b]0;a\x1bX b\x07tail";
        let got = run(&[input]);
        assert_eq!(got, input);
    }
}
