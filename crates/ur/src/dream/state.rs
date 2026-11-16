//! `<brain>/.dream/state.json`: cached ref checks and the durable-doc review log.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct State {
    #[serde(default)]
    pub refs: BTreeMap<String, RefState>,
    #[serde(default)]
    pub reviews: BTreeMap<String, ReviewRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    #[serde(default, skip_serializing_if = "HarvestState::is_empty")]
    pub harvest: HarvestState,
    /// Keys this version doesn't know, kept on rewrite.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// What `ur dream harvest --write` has already handed to a dream run.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct HarvestState {
    /// Transcript path, as [`session_key`] stores it → lines already digested.
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionCursor>,
    /// Memory files changed at or before this RFC 3339 instant were already listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_as_of: Option<String>,
}

impl HarvestState {
    fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.memory_as_of.is_none()
    }

    /// Rewrite every session key to its [`session_key`] form. A key stored by an older version
    /// (an absolute path) and its new form are the same session: the cursor that read further
    /// wins, so nothing is harvested twice.
    pub fn normalize_keys(&mut self, home: Option<&Path>) {
        let old = std::mem::take(&mut self.sessions);
        for (k, cursor) in old {
            let key = session_key(&k, home);
            let read_further = self.sessions.get(&key).is_none_or(|kept| {
                (cursor.lines, &cursor.harvested_at) > (kept.lines, &kept.harvested_at)
            });
            if read_further {
                self.sessions.insert(key, cursor);
            }
        }
    }
}

