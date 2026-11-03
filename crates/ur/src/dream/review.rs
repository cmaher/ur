//! `ur dream review` / `ur dream reviewed`: durable-doc review scheduling.
//!
//! Each durable doc has its own review interval, stored in `state.json`. A review that finds the
//! doc still accurate doubles the interval; one that finds drift halves it (clamped to the
//! config's `[min_days, max_days]`). Docs that never change are checked less and less often.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};
use chrono::{Local, NaiveDate};
use serde::Serialize;

use super::changes::{self, RepoChanges};
use super::config::{Config, Role};
use super::state::{Outcome, ReviewEntry, ReviewRecord, State};
use super::{files, refs};

const HISTORY_LIMIT: usize = 20;

#[derive(Debug, Serialize)]
pub struct ReviewReport {
    /// The docs to review this run: most overdue first, at most `review.per_run`.
    pub pick: Vec<String>,
    /// For each picked doc with commits recorded by `reviewed --at`: per repo, what changed
    /// since the most recent recorded commit (narrowed by the doc's `sources` frontmatter).
    pub changes_since: BTreeMap<String, BTreeMap<String, RepoChanges>>,
    pub docs: Vec<DocReview>,
}

#[derive(Debug, Serialize)]
pub struct DocReview {
    pub file: String,
    pub last_reviewed: Option<String>,
    pub interval_days: i64,
    pub due: String,
    pub overdue_days: i64,
    pub reviews: usize,
    pub changed_reviews: usize,
    pub last_verified: Option<String>,
}

