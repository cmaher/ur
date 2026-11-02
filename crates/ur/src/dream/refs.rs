//! `ur dream refs`: scratch streams, their refs, and when they last changed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, Local, NaiveDate};
use serde::Serialize;

use super::config::{Config, RefKind, Role, fill};
use super::files::{self, join};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Ref {
    pub kind: String,
    pub id: String,
    pub groups: BTreeMap<String, String>,
    /// Set when the value matched none of the kind's patterns (or the kind is unknown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

impl Ref {
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }
}

#[derive(Debug, Serialize)]
pub struct Stream {
    pub entry: Option<String>,
    pub frontmatter: bool,
    pub status: Option<String>,
    pub recorded_refs: Vec<Ref>,
    pub discovered_refs: Vec<Ref>,
    pub unclassified: Vec<String>,
    pub last_change: Option<String>,
    /// `git`, `uncommitted`, `mtime` (no history beyond `brain: init`), or `empty`.
    pub last_change_source: &'static str,
    pub quiet_days: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RefsReport {
    pub streams: BTreeMap<String, Stream>,
    /// Refs of active streams (status `active` or no frontmatter): what `check` checks.
    pub active_refs: Vec<Ref>,
}

fn make_ref(kind: &RefKind, groups: BTreeMap<String, String>, text: &str) -> Ref {
    let id = kind
        .id
        .as_deref()
        .and_then(|t| fill(t, &groups))
        .unwrap_or_else(|| text.to_string());
    Ref {
        kind: kind.kind.clone(),
        id,
        groups,
        problem: None,
    }
}

fn groups_of(pat: &super::config::Pattern, caps: &regex::Captures) -> BTreeMap<String, String> {
    let mut g = pat.set.clone();
    for name in pat.regex.capture_names().flatten() {
        if let Some(m) = caps.name(name) {
            g.insert(name.to_string(), m.as_str().to_string());
        }
    }
    g
}

/// Refs in `text`: kinds and patterns in config order; a match overlapping an earlier one is
/// skipped, so a specific pattern listed first wins over a looser one listed later.
pub fn match_refs(cfg: &Config, text: &str) -> (Vec<Ref>, BTreeSet<String>) {
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let overlaps =
        |taken: &[(usize, usize)], a: usize, b: usize| taken.iter().any(|&(x, y)| a < y && x < b);
    let mut found = Vec::new();
    let all = cfg
        .kinds
        .iter()
        .flat_map(|k| k.patterns.iter().map(move |p| (k, p)))
        .flat_map(|(k, p)| p.regex.captures_iter(text).map(move |c| (k, p, c)));
    for (kind, pat, caps) in all {
        let m = caps.get(0).unwrap();
        if overlaps(&taken, m.start(), m.end()) {
            continue;
        }
        taken.push((m.start(), m.end()));
        found.push(make_ref(kind, groups_of(pat, &caps), m.as_str()));
    }
    let mut unclassified = BTreeSet::new();
    for m in cfg.unclassified.iter().flat_map(|re| re.find_iter(text)) {
        if !overlaps(&taken, m.start(), m.end()) && !cfg.ignore_unclassified(m.as_str()) {
            unclassified.insert(m.as_str().to_string());
        }
    }
    (found, unclassified)
}

/// Parse a recorded or given value as a ref of `kind_name`.
pub fn parse_ref(cfg: &Config, kind_name: &str, value: &str) -> Ref {
    let unparsed = |problem: &str| Ref {
        kind: kind_name.to_string(),
        id: value.to_string(),
        groups: BTreeMap::new(),
        problem: Some(problem.to_string()),
    };
    let Some(kind) = cfg.kind(kind_name) else {
        return unparsed("unknown kind");
    };
    for pat in &kind.patterns {
        if let Some(caps) = pat.regex.captures(value) {
            let text = caps.get(0).unwrap().as_str().to_string();
            return make_ref(kind, groups_of(pat, &caps), &text);
        }
    }
    unparsed("matches no pattern")
}

/// Immediate children of scratch folders: one stream each (a folder or a single `.md`).
fn streams(brain: &Path, cfg: &Config) -> Vec<String> {
    let mut out = Vec::new();
    for folder in cfg.folders(Role::Scratch) {
        let Ok(entries) = std::fs::read_dir(brain.join(&folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_stream = entry.path().is_dir() || name.ends_with(".md");
            if is_stream && name != "CLAUDE.md" && !name.starts_with('.') {
                out.push(join(&folder, &name));
            }
        }
    }
    out.sort();
    out
}

/// The stream's entry file: itself, its `README.md`, or its only `.md` file.
fn entry_file(brain: &Path, rel: &str) -> Option<String> {
    if brain.join(rel).is_file() {
        return Some(rel.to_string());
    }
    let readme = join(rel, "README.md");
    if brain.join(&readme).is_file() {
        return Some(readme);
    }
    let mds: Vec<String> = std::fs::read_dir(brain.join(rel))
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md"))
        .collect();
    (mds.len() == 1).then(|| join(rel, &mds[0]))
}

pub(super) fn last_change(
    brain: &Path,
    rel: &str,
    init_commits: &BTreeSet<String>,
) -> (Option<NaiveDate>, &'static str) {
    if files::git(brain, &["status", "--porcelain", "--", rel]).is_some_and(|s| !s.is_empty()) {
        return (Some(Local::now().date_naive()), "uncommitted");
    }
    let log = files::git(brain, &["log", "--format=%H %cs", "--", rel]).unwrap_or_default();
    let commits: Vec<(&str, &str)> = log.lines().filter_map(|l| l.split_once(' ')).collect();
    let real = commits.iter().any(|(h, _)| !init_commits.contains(*h));
    if let (true, Some((_, date))) = (real, commits.first()) {
        return (NaiveDate::parse_from_str(date, "%Y-%m-%d").ok(), "git");
    }
    let newest = files::files_under(brain, rel)
        .iter()
        .filter_map(|p| p.metadata().and_then(|m| m.modified()).ok())
        .max();
    match newest {
        Some(t) => (Some(DateTime::<Local>::from(t).date_naive()), "mtime"),
        None => (None, "empty"),
    }
}

fn discover(
    brain: &Path,
    cfg: &Config,
    rel: &str,
    skip: &BTreeSet<String>,
) -> (Vec<Ref>, BTreeSet<String>) {
    let mut seen = skip.clone();
    let mut found = Vec::new();
    let mut unclassified = BTreeSet::new();
    let mut texts: Vec<String> = files::files_under(brain, rel)
        .iter()
        .map(|p| files::rel(brain, p))
        .filter(|r| files::is_text(r))
        .collect();
    texts.sort();
    for t in texts {
        let (refs, unc) = match_refs(cfg, &files::read(brain, &t));
        unclassified.extend(unc);
        found.extend(refs.into_iter().filter(|r| seen.insert(r.key())));
    }
    (found, unclassified)
}

/// Hashes of `brain: init` commits: history before them is not in git.
pub(super) fn init_commits(brain: &Path) -> BTreeSet<String> {
    files::git(brain, &["log", "--format=%H", "--grep=^brain: init$"])
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn scan(brain: &Path, cfg: &Config) -> RefsReport {
    let today = Local::now().date_naive();
    let init_commits = init_commits(brain);
    let mut streams_out = BTreeMap::new();
    let mut active: BTreeMap<String, Ref> = BTreeMap::new();
    for rel in streams(brain, cfg) {
        let entry = entry_file(brain, &rel);
        let fm = entry
            .as_deref()
            .and_then(|e| files::frontmatter(&files::read(brain, e)));
        let recorded: Vec<Ref> = fm
            .iter()
            .flat_map(|f| f.refs.iter())
            .map(|(k, v)| parse_ref(cfg, k, v))
            .collect();
        let skip = recorded.iter().map(Ref::key).collect();
        let (discovered, unclassified) = if cfg.raw.discovery.allowed {
            discover(brain, cfg, &rel, &skip)
        } else {
            (Vec::new(), BTreeSet::new())
        };
        let status = fm.as_ref().and_then(|f| f.fields.get("status").cloned());
        if matches!(status.as_deref(), None | Some("active")) {
            for r in recorded.iter().chain(&discovered) {
                active.insert(r.key(), r.clone());
            }
        }
        let (changed, source) = last_change(brain, &rel, &init_commits);
        streams_out.insert(
            rel,
            Stream {
                entry,
                frontmatter: fm.is_some(),
                status,
                recorded_refs: recorded,
                discovered_refs: discovered,
                unclassified: unclassified.into_iter().collect(),
                last_change: changed.map(|d| d.to_string()),
                last_change_source: source,
                quiet_days: changed.map(|d| (today - d).num_days()),
            },
        );
    }
    RefsReport {
        streams: streams_out,
        active_refs: active.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dream::config::tests::SAMPLE;

    #[test]
    fn match_refs_canonicalizes_and_prefers_earlier_patterns() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let text = "EXAMPLE-7 and example-org/api#12, PR #3, ISO-9001, OTHER-4";
        let (refs, unc) = match_refs(&cfg, text);
        let ids: Vec<_> = refs.iter().map(Ref::key).collect();
        assert_eq!(
            ids,
            vec!["ticket:EXAMPLE-7", "pr:api#12", "pr:example-repo#3"]
        );
        assert_eq!(unc.into_iter().collect::<Vec<_>>(), vec!["OTHER-4"]);
    }

    #[test]
    fn parse_ref_reads_recorded_values() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let r = parse_ref(&cfg, "pr", "example-org/web#45");
        assert_eq!(
            (r.id.as_str(), r.groups["repo"].as_str()),
            ("web#45", "web")
        );
        assert_eq!(
            parse_ref(&cfg, "pr", "nope").problem.as_deref(),
            Some("matches no pattern")
        );
        assert_eq!(
            parse_ref(&cfg, "branch", "x").problem.as_deref(),
            Some("unknown kind")
        );
    }
}