/// The `harvest.sessions` key for a transcript path, free of the home directory: `state.json`
/// is committed to the brain's repository, which may be shared.
///
/// A path under `home` becomes `~/rest`; other paths stay absolute. Claude Code names a
/// transcript directory after its working directory with every character that is not an ASCII
/// letter or digit replaced by `-` (`/Users/me/src/r` → `-Users-me-src-r`), so a path component
/// that is the encoded home, or starts with it followed by `-`, has that prefix replaced by `~`
/// (`-Users-me-src-r` → `~-src-r`). Stripping a fixed prefix keeps distinct directories distinct,
/// and a key already in this form maps to itself.
pub fn session_key(path: &str, home: Option<&Path>) -> String {
    let Some(home) = home.map(|h| h.to_string_lossy().trim_end_matches('/').to_string()) else {
        return path.to_string();
    };
    if home.is_empty() {
        return path.to_string();
    }
    let relative = if path == home {
        "~".to_string()
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
        format!("~/{rest}")
    } else {
        path.to_string()
    };
    let encoded = encode_dir_name(&home);
    relative
        .split('/')
        .map(|c| match c.strip_prefix(encoded.as_str()) {
            Some(rest) if rest.is_empty() || rest.starts_with('-') => format!("~{rest}"),
            _ => c.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A directory as Claude Code names its transcript directory.
fn encode_dir_name(dir: &str) -> String {
    dir.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionCursor {
    pub lines: usize,
    pub harvested_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RefState {
    pub state: String,
    /// `true` done, `false` not done, `null` unknown.
    pub done: Option<bool>,
    pub checked_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReviewRecord {
    pub last_reviewed: String,
    pub interval_days: i64,
    /// Most recent last.
    #[serde(default)]
    pub history: Vec<ReviewEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReviewEntry {
    pub date: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Repo name → full commit the doc was checked against (`reviewed --at`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub at: BTreeMap<String, String>,
}

impl ReviewRecord {
    /// Per repo, the commit from the most recent review that recorded one, with its date.
    pub fn latest_at(&self) -> BTreeMap<String, (String, String)> {
        let mut out = BTreeMap::new();
        for e in self.history.iter().rev() {
            for (repo, sha) in &e.at {
                out.entry(repo.clone())
                    .or_insert_with(|| (sha.clone(), e.date.clone()));
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The doc still matches reality.
    Unchanged,
    /// The review found drift and proposed changes.
    Changed,
}

pub fn path(brain: &Path) -> PathBuf {
    brain.join(".dream").join("state.json")
}

impl State {
    pub fn load(brain: &Path) -> Result<Self> {
        let p = path(brain);
        match std::fs::read_to_string(&p) {
            Ok(text) => {
                let mut st: Self =
                    serde_json::from_str(&text).with_context(|| format!("{}", p.display()))?;
                st.harvest.normalize_keys(dirs::home_dir().as_deref());
                Ok(st)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("{}", p.display())),
        }
    }

    pub fn save(&self, brain: &Path) -> Result<()> {
        let p = path(brain);
        std::fs::create_dir_all(p.parent().unwrap())?;
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        std::fs::write(&p, text).with_context(|| format!("{}", p.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/example";

    fn key(path: &str) -> String {
        session_key(path, Some(Path::new(HOME)))
    }

    fn cursor(lines: usize, at: &str) -> SessionCursor {
        SessionCursor {
            lines,
            harvested_at: at.into(),
        }
    }

    #[test]
    fn session_key_drops_the_home_directory() {
        // Under home, including the encoded home in a transcript directory name.
        assert_eq!(
            key("/Users/example/.claude/projects/-Users-example-src-app/s1.jsonl"),
            "~/.claude/projects/~-src-app/s1.jsonl"
        );
        assert_eq!(
            key("/Users/example/.claude/projects/-Users-example/s1.jsonl"),
            "~/.claude/projects/~/s1.jsonl"
        );
        // A dot-dir in the working directory encodes as `--`.
        assert_eq!(
            key("/Users/example/.claude/projects/-Users-example--ur-pool-1/s.jsonl"),
            "~/.claude/projects/~--ur-pool-1/s.jsonl"
        );
        assert_eq!(key("/Users/example"), "~");
        // Outside home: stays absolute, but an encoded home component is still replaced.
        assert_eq!(key("/opt/t/s.jsonl"), "/opt/t/s.jsonl");
        assert_eq!(key("/opt/t/-Users-example-x/s.jsonl"), "/opt/t/~-x/s.jsonl");
        // Only on a component boundary: another account whose name starts the same is untouched.
        assert_eq!(key("/Users/examples/a"), "/Users/examples/a");
        assert_eq!(
            key("/t/-Users-examples-x/s.jsonl"),
            "/t/-Users-examples-x/s.jsonl"
        );
        // Already stored in this form: unchanged.
        let done = "~/.claude/projects/~-src-app/s1.jsonl";
        assert_eq!(key(done), done);
        // No home directory known: unchanged.
        assert_eq!(session_key("/Users/example/a", None), "/Users/example/a");
        assert_eq!(
            session_key("/Users/example/a", Some(Path::new("/Users/example/"))),
            "~/a"
        );
    }

    #[test]
    fn same_session_file_in_two_directories_keeps_two_keys() {
        let a = key("/Users/example/.claude/projects/-Users-example-src-a/s.jsonl");
        let b = key("/Users/example/.claude/projects/-Users-example-src-b/s.jsonl");
        assert_ne!(a, b);
        let mut h = HarvestState::default();
        h.sessions.insert(
            "/Users/example/.claude/projects/-Users-example-src-a/s.jsonl".into(),
            cursor(3, "2026-01-01T00:00:00+00:00"),
        );
        h.sessions
            .insert(b.clone(), cursor(5, "2026-01-02T00:00:00+00:00"));
        h.normalize_keys(Some(Path::new(HOME)));
        assert_eq!(h.sessions.len(), 2);
        assert_eq!(h.sessions[&a].lines, 3);
        assert_eq!(h.sessions[&b].lines, 5);
    }

    #[test]
    fn old_absolute_key_matches_and_merges_with_its_new_form() {
        let old = "/Users/example/.claude/projects/-Users-example-src-app/s1.jsonl";
        let new = key(old);
        let mut h = HarvestState::default();
        h.sessions
            .insert(old.into(), cursor(7, "2026-01-02T00:00:00+00:00"));
        h.sessions
            .insert(new.clone(), cursor(4, "2026-01-01T00:00:00+00:00"));
        h.sessions.insert(
            "/opt/t/s.jsonl".into(),
            cursor(1, "2026-01-01T00:00:00+00:00"),
        );
        h.normalize_keys(Some(Path::new(HOME)));
        let keys: Vec<_> = h.sessions.keys().cloned().collect();
        assert_eq!(keys, vec!["/opt/t/s.jsonl".to_string(), new.clone()]);
        assert_eq!(
            h.sessions[&new].lines, 7,
            "the cursor that read further wins"
        );
    }

    #[test]
    fn load_rewrites_absolute_keys_and_save_persists_them() {
        let Some(home) = dirs::home_dir() else { return };
        let home = home.to_string_lossy().trim_end_matches('/').to_string();
        let enc = encode_dir_name(&home);
        let old = format!("{home}/.claude/projects/{enc}-src-example/s1.jsonl");
        let tmp = tempfile::tempdir().unwrap();
        let brain = tmp.path();
        std::fs::create_dir_all(brain.join(".dream")).unwrap();
        let text = serde_json::json!({
            "harvest": {"sessions": {&old: {"lines": 2, "harvested_at": "T"}}},
            "future": 1,
        });
        std::fs::write(path(brain), text.to_string()).unwrap();

        let st = State::load(brain).unwrap();
        let new = "~/.claude/projects/~-src-example/s1.jsonl";
        assert_eq!(st.harvest.sessions.keys().collect::<Vec<_>>(), vec![new]);
        st.save(brain).unwrap();
        let saved = std::fs::read_to_string(path(brain)).unwrap();
        assert!(!saved.contains(&home), "{saved}");
        assert!(!saved.contains(&enc), "{saved}");
        assert!(
            saved.contains(new) && saved.contains("\"future\""),
            "{saved}"
        );
    }

    #[test]
    fn old_state_without_at_loads_and_latest_at_prefers_newest() {
        let old = r#"{"reviews": {"r/a.md": {"last_reviewed": "2026-01-02", "interval_days": 30,
            "history": [{"date": "2026-01-01", "outcome": "unchanged"}]}}}"#;
        let st: State = serde_json::from_str(old).unwrap();
        let rec = &st.reviews["r/a.md"];
        assert!(rec.history[0].at.is_empty());
        assert!(rec.latest_at().is_empty());
        assert!(!serde_json::to_string(&st).unwrap().contains("\"at\""));

        let entry = |date: &str, at: &[(&str, &str)]| ReviewEntry {
            date: date.into(),
            outcome: Outcome::Unchanged,
            note: None,
            at: at
                .iter()
                .map(|(r, s)| (r.to_string(), s.to_string()))
                .collect(),
        };
        let rec = ReviewRecord {
            last_reviewed: "2026-03-01".into(),
            interval_days: 30,
            history: vec![
                entry("2026-01-01", &[("x", "old"), ("y", "y1")]),
                entry("2026-02-01", &[("x", "new")]),
                entry("2026-03-01", &[]),
            ],
        };
        let got = rec.latest_at();
        assert_eq!(got["x"], ("new".into(), "2026-02-01".into()));
        assert_eq!(got["y"], ("y1".into(), "2026-01-01".into()));
    }
}
