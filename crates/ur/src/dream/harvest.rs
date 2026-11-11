//! `ur dream harvest`: the mechanical half of the Harvest pass.
//!
//! Digests agent transcripts (Claude Code `<session>.jsonl`) and lists changed agent memory
//! files from the directories `[harvest]` in `DREAM.md` names, so a dream run can pull what was
//! learned in conversations into the brain. Only human prompts and assistant prose are kept: tool
//! calls, tool output, thinking, subagent turns and system reminders are dropped.
//!
//! A file is only considered once it has been idle for `idle_minutes` (the `as_of` cutoff), so
//! live sessions, including the dream run's own, are left for a later run. Every call records
//! in `state.json` how far each returned session was read and the memory cutoff, so the next call
//! returns only what is new. An interrupted run that never commits can discard the uncommitted
//! `state.json` to have the same content returned again.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use super::config::{Config, Harvest, expand};
use super::state::{SessionCursor, State};

static REMINDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<system-reminder>.*?</system-reminder>").unwrap());

#[derive(Debug, Serialize)]
pub struct HarvestReport {
    /// Files changed after this RFC 3339 instant are left for a later run.
    pub as_of: String,
    /// Sessions with new messages, oldest first, at most `max_sessions`.
    pub sessions: Vec<Session>,
    /// Further sessions with unread lines, left for a later run.
    pub remaining: usize,
    /// Sessions whose new lines held no messages; only their cursor moves.
    pub skipped_without_messages: usize,
    /// Memory files changed since the previous harvest.
    pub memory: Vec<MemoryFile>,
    /// How many directories each configured glob matched.
    pub globs: Vec<GlobMatch>,
    /// Files or directories that could not be read.
    pub errors: Vec<ReadError>,
    #[serde(skip)]
    cursors: Vec<(String, usize)>,
}

#[derive(Debug, Serialize)]
pub struct Session {
    pub file: String,
    pub session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub modified: String,
    /// First unread line (1-based) and last line read.
    pub from_line: usize,
    pub through_line: usize,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Serialize)]
