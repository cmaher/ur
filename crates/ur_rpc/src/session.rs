//! Client-side helpers for the session-control RPCs, shared by `ur worker`
//! and `ur-workertools worker`.

use crate::proto::core::{TranscriptTurn, WorkerKey};

/// Parse a key name (case-insensitive) into the allowlisted key.
pub fn parse_key(name: &str) -> Result<WorkerKey, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "escape" | "esc" => WorkerKey::Escape,
        "ctrl-c" | "c-c" | "ctrlc" => WorkerKey::CtrlC,
        "enter" | "return" => WorkerKey::Enter,
        "up" => WorkerKey::Up,
        "down" => WorkerKey::Down,
        "left" => WorkerKey::Left,
        "right" => WorkerKey::Right,
        "tab" => WorkerKey::Tab,
        "0" => WorkerKey::Digit0,
        "1" => WorkerKey::Digit1,
        "2" => WorkerKey::Digit2,
        "3" => WorkerKey::Digit3,
        "4" => WorkerKey::Digit4,
        "5" => WorkerKey::Digit5,
        "6" => WorkerKey::Digit6,
        "7" => WorkerKey::Digit7,
        "8" => WorkerKey::Digit8,
        "9" => WorkerKey::Digit9,
        _ => {
            return Err(format!(
                "invalid key '{name}': expected one of escape, ctrl-c, enter, up, down, left, \
                 right, tab, 0-9"
            ));
        }
    })
}

/// Plain-text rendering: a `[role]` header per turn, its text, then one
/// `  -> tool: summary` line per tool call.
pub fn format_turns(turns: &[TranscriptTurn]) -> String {
    let mut out = String::new();
    for turn in turns {
        out.push_str(&format!("[{}]\n", turn.role));
        if !turn.text.is_empty() {
            out.push_str(&turn.text);
            out.push('\n');
        }
        for call in &turn.tool_calls {
            out.push_str(&format!("  -> {}: {}\n", call.name, call.summary));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::core::TranscriptToolCall;

    #[test]
    fn parse_key_accepts_allowlist_only() {
        assert_eq!(parse_key("Esc").unwrap(), WorkerKey::Escape);
        assert_eq!(parse_key("escape").unwrap(), WorkerKey::Escape);
        assert_eq!(parse_key("ctrl-c").unwrap(), WorkerKey::CtrlC);
        assert_eq!(parse_key("C-c").unwrap(), WorkerKey::CtrlC);
        assert_eq!(parse_key("return").unwrap(), WorkerKey::Enter);
        assert_eq!(parse_key("Tab").unwrap(), WorkerKey::Tab);
        assert_eq!(parse_key("7").unwrap(), WorkerKey::Digit7);
        for bad in ["C-x", "Enter; rm", "unspecified", "12"] {
            assert!(parse_key(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn format_turns_lists_tool_calls() {
        let text = format_turns(&[TranscriptTurn {
            role: "assistant".into(),
            text: "Done.".into(),
            tool_calls: vec![TranscriptToolCall {
                name: "Bash".into(),
                summary: "cargo test".into(),
            }],
        }]);
        assert_eq!(text, "[assistant]\nDone.\n  -> Bash: cargo test\n\n");
    }
}
