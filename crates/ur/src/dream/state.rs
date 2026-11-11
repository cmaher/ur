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
    /// Transcript path → lines already digested.
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
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("{}", p.display())),
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