pub struct Message {
    pub line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    pub role: Role,
    pub text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// A changed memory file with its text: the path is a host path a worker cannot read.
#[derive(Debug, Serialize)]
pub struct MemoryFile {
    pub path: String,
    pub modified: String,
    pub text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct GlobMatch {
    pub glob: String,
    pub kind: &'static str,
    pub dirs: usize,
}

#[derive(Debug, Serialize)]
pub struct ReadError {
    pub path: String,
    pub error: String,
}

/// A transcript with unread lines, before digesting.
struct Candidate {
    path: PathBuf,
    modified: DateTime<Utc>,
    from_line: usize,
}

/// The cutoff: now minus the idle window.
pub fn default_as_of(cfg: &Config) -> DateTime<Utc> {
    Utc::now() - chrono::Duration::minutes(cfg.raw.harvest.idle_minutes)
}

fn parse_as_of(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

pub fn harvest(brain: &Path, cfg: &Config, st: &State, as_of: DateTime<Utc>) -> HarvestReport {
    let h = &cfg.raw.harvest;
    let since = h
        .since
        .as_deref()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let mut errors = Vec::new();
    let mut globs = Vec::new();
    let transcript_dirs = expand_globs(brain, &h.transcripts, "transcripts", &mut globs);
    let memory_dirs = expand_globs(brain, &h.memory, "memory", &mut globs);

    let candidates = candidates(&transcript_dirs, st, as_of, since, &mut errors);
    let mut sessions = Vec::new();
    let mut cursors = Vec::new();
    let mut remaining = 0;
    let mut skipped_without_messages = 0;
    for c in candidates {
        if sessions.len() == h.max_sessions {
            remaining += 1;
            continue;
        }
        match digest(&c, h) {
            Ok(s) => {
                cursors.push((key(&c.path), s.through_line));
                if s.messages.is_empty() {
                    skipped_without_messages += 1;
                } else {
                    sessions.push(s);
                }
            }
            Err(e) => errors.push(ReadError {
                path: key(&c.path),
                error: format!("{e:#}"),
            }),
        }
    }
    let memory_after = st.harvest.memory_as_of.as_deref().and_then(parse_as_of);
    let window = Window {
        after: memory_after,
        as_of,
        since,
    };
    let memory = memory_files(&memory_dirs, &window, h.max_message_chars, &mut errors);
    HarvestReport {
        as_of: as_of.to_rfc3339(),
        sessions,
        remaining,
        skipped_without_messages,
        memory,
        globs,
        errors,
        cursors,
    }
}

/// Record what `report` returned: each digested session's cursor and the memory cutoff.
pub fn record(st: &mut State, report: &HarvestReport) {
    for (path, lines) in &report.cursors {
        st.harvest.sessions.insert(
            path.clone(),
            SessionCursor {
                lines: *lines,
                harvested_at: report.as_of.clone(),
            },
        );
    }
    st.harvest.memory_as_of = Some(report.as_of.clone());
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn expand_globs(
    brain: &Path,
    patterns: &[String],
    kind: &'static str,
    out: &mut Vec<GlobMatch>,
) -> Vec<PathBuf> {
    let mut dirs = BTreeSet::new();
    for g in patterns {
        let full = expand(brain, g);
        let matched: Vec<PathBuf> = glob::glob(&full.to_string_lossy())
            .map(|paths| paths.flatten().filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
        out.push(GlobMatch {
            glob: g.clone(),
            kind,
            dirs: matched.len(),
        });
        dirs.extend(matched);
    }
    dirs.into_iter().collect()
}

fn modified(path: &Path) -> std::io::Result<DateTime<Utc>> {
    Ok(DateTime::<Utc>::from(std::fs::metadata(path)?.modified()?))
}

fn on_or_after(t: DateTime<Utc>, since: Option<NaiveDate>) -> bool {
    since.is_none_or(|d| t.with_timezone(&Local).date_naive() >= d)
}

/// Transcripts idle since `as_of` that changed since their cursor was written, oldest first.
fn candidates(
    dirs: &[PathBuf],
    st: &State,
    as_of: DateTime<Utc>,
    since: Option<NaiveDate>,
    errors: &mut Vec<ReadError>,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                errors.push(ReadError {
                    path: key(dir),
                    error: e.to_string(),
                });
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "jsonl") || !path.is_file() {
                continue;
            }
            let Ok(m) = modified(&path) else { continue };
            if m > as_of || !on_or_after(m, since) {
                continue;
            }
            let cursor = st.harvest.sessions.get(&key(&path));
            let unchanged = cursor
                .and_then(|c| parse_as_of(&c.harvested_at))
                .is_some_and(|at| m <= at);
            if unchanged {
                continue;
            }
            out.push(Candidate {
                path,
                modified: m,
                from_line: cursor.map_or(1, |c| c.lines + 1),
            });
        }
    }
    out.sort_by(|a, b| (a.modified, &a.path).cmp(&(b.modified, &b.path)));
    out
}

fn digest(c: &Candidate, h: &Harvest) -> Result<Session> {
    let file = std::fs::File::open(&c.path).with_context(|| key(&c.path))?;
    let mut session = Session {
        file: key(&c.path),
        session: c
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        title: None,
        cwd: None,
        modified: c.modified.to_rfc3339(),
        from_line: c.from_line,
        through_line: c.from_line.saturating_sub(1),
        messages: Vec::new(),
    };
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| key(&c.path))?;
        let n = i + 1;
        session.through_line = n;
        if n < c.from_line {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        absorb(&mut session, n, &v);
    }
    for m in &mut session.messages {
        (m.text, m.truncated) = cut(std::mem::take(&mut m.text), h.max_message_chars);
    }
    Ok(session)
}

/// `text` cut to `max` characters, and whether it was cut.
fn cut(text: String, max: usize) -> (String, bool) {
    if text.chars().count() > max {
        (text.chars().take(max).collect(), true)
    } else {
        (text, false)
    }
}

/// Fold one transcript entry into the session.
fn absorb(session: &mut Session, line: usize, v: &Value) {
    let ty = v["type"].as_str().unwrap_or("");
    if ty == "ai-title"
        && let Some(t) = v["aiTitle"].as_str()
    {
        session.title = Some(t.to_string());
        return;
    }
    if session.cwd.is_none()
        && let Some(cwd) = v["cwd"].as_str()
    {
        session.cwd = Some(cwd.to_string());
    }
    let role = match ty {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return,
    };
    if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
        return;
    }
    if role == Role::User && !is_human(v) {
        return;
    }
    let Some(text) = text_of(&v["message"]["content"]) else {
        return;
    };
    let at = v["timestamp"].as_str().map(str::to_string);
    match session.messages.last_mut() {
        // One assistant turn is spread over several entries (one per content block).
        Some(last) if role == Role::Assistant && last.role == Role::Assistant => {
            last.text.push_str("\n\n");
            last.text.push_str(&text);
        }
        _ => session.messages.push(Message {
            line,
            at,
            role,
            text,
            truncated: false,
        }),
    }
}

