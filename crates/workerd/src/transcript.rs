//! Agent session-log reader for `ReadTranscript`.
//!
//! Each agent writes its own JSONL session log; this module finds the live
//! session's file ([`ur_config::AgentType::transcript_location`]) and
//! normalizes its entries into [`TranscriptTurn`]s so callers never care
//! which agent produced them. Lines that do not parse (an agent mid-write) are
//! skipped; a missing or empty log is an error.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use ur_config::{AgentType, TranscriptLocation};
use ur_rpc::proto::core::{TranscriptToolCall, TranscriptTurn};

/// Longest tool-call summary, in characters.
const SUMMARY_MAX_CHARS: usize = 120;

/// Codex rollout files inspected for a matching cwd before giving up.
const CODEX_MAX_CANDIDATES: usize = 200;

/// Input keys whose value best summarizes a tool call, in priority order.
const SUMMARY_KEYS: &[&str] = &[
    "command",
    "cmd",
    "file_path",
    "path",
    "pattern",
    "url",
    "query",
    "description",
    "prompt",
];

/// Codex injects these as `user` messages; they are context, not turns.
const CODEX_CONTEXT_PREFIXES: &[&str] = &[
    "<environment_context>",
    "<user_instructions>",
    "# AGENTS.md instructions",
];

#[derive(Debug, PartialEq, Eq)]
pub enum TranscriptError {
    /// The agent has no session-log format to read.
    Unsupported {
        agent: &'static str,
    },
    /// No session log exists for this session.
    NotFound {
        reason: String,
    },
    /// The log exists but holds no conversation turns.
    Empty {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        reason: String,
    },
}

impl std::fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { agent } => write!(
                f,
                "agent '{agent}' has no readable session log; use `ur worker read --screen`"
            ),
            Self::NotFound { reason } => write!(f, "no session log found: {reason}"),
            Self::Empty { path } => {
                write!(f, "session log {} has no turns yet", path.display())
            }
            Self::Io { path, reason } => write!(f, "read {}: {reason}", path.display()),
        }
    }
}

/// The last turns of the live session and the log they came from.
#[derive(Debug)]
pub struct Transcript {
    pub turns: Vec<TranscriptTurn>,
    pub source_path: PathBuf,
}

/// Find the live session log for `agent` running in `cwd` and return its last
/// `turns` turns. Logs last written before `not_before` (when this worker's
/// session started) are ignored: in a reused pool slot they belong to an
/// earlier worker. Blocking file IO; run it on a blocking thread.
pub fn read_transcript(
    agent: AgentType,
    home: &Path,
    cwd: &Path,
    not_before: SystemTime,
    turns: usize,
) -> Result<Transcript, TranscriptError> {
    let location = agent
        .transcript_location(home, cwd)
        .ok_or(TranscriptError::Unsupported {
            agent: agent.name(),
        })?;
    let (path, parse): (PathBuf, fn(&Value) -> Option<Entry>) = match &location {
        TranscriptLocation::ClaudeProjectDir { dir } => {
            (newest_log(dir, false, None, not_before)?, claude_entry)
        }
        TranscriptLocation::CodexSessions { root, cwd } => {
            (newest_log(root, true, Some(cwd), not_before)?, codex_entry)
        }
    };
    let content = std::fs::read_to_string(&path).map_err(|e| TranscriptError::Io {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    let all = merge_turns(
        content
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|v| parse(&v)),
    );
    if all.is_empty() {
        return Err(TranscriptError::Empty { path });
    }
    let skip = all.len().saturating_sub(turns);
    Ok(Transcript {
        turns: all.into_iter().skip(skip).collect(),
        source_path: path,
    })
}

/// One log entry reduced to what a turn needs.
#[derive(Debug, PartialEq)]
struct Entry {
    role: &'static str,
    text: String,
    tool_calls: Vec<TranscriptToolCall>,
}

/// Fold consecutive same-role entries into one turn each.
fn merge_turns(entries: impl Iterator<Item = Entry>) -> Vec<TranscriptTurn> {
    let mut turns: Vec<TranscriptTurn> = Vec::new();
    for entry in entries {
        match turns.last_mut() {
            Some(last) if last.role == entry.role => append_entry(last, entry),
            _ => turns.push(TranscriptTurn {
                role: entry.role.to_owned(),
                text: entry.text,
                tool_calls: entry.tool_calls,
            }),
        }
    }
    turns
}

/// Add a same-role entry to the turn it continues.
fn append_entry(turn: &mut TranscriptTurn, entry: Entry) {
    if !entry.text.is_empty() {
        if !turn.text.is_empty() {
            turn.text.push_str("\n\n");
        }
        turn.text.push_str(&entry.text);
    }
    turn.tool_calls.extend(entry.tool_calls);
}

/// Claude Code entry: `{"type":"user"|"assistant","message":{"content":...}}`.
/// Sidechain (subagent) and meta entries are skipped, and so are user entries
/// that only carry tool results, so tool round-trips stay inside one
/// assistant turn.
fn claude_entry(v: &Value) -> Option<Entry> {
    let role = match v.get("type")?.as_str()? {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true)
        || v.get("isMeta").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let content = v.get("message")?.get("content")?;
    let mut entry = Entry {
        role,
        text: String::new(),
        tool_calls: Vec::new(),
    };
    match content {
        Value::String(s) => entry.text = s.trim().to_owned(),
        Value::Array(blocks) => claude_blocks(blocks, &mut entry),
        _ => return None,
    }
    (!entry.text.is_empty() || !entry.tool_calls.is_empty()).then_some(entry)
}

/// Collect text and tool calls from a Claude content-block array.
fn claude_blocks(blocks: &[Value], entry: &mut Entry) {
    let mut texts = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => texts.extend(block.get("text").and_then(Value::as_str).map(str::trim)),
            Some("tool_use") => entry.tool_calls.push(TranscriptToolCall {
                name: str_field(block, "name"),
                summary: block.get("input").map(summarize_input).unwrap_or_default(),
            }),
            _ => {}
        }
    }
    entry.text = texts.join("\n");
}

