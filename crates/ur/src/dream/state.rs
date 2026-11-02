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
    /// Keys this version doesn't know, kept on rewrite.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
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
