//! `ur dream status`: where the last dream run stopped.
//!
//! Reads the newest report, looks for its run commit (`brain:dream: <date>` on HEAD's history)
//! and lists uncommitted paths. "No reports yet" is `report: null`, not an error; git failures
//! and unreadable reports are errors.

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use super::files;
use super::report::{self, Report};

/// Brain-relative prefix of the dream's own files.
const DREAM_PREFIX: &str = ".dream/";

#[derive(Debug, Serialize)]
pub struct ReportStatus {
    /// Brain-relative path of the newest report.
    pub path: String,
    pub date: String,
    /// Subject the run commit has (or will have).
    pub run_subject: String,
    /// Hash of the newest commit on HEAD with exactly that subject; `None` if not committed.
    pub run_commit: Option<String>,
    /// Items under `Needs a human`; empty when the section is missing or empty.
    pub needs_human: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Uncommitted {
    /// Paths under `.dream/` (report, `state.json`).
    pub dream: Vec<String>,
    /// Every other uncommitted path (brain content).
    pub other: Vec<String>,
}

/// `ur dream status` output.
#[derive(Debug, Serialize)]
pub struct StatusReport {
    /// `None` when there are no reports yet.
    pub report: Option<ReportStatus>,
    pub uncommitted: Uncommitted,
}

pub fn status(brain: &Path) -> Result<StatusReport> {
    let uncommitted = uncommitted(brain)?;
    let report = match Report::newest(brain)? {
        Some(r) => Some(report_status(brain, &r)?),
        None => None,
    };
    Ok(StatusReport {
        report,
        uncommitted,
    })
}

fn report_status(brain: &Path, r: &Report) -> Result<ReportStatus> {
    let subject = r.run_subject();
    Ok(ReportStatus {
        path: r.path.clone(),
        date: r.date.format("%Y-%m-%d").to_string(),
        run_commit: commit_with_subject(brain, &subject)?,
        run_subject: subject,
        needs_human: r
            .section(report::NEEDS_HUMAN)
            .map(|s| report::items(&s))
            .unwrap_or_default(),
    })
}

/// Newest commit reachable from HEAD whose subject is exactly `subject`. A repository with no
/// commits yet has none.
fn commit_with_subject(brain: &Path, subject: &str) -> Result<Option<String>> {
    if files::git(brain, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_none() {
        return Ok(None);
    }
    let grep = format!("--grep={subject}");
    let log = files::git_checked(
        brain,
        &["log", "--fixed-strings", &grep, "--format=%H %s", "HEAD"],
    )?;
    Ok(log.lines().find_map(|l| {
        let (hash, s) = l.split_once(' ')?;
        (s == subject).then(|| hash.to_string())
    }))
}

/// Uncommitted paths (staged, unstaged, untracked), split into `.dream/` and the rest.
fn uncommitted(brain: &Path) -> Result<Uncommitted> {
    let out = files::git_checked(
        brain,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut split = Uncommitted::default();
    for path in porcelain_paths(&out) {
        if path.starts_with(DREAM_PREFIX) {
            split.dream.push(path);
        } else {
            split.other.push(path);
        }
    }
    split.dream.sort();
    split.other.sort();
    Ok(split)
}

/// Paths from `git status --porcelain=v1 -z`: `XY path\0`, and for a rename or copy the
/// original path follows as its own `\0`-terminated field (both are reported).
fn porcelain_paths(out: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    while let Some(entry) = fields.next() {
        let Some((code, path)) = entry.split_at_checked(3) else {
            continue;
        };
        paths.push(path.to_string());
        if code.starts_with('R') || code.starts_with('C') {
            paths.extend(fields.next().map(str::to_string));
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    const REPORT: &str = "# Dream report 2026-01-15\nSummary: 1 findings, 1 changes.\n\
                          ## Needs a human\n- reference/a.md: no source\n\
                          - decide x\n";

    #[test]
    fn no_reports_and_no_commits_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path();
        git(b, &["init", "-q"]);
        write(b, "notes.md", "x");
        let s = status(b).unwrap();
        assert!(s.report.is_none());
        assert_eq!(s.uncommitted.other, vec!["notes.md"]);
        assert!(s.uncommitted.dream.is_empty());
    }

    #[test]
    fn interrupted_then_committed_run() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path();
        git(b, &["init", "-q"]);
        write(b, "a.md", "a");
        git(b, &["add", "."]);
        git(b, &["commit", "-qm", "brain: init"]);

        // Interrupted run: report and an edit written, nothing committed.
        write(b, ".dream/reports/2026-01-15.md", REPORT);
        write(b, ".dream/state.json", "{}");
        write(b, "a.md", "b");
        let s = status(b).unwrap();
        let r = s.report.unwrap();
        assert_eq!(r.path, ".dream/reports/2026-01-15.md");
        assert_eq!(r.run_commit, None);
        assert_eq!(r.needs_human, vec!["reference/a.md: no source", "decide x"]);
        assert_eq!(
            s.uncommitted.dream,
            vec![".dream/reports/2026-01-15.md", ".dream/state.json"]
        );
        assert_eq!(s.uncommitted.other, vec!["a.md"]);

        // A subject that only contains the run subject does not count.
        git(b, &["add", "."]);
        git(b, &["commit", "-qm", "revert brain:dream: 2026-01-15"]);
        assert_eq!(status(b).unwrap().report.unwrap().run_commit, None);
        git(
            b,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "brain:dream: 2026-01-15",
            ],
        );
        let hash = files::git(b, &["rev-parse", "HEAD"]).unwrap();
        let s = status(b).unwrap();
        assert_eq!(s.report.unwrap().run_commit, Some(hash));
        assert_eq!(s.uncommitted, Uncommitted::default());
    }

    #[test]
    fn not_a_git_repo_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = status(tmp.path()).unwrap_err();
        assert!(format!("{err:#}").contains("git status"));
    }

    #[test]
    fn porcelain_reports_both_sides_of_a_rename() {
        let out = "R  new.md\0old.md\0 M .dream/state.json\0?? x y.md\0";
        assert_eq!(
            porcelain_paths(out),
            vec!["new.md", "old.md", ".dream/state.json", "x y.md"]
        );
    }
}