/// A user entry a person typed: `origin.kind == "human"` when recorded; older transcripts have
/// no `origin`, so fall back to excluding harness-generated prompts by their markers.
fn is_human(v: &Value) -> bool {
    if let Some(kind) = v["origin"]["kind"].as_str() {
        return kind == "human";
    }
    let content = &v["message"]["content"];
    let first = content
        .as_str()
        .or_else(|| content.as_array()?.first()?["text"].as_str())
        .unwrap_or("");
    let t = first.trim_start();
    ![
        "<task-notification",
        "<local-command",
        "<command-",
        "Caveat:",
    ]
    .iter()
    .any(|p| t.starts_with(p))
}

/// Prose in a message: a string, or its `text` blocks. Tool use, tool results and thinking are
/// dropped, as are `<system-reminder>` blocks. `None` when nothing is left.
fn text_of(content: &Value) -> Option<String> {
    let raw = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => return None,
    };
    let text = REMINDER_RE.replace_all(&raw, "").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Which memory-file changes to list: after the last recorded cutoff, up to `as_of`, and on or
/// after `since`.
struct Window {
    after: Option<DateTime<Utc>>,
    as_of: DateTime<Utc>,
    since: Option<NaiveDate>,
}

impl Window {
    fn contains(&self, m: DateTime<Utc>) -> bool {
        m <= self.as_of && self.after.is_none_or(|a| m > a) && on_or_after(m, self.since)
    }
}