/// Codex rollout entry: `{"type":"response_item","payload":{...}}` carrying a
/// message, or a tool call (`function_call`, `custom_tool_call`,
/// `local_shell_call`). Developer messages and injected context are skipped.
fn codex_entry(v: &Value) -> Option<Entry> {
    if v.get("type")?.as_str()? != "response_item" {
        return None;
    }
    let payload = v.get("payload")?;
    let tool = |name: String, summary: String| Entry {
        role: "assistant",
        text: String::new(),
        tool_calls: vec![TranscriptToolCall { name, summary }],
    };
    match payload.get("type")?.as_str()? {
        "message" => codex_message(payload),
        "function_call" => {
            let args = payload.get("arguments").and_then(Value::as_str)?;
            let summary = serde_json::from_str::<Value>(args)
                .map(|a| summarize_input(&a))
                .unwrap_or_else(|_| truncate(args));
            Some(tool(str_field(payload, "name"), summary))
        }
        "custom_tool_call" => Some(tool(
            str_field(payload, "name"),
            truncate(&str_field(payload, "input")),
        )),
        "local_shell_call" => Some(tool(
            "shell".to_owned(),
            payload
                .get("action")
                .map(summarize_input)
                .unwrap_or_default(),
        )),
        _ => None,
    }
}

fn codex_message(payload: &Value) -> Option<Entry> {
    let role = match payload.get("role")?.as_str()? {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .map(str::trim)
        .filter(|t| !CODEX_CONTEXT_PREFIXES.iter().any(|p| t.starts_with(p)))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(Entry {
        role,
        text,
        tool_calls: Vec::new(),
    })
}

/// One-line summary of a tool call's input: the first well-known field
/// (command, path, ...) when present, else the compact JSON.
fn summarize_input(input: &Value) -> String {
    let value = SUMMARY_KEYS
        .iter()
        .find_map(|k| input.get(*k))
        .unwrap_or(input);
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Array(items) if items.iter().all(Value::is_string) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    };
    truncate(&text)
}

/// First line of `text`, capped at [`SUMMARY_MAX_CHARS`].
fn truncate(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= SUMMARY_MAX_CHARS {
        return line.to_owned();
    }
    let mut out: String = line.chars().take(SUMMARY_MAX_CHARS - 1).collect();
    out.push('…');
    out
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The newest-mtime `*.jsonl` in `dir` (also in subdirectories when
/// `recursive`) written at or after `not_before`, and, when `cwd` is given,
/// whose `session_meta` names it (among the newest [`CODEX_MAX_CANDIDATES`]).
fn newest_log(
    dir: &Path,
    recursive: bool,
    cwd: Option<&Path>,
    not_before: SystemTime,
) -> Result<PathBuf, TranscriptError> {
    let mut files = Vec::new();
    collect_jsonl(dir, recursive, &mut files);
    files.retain(|(t, _)| *t >= not_before);
    files.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    files
        .into_iter()
        .take(CODEX_MAX_CANDIDATES)
        .map(|(_, p)| p)
        .find(|p| cwd.is_none_or(|cwd| codex_session_cwd(p).as_deref() == Some(cwd)))
        .ok_or_else(|| TranscriptError::NotFound {
            reason: format!(
                "no *.jsonl session log under {}{} written since this worker started",
                dir.display(),
                cwd.map(|c| format!(" for cwd {}", c.display()))
                    .unwrap_or_default()
            ),
        })
}

/// Every `*.jsonl` in `dir` (and below it when `recursive`) with its mtime.
/// Unreadable directories contribute nothing.
fn collect_jsonl(dir: &Path, recursive: bool, out: &mut Vec<(SystemTime, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path.is_dir() {
            if recursive {
                collect_jsonl(&path, recursive, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "jsonl")
            && let Some(t) = mtime(&path)
        {
            out.push((t, path));
        }
    }
}

/// The `payload.cwd` of a rollout's first (`session_meta`) line.
fn codex_session_cwd(path: &Path) -> Option<PathBuf> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufReader::new(file).read_line(&mut first).ok()?;
    let v: Value = serde_json::from_str(&first).ok()?;
    if v.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    Some(PathBuf::from(v.get("payload")?.get("cwd")?.as_str()?))
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