/// Durable docs under review: `.md` files in durable folders, except indexes and off-limits.
fn durable_docs(brain: &Path, cfg: &Config) -> Vec<String> {
    let folders = cfg.folders(Role::Durable);
    files::walk(brain)
        .into_iter()
        .filter(|f| f.ends_with(".md") && !f.ends_with("CLAUDE.md") && !cfg.is_off_limits(f))
        .filter(|f| folders.iter().any(|d| f.starts_with(&format!("{d}/"))))
        .collect()
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

pub fn schedule(brain: &Path, cfg: &Config, state: &State) -> ReviewReport {
    let today = Local::now().date_naive();
    let initial = cfg.raw.review.initial_days;
    let init_commits = refs::init_commits(brain);
    let mut docs: Vec<DocReview> = durable_docs(brain, cfg)
        .into_iter()
        .map(|file| {
            let text = files::read(brain, &file);
            let lv = files::last_verified(&text, files::frontmatter(&text).as_ref());
            let rec = state.reviews.get(&file);
            let last = rec.and_then(|r| parse_date(&r.last_reviewed));
            let interval = rec.map_or(initial, |r| r.interval_days);
            // Never reviewed: due one interval after its last verification, else after it
            // last changed (git, or mtime when history starts at `brain: init`).
            let anchor = last
                .or(lv)
                .or_else(|| refs::last_change(brain, &file, &init_commits).0);
            let due = anchor.map_or(today, |d| d + chrono::Days::new(interval.max(0) as u64));
            DocReview {
                last_reviewed: last.map(|d| d.to_string()),
                interval_days: interval,
                due: due.to_string(),
                overdue_days: (today - due).num_days(),
                reviews: rec.map_or(0, |r| r.history.len()),
                changed_reviews: rec.map_or(0, |r| {
                    r.history
                        .iter()
                        .filter(|e| e.outcome == Outcome::Changed)
                        .count()
                }),
                last_verified: lv.map(|d| d.to_string()),
                file,
            }
        })
        .collect();
    docs.sort_by(|a, b| {
        b.overdue_days
            .cmp(&a.overdue_days)
            .then(a.file.cmp(&b.file))
    });
    let pick: Vec<String> = docs
        .iter()
        .filter(|d| d.overdue_days >= 0)
        .take(cfg.raw.review.per_run)
        .map(|d| d.file.clone())
        .collect();
    let changes_since = pick
        .iter()
        .filter_map(|file| {
            let rec = state.reviews.get(file)?;
            let text = files::read(brain, file);
            let sources = files::frontmatter(&text)
                .map(|f| f.sources)
                .unwrap_or_default();
            let changes = changes::since(brain, cfg, rec, &sources);
            (!changes.is_empty()).then(|| (file.clone(), changes))
        })
        .collect();
    ReviewReport {
        pick,
        changes_since,
        docs,
    }
}

/// What a review found, and the source commits (resolved by [`changes::resolve_at`]) it was
/// checked against.
pub struct Finding {
    pub outcome: Outcome,
    pub note: Option<String>,
    pub at: BTreeMap<String, String>,
}

/// Record the same review of each of `files`; all are validated before any is recorded.
pub fn record_all(
    brain: &Path,
    cfg: &Config,
    state: &mut State,
    files: &[String],
    finding: &Finding,
) -> Result<BTreeMap<String, ReviewRecord>> {
    let durable = durable_docs(brain, cfg);
    if let Some(f) = files.iter().find(|f| !durable.contains(f)) {
        bail!("{f} is not a durable doc under review (see `ur dream review`)");
    }
    files
        .iter()
        .map(|f| Ok((f.clone(), record(brain, cfg, state, f, finding)?)))
        .collect()
}

/// Record a review of `file` and adapt its interval.
pub fn record(
    brain: &Path,
    cfg: &Config,
    state: &mut State,
    file: &str,
    finding: &Finding,
) -> Result<ReviewRecord> {
    let outcome = finding.outcome;
    if !durable_docs(brain, cfg).iter().any(|f| f == file) {
        bail!("{file} is not a durable doc under review (see `ur dream review`)");
    }
    let r = &cfg.raw.review;
    let today = Local::now().date_naive().to_string();
    let prev = state.reviews.get(file);
    let base = prev.map_or(r.initial_days, |p| p.interval_days);
    let interval = match outcome {
        // The first review only confirms the starting interval.
        Outcome::Unchanged if prev.is_none() => r.initial_days,
        Outcome::Unchanged => base * 2,
        Outcome::Changed => base / 2,
    }
    .clamp(r.min_days, r.max_days);
    let mut history = prev.map(|p| p.history.clone()).unwrap_or_default();
    history.push(ReviewEntry {
        date: today.clone(),
        outcome,
        note: finding.note.clone(),
        at: finding.at.clone(),
    });
    let excess = history.len().saturating_sub(HISTORY_LIMIT);
    history.drain(..excess);
    let rec = ReviewRecord {
        last_reviewed: today,
        interval_days: interval,
        history,
    };
    state.reviews.insert(file.to_string(), rec.clone());
    Ok(rec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dream::changes::tests::source_repo;
    use crate::dream::config::tests::SAMPLE;

    fn finding(outcome: Outcome) -> Finding {
        Finding {
            outcome,
            note: None,
            at: BTreeMap::new(),
        }
    }

    fn brain() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path();
        std::fs::create_dir_all(b.join("reference/generated")).unwrap();
        std::fs::write(b.join("DREAM.md"), SAMPLE).unwrap();
        std::fs::write(b.join("reference/CLAUDE.md"), "- [a](a.md)").unwrap();
        std::fs::write(b.join("reference/a.md"), "never verified").unwrap();
        std::fs::write(b.join("reference/b.md"), "Last verified: 2000-01-01\n").unwrap();
        std::fs::write(b.join("reference/generated/g.md"), "x").unwrap();
        tmp
    }

    #[test]
    fn schedule_orders_by_overdue_and_skips_indexes_and_off_limits() {
        let tmp = brain();
        let cfg = Config::load(tmp.path()).unwrap();
        let r = schedule(tmp.path(), &cfg, &State::default());
        let files: Vec<_> = r.docs.iter().map(|d| d.file.as_str()).collect();
        assert_eq!(files, vec!["reference/b.md", "reference/a.md"]);
        assert_eq!(r.pick, vec!["reference/b.md"]);
    }

    #[test]
    fn intervals_grow_when_unchanged_and_shrink_when_changed() {
        let tmp = brain();
        let cfg = Config::load(tmp.path()).unwrap();
        let mut st = State::default();
        let f = "reference/a.md";
        let step = |st: &mut State, o| {
            record(tmp.path(), &cfg, st, f, &finding(o))
                .unwrap()
                .interval_days
        };
        assert_eq!(step(&mut st, Outcome::Unchanged), 30);
        assert_eq!(step(&mut st, Outcome::Unchanged), 60);
        assert_eq!(step(&mut st, Outcome::Unchanged), 120);
        assert_eq!(step(&mut st, Outcome::Unchanged), 180);
        assert_eq!(step(&mut st, Outcome::Changed), 90);
        assert_eq!(st.reviews[f].history.len(), 5);
        let sched = schedule(tmp.path(), &cfg, &st);
        let a = sched.docs.iter().find(|d| d.file == f).unwrap();
        assert_eq!((a.reviews, a.changed_reviews, a.overdue_days), (5, 1, -90));
        assert!(
            record(
                tmp.path(),
                &cfg,
                &mut st,
                "reference/CLAUDE.md",
                &finding(Outcome::Changed)
            )
            .is_err()
        );
    }

    #[test]
    fn reviewed_at_is_stored_and_review_lists_changes_for_picked_docs() {
        let tmp = brain();
        let b = tmp.path();
        let first = source_repo(&b.join("src-clone"));
        let repos = "[[repos]]\nname = \"src\"\npath = \"src-clone\"\n\n[discovery]";
        std::fs::write(b.join("DREAM.md"), SAMPLE.replace("[discovery]", repos)).unwrap();
        std::fs::write(
            b.join("reference/b.md"),
            "---\nsources:\n  - src: src\n---\nLast verified: 2000-01-01\n",
        )
        .unwrap();
        let cfg = Config::load(b).unwrap();
        let mut st = State::default();
        let at = changes::resolve_at(b, &cfg, &[("src".into(), first[..10].into())]).unwrap();
        let files = vec!["reference/a.md".to_string(), "reference/b.md".to_string()];
        let f = Finding {
            outcome: Outcome::Unchanged,
            note: Some("n".into()),
            at,
        };
        let out = record_all(b, &cfg, &mut st, &files, &f).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(st.reviews["reference/b.md"].history[0].at["src"], first);
        let bad = vec![
            "reference/a.md".to_string(),
            "reference/CLAUDE.md".to_string(),
        ];
        let before = st.reviews["reference/a.md"].history.len();
        assert!(record_all(b, &cfg, &mut st, &bad, &f).is_err());
        assert_eq!(st.reviews["reference/a.md"].history.len(), before);

        // Make b.md due now so it is picked; its `sources` narrow the diff to `src/`.
        st.reviews.get_mut("reference/b.md").unwrap().last_reviewed = "2000-01-01".into();
        let r = schedule(b, &cfg, &st);
        assert_eq!(r.pick, vec!["reference/b.md"]);
        let c = &r.changes_since["reference/b.md"]["src"];
        assert_eq!((c.commits, c.since.as_str()), (Some(1), first.as_str()));
        assert_eq!(c.paths.as_deref(), Some(&["src/b.rs".to_string()][..]));
        assert!(!r.changes_since.contains_key("reference/a.md"));
    }
}