fn memory_files(
    dirs: &[PathBuf],
    window: &Window,
    max_chars: usize,
    errors: &mut Vec<ReadError>,
) -> Vec<MemoryFile> {
    let mut out = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        if let Err(e) = walk(dir, &mut files) {
            errors.push(ReadError {
                path: key(dir),
                error: e.to_string(),
            });
        }
        for f in files {
            let Ok(m) = modified(&f) else { continue };
            if !window.contains(m) {
                continue;
            }
            let raw = match std::fs::read(&f) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(e) => {
                    errors.push(ReadError {
                        path: key(&f),
                        error: e.to_string(),
                    });
                    continue;
                }
            };
            let (text, truncated) = cut(raw, max_chars);
            out.push(MemoryFile {
                path: key(&f),
                modified: m.to_rfc3339(),
                text,
                truncated,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Every regular file under `dir`, not following symlinked directories.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        let Ok(ty) = entry.file_type() else { continue };
        if ty.is_dir() {
            walk(&path, out)?;
        } else if ty.is_file() && entry.file_name() != ".DS_Store" {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn set_mtime(path: &Path, secs_ago: u64) {
        let t = SystemTime::now() - Duration::from_secs(secs_ago);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn cfg(extra: &str) -> Config {
        Config::parse(&format!(
            "```toml\n[harvest]\ntranscripts = [\"t/*\"]\nmemory = [\"mem\"]\n{extra}```\n"
        ))
        .unwrap()
    }

    const HUMAN: &str = r#"{"type":"user","origin":{"kind":"human"},"cwd":"/w","timestamp":"T1","message":{"content":"why is X slow?<system-reminder>noise</system-reminder>"}}"#;
    const TOOL_RESULT: &str =
        r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"big output"}]}}"#;
    const NOTIFY: &str = r#"{"type":"user","origin":{"kind":"task-notification"},"message":{"content":"<task-notification>x</task-notification>"}}"#;
    const THINK: &str =
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#;
    const SAY1: &str = r#"{"type":"assistant","timestamp":"T2","message":{"content":[{"type":"text","text":"Because Y."}]}}"#;
    const TOOL_USE: &str =
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#;
    const SAY2: &str =
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed."}]}}"#;
    const SIDE: &str = r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"sub"}]}}"#;
    const TITLE: &str = r#"{"type":"ai-title","aiTitle":"Slow X"}"#;
    const OLD_HUMAN: &str =
        r#"{"type":"user","message":{"content":[{"type":"text","text":"and Z?"}]}}"#;

    #[test]
    fn digests_human_and_assistant_prose_only() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path();
        std::fs::create_dir_all(b.join("t/a")).unwrap();
        let f = b.join("t/a/s1.jsonl");
        let lines = [
            HUMAN,
            TOOL_RESULT,
            NOTIFY,
            THINK,
            SAY1,
            TOOL_USE,
            SAY2,
            SIDE,
            TITLE,
            OLD_HUMAN,
            "not json",
        ];
        std::fs::write(&f, lines.join("\n")).unwrap();
        set_mtime(&f, 7200);

        let cfg = cfg("max_message_chars = 9\n");
        let r = harvest(b, &cfg, &State::default(), default_as_of(&cfg));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.sessions.len(), 1);
        let s = &r.sessions[0];
        assert_eq!(s.session, "s1");
        assert_eq!(s.title.as_deref(), Some("Slow X"));
        assert_eq!(s.cwd.as_deref(), Some("/w"));
        assert_eq!((s.from_line, s.through_line), (1, 11));
        let got: Vec<_> = s
            .messages
            .iter()
            .map(|m| (m.line, m.role, m.text.as_str(), m.truncated))
            .collect();
        assert_eq!(
            got,
            vec![
                (1, Role::User, "why is X ", true),
                (5, Role::Assistant, "Because Y", true),
                (10, Role::User, "and Z?", false),
            ]
        );
    }

    #[test]
    fn cursor_live_files_since_and_max_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path();
        std::fs::create_dir_all(b.join("t/a")).unwrap();
        std::fs::create_dir_all(b.join("mem")).unwrap();
        let mk = |name: &str, secs_ago: u64| {
            let p = b.join("t/a").join(name);
            std::fs::write(&p, format!("{HUMAN}\n{SAY1}\n")).unwrap();
            set_mtime(&p, secs_ago);
            p
        };
        let old = mk("old.jsonl", 9000);
        mk("mid.jsonl", 8000);
        mk("live.jsonl", 10);
        mk("ancient.jsonl", 60 * 60 * 24 * 400);
        std::fs::write(b.join("t/a/notes.txt"), "x").unwrap();
        std::fs::write(b.join("mem/fact.md"), "x").unwrap();
        set_mtime(&b.join("mem/fact.md"), 5000);

        let since = (Local::now() - chrono::Duration::days(30)).date_naive();
        let cfg = cfg(&format!("max_sessions = 1\nsince = \"{since}\"\n"));
        let mut st = State::default();
        let as_of = default_as_of(&cfg);
        let r = harvest(b, &cfg, &st, as_of);
        assert_eq!(r.sessions.len(), 1);
        assert_eq!(r.sessions[0].session, "old");
        assert_eq!(r.remaining, 1, "mid left; live and ancient excluded");
        assert_eq!(r.memory.len(), 1);
        assert_eq!(r.memory[0].text, "x");
        assert_eq!(r.globs.len(), 2);
        record(&mut st, &r);
        assert_eq!(st.harvest.sessions[&key(&old)].lines, 2);

        // Same cutoff again: old is done, mid comes up; memory already listed.
        let r = harvest(b, &cfg, &st, as_of);
        assert_eq!(r.sessions[0].session, "mid");
        assert_eq!(r.remaining, 0);
        assert!(r.memory.is_empty());

        // old grows after it was harvested: only the new lines come back.
        std::fs::write(&old, format!("{HUMAN}\n{SAY1}\n{OLD_HUMAN}\n")).unwrap();
        set_mtime(&old, 2000); // after the first cutoff (3600s ago), before `later`
        let later = Utc::now() - chrono::Duration::minutes(30);
        let mut st2 = st;
        st2.harvest
            .sessions
            .insert(key(&b.join("t/a/mid.jsonl")), cursor(2, &later));
        let r = harvest(b, &cfg, &st2, later);
        assert_eq!(r.sessions.len(), 1);
        let s = &r.sessions[0];
        assert_eq!(
            (s.session.as_str(), s.from_line, s.through_line),
            ("old", 3, 3)
        );
        assert_eq!(s.messages[0].text, "and Z?");
    }

    fn cursor(lines: usize, at: &DateTime<Utc>) -> SessionCursor {
        SessionCursor {
            lines,
            harvested_at: at.to_rfc3339(),
        }
    }

    #[test]
    fn config_rejects_bad_harvest_values() {
        let bad =
            |body: &str| Config::parse(&format!("```toml\n[harvest]\n{body}\n```\n")).is_err();
        assert!(bad("since = \"last week\""));
        assert!(bad("max_sessions = 0"));
        assert!(bad("transcripts = [\"[\"]"));
        assert!(bad("transcript = []"));
        assert!(!bad("since = \"2026-01-01\"\nmemory = [\"~/m\"]"));
    }

    #[test]
    fn as_of_round_trips() {
        let t = Utc::now();
        assert_eq!(parse_as_of(&t.to_rfc3339()), Some(t));
        assert_eq!(parse_as_of("yesterday"), None);
    }
}
