//! `ur dream verify-report`: pre-check a report's proposed edits before apply mode.
//!
//! Each `### E<n> <path>: <change>` heading under `## Proposed edits` is one edit; its
//! `Target blob:` line is the `git hash-object` of the file as the report read it, or `new`.
//! Read-only: nothing is applied or written. Verdicts, checked in this order:
//!
//! - `malformed`: the heading has no `E<n> <path>`, or the edit has no `Target blob:` line, or
//!   its value is neither `new` nor a hex hash of at least 7 characters (`detail` says which).
//! - `outside-folders`: the path is absolute, climbs out with `..`, is under `.git/` or
//!   `.dream/`, or is under a top-level directory the config's `folders` don't list. Top-level
//!   files (`CLAUDE.md`, `DREAM.md`) are inside.
//! - `off-limits`: the path matches an `off_limits` glob.
//! - `missing`: a hash is recorded but the file does not exist.
//! - `blob-mismatch`: the file's current hash does not start with the recorded one, the edit is
//!   `new` but the file exists, or the path is not a regular file.
//! - `ok`: none of the above.

use std::path::{Component, Path};
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde::Serialize;

use super::config::Config;
use super::files;
use super::report::{self, Report};

static EDIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(E\d+)\s+(.+)$").unwrap());
const TARGET_BLOB: &str = "Target blob:";
const NEW: &str = "new";
/// Shortest recorded hash accepted (git's default abbreviation).
const MIN_HASH_LEN: usize = 7;
/// Top-level directories apply mode never edits.
const RESERVED_DIRS: &[&str] = &[".git", ".dream"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Ok,
    BlobMismatch,
    OffLimits,
    OutsideFolders,
    Missing,
    Malformed,
}

/// One edit's verdict.
#[derive(Debug, Serialize)]
pub struct EditVerdict {
    /// `E<n>`; `None` when the heading has none.
    pub id: Option<String>,
    /// The heading text, as written.
    pub heading: String,
    /// Brain-relative path as written in the heading.
    pub path: Option<String>,
    /// The `Target blob:` value as written (`new` or a hash).
    pub target_blob: Option<String>,
    /// `git hash-object` of the file now; `None` when it does not exist or was not hashed.
    pub current_blob: Option<String>,
    pub verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub total: usize,
    pub ok: usize,
    pub blob_mismatch: usize,
    pub off_limits: usize,
    pub outside_folders: usize,
    pub missing: usize,
    pub malformed: usize,
}

/// `ur dream verify-report` output.
#[derive(Debug, Serialize)]
pub struct VerifyReport {
    /// Brain-relative path of the verified report.
    pub report: String,
    pub summary: Summary,
    pub edits: Vec<EditVerdict>,
}

/// One edit as parsed from the report.
#[derive(Debug, PartialEq, Eq)]
struct Edit {
    heading: String,
    id: Option<String>,
    path: Option<String>,
    target_blob: Option<String>,
}

pub fn verify(brain: &Path, cfg: &Config, report: &Report) -> Result<VerifyReport> {
    let mut summary = Summary::default();
    let mut edits = Vec::new();
    for edit in parse_edits(&report.text) {
        let v = judge(brain, cfg, edit)?;
        summary.total += 1;
        *match v.verdict {
            Verdict::Ok => &mut summary.ok,
            Verdict::BlobMismatch => &mut summary.blob_mismatch,
            Verdict::OffLimits => &mut summary.off_limits,
            Verdict::OutsideFolders => &mut summary.outside_folders,
            Verdict::Missing => &mut summary.missing,
            Verdict::Malformed => &mut summary.malformed,
        } += 1;
        edits.push(v);
    }
    Ok(VerifyReport {
        report: report.path.clone(),
        summary,
        edits,
    })
}

/// Edits: headings one level below `Proposed edits`, each up to the next heading at that
/// level or higher. Headings inside code fences (a new file's content) are not edits.
fn parse_edits(text: &str) -> Vec<Edit> {
    let mut section: Option<usize> = None;
    let mut edits: Vec<Edit> = Vec::new();
    let mut in_edit = false;
    for (line, heading) in report::lines_with_headings(text) {
        match (section, heading) {
            (None, Some((level, t))) if t == report::PROPOSED_EDITS => section = Some(level),
            (Some(open), Some((level, _))) if level <= open => break,
            (Some(open), Some((level, t))) if level == open + 1 => {
                edits.push(edit_heading(t));
                in_edit = true;
            }
            (Some(open), Some((level, _))) if level == open + 2 || !in_edit => {}
            (Some(_), _) if in_edit => {
                let last = edits.last_mut().expect("in_edit implies an edit");
                if last.target_blob.is_none()
                    && let Some(v) = line.trim().strip_prefix(TARGET_BLOB)
                {
                    last.target_blob = Some(clean(v));
                }
            }
            _ => {}
        }
    }
    edits
}

fn edit_heading(text: &str) -> Edit {
    let caps = EDIT_RE.captures(text);
    let path = caps.as_ref().map(|c| {
        let rest = c[2].trim();
        let path = rest
            .split_once(": ")
            .map_or(rest.strip_suffix(':').unwrap_or(rest), |(p, _)| p);
        clean(path)
    });
    Edit {
        heading: text.to_string(),
        id: caps.as_ref().map(|c| c[1].to_string()),
        path: path.filter(|p| !p.is_empty()),
        target_blob: None,
    }
}

/// Strip whitespace, backticks and quotes around a value.
fn clean(s: &str) -> String {
    s.trim()
        .trim_matches(|c| c == '`' || c == '"' || c == '\'')
        .trim()
        .to_string()
}

fn judge(brain: &Path, cfg: &Config, edit: Edit) -> Result<EditVerdict> {
    let mut v = EditVerdict {
        id: edit.id,
        heading: edit.heading,
        path: edit.path,
        target_blob: edit.target_blob,
        current_blob: None,
        verdict: Verdict::Ok,
        detail: None,
    };
    let (verdict, detail) = match (&v.path, &v.target_blob) {
        (None, _) => (
            Verdict::Malformed,
            "heading is not `E<n> <path>: <change>`".into(),
        ),
        (Some(_), None) => (Verdict::Malformed, format!("no `{TARGET_BLOB}` line")),
        (Some(path), Some(blob)) => match check_path(cfg, path) {
            Some(rejected) => rejected,
            None => check_blob(brain, path, blob, &mut v.current_blob)?,
        },
    };
    v.verdict = verdict;
    v.detail = (verdict != Verdict::Ok).then_some(detail);
    Ok(v)
}

/// Why the path may not be edited, or `None` when it may.
fn check_path(cfg: &Config, path: &str) -> Option<(Verdict, String)> {
    let Some(rel) = brain_relative(path) else {
        return Some((
            Verdict::OutsideFolders,
            "not a brain-relative path (absolute or climbs out with `..`)".into(),
        ));
    };
    if !in_folders(cfg, &rel) {
        return Some((
            Verdict::OutsideFolders,
            "not under a folder the config lists".into(),
        ));
    }
    cfg.is_off_limits(&rel)
        .then(|| (Verdict::OffLimits, "matches an off_limits glob".into()))
}

/// Compare the file's current hash to `blob` (`new` or a hash), recording it in `current`.
fn check_blob(
    brain: &Path,
    path: &str,
    blob: &str,
    current: &mut Option<String>,
) -> Result<(Verdict, String)> {
    let is_new = blob.eq_ignore_ascii_case(NEW);
    let is_hash = blob.len() >= MIN_HASH_LEN && blob.chars().all(|c| c.is_ascii_hexdigit());
    if !is_new && !is_hash {
        return Ok((
            Verdict::Malformed,
            format!("`{TARGET_BLOB}` {blob:?} is neither `new` nor a hash"),
        ));
    }
    let rel = brain_relative(path).expect("checked by check_path");
    let meta = match std::fs::symlink_metadata(brain.join(&rel)) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && is_new => {
            return Ok((Verdict::Ok, String::new()));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Verdict::Missing, "the file does not exist".into()));
        }
        Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {rel}"))),
    };
    if !meta.is_file() {
        return Ok((Verdict::BlobMismatch, "not a regular file".into()));
    }
    let hash = files::git_checked(brain, &["hash-object", "--", &rel])?
        .trim()
        .to_string();
    let matches = !is_new && hash.starts_with(&blob.to_ascii_lowercase());
    *current = Some(hash);
    Ok(match (is_new, matches) {
        (true, _) => (
            Verdict::BlobMismatch,
            "edit is `new` but the file exists".into(),
        ),
        (false, false) => (
            Verdict::BlobMismatch,
            "the file changed since the report".into(),
        ),
        (false, true) => (Verdict::Ok, String::new()),
    })
}

/// `path` as a normalized brain-relative path; `None` if absolute or it contains `..`.
fn brain_relative(path: &str) -> Option<String> {
    if path.starts_with('/') || path.starts_with('~') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for c in Path::new(path).components() {
        match c {
            Component::Normal(p) => parts.push(p.to_str()?),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Top-level files, and files under a configured folder; never `.git/` or `.dream/`.
fn in_folders(cfg: &Config, rel: &str) -> bool {
    let Some((top, _)) = rel.split_once('/') else {
        return true;
    };
    if RESERVED_DIRS.contains(&top) {
        return false;
    }
    cfg.raw.folders.iter().any(|f| {
        let dir = f.path.trim_matches('/');
        !dir.is_empty() && rel.starts_with(&format!("{dir}/"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use std::process::Command;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn hash(dir: &Path, rel: &str) -> String {
        files::git(dir, &["hash-object", "--", rel]).unwrap()
    }

    fn report(text: String) -> Report {
        Report {
            date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            path: ".dream/reports/2026-01-15.md".into(),
            text,
        }
    }

    #[test]
    fn parse_edits_reads_headings_blobs_and_skips_fenced_content() {
        let text = "# Dream report\n## Findings\n### E9 not/an/edit.md: x\nTarget blob: abc\n\
                    ## Proposed edits\n### E1 `reference/a.md`: fix it\nTarget blob: `abcdef1`\n\
                    Why: y\n```diff\n-Target blob: zzz\n```\n#### detail\nmore\n\
                    ### E2 reference/new.md\nTarget blob: new\n```markdown\n### E3 fake.md: no\n\
                    ```\n### Not an edit\n## Not proposed (needs a human)\n### E4 x.md: no\n";
        let got = parse_edits(text);
        let summary: Vec<_> = got
            .iter()
            .map(|e| (e.id.as_deref(), e.path.as_deref(), e.target_blob.as_deref()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (Some("E1"), Some("reference/a.md"), Some("abcdef1")),
                (Some("E2"), Some("reference/new.md"), Some("new")),
                (None, None, None),
            ]
        );
        assert!(parse_edits("# no edits\n").is_empty());
    }

    #[test]
    fn brain_relative_rejects_escapes() {
        assert_eq!(brain_relative("./a/./b.md").as_deref(), Some("a/b.md"));
        for p in [
            "/etc/passwd",
            "../x.md",
            "a/../../x.md",
            "a/../b.md",
            "~/x",
            ".",
        ] {
            assert_eq!(brain_relative(p), None, "{p}");
        }
    }

    #[test]
    fn verify_gives_each_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(b)
                .status()
                .unwrap()
                .success()
        );
        write(b, "DREAM.md", super::super::config::tests::SAMPLE);
        write(b, "CLAUDE.md", "root");
        write(b, "reference/a.md", "a");
        write(b, "reference/changed.md", "before");
        write(b, "reference/generated/g.md", "g");
        write(b, "extra/x.md", "x");
        let a = hash(b, "reference/a.md");
        let before = hash(b, "reference/changed.md");
        write(b, "reference/changed.md", "after");
        let root = hash(b, "CLAUDE.md");
        let g = hash(b, "reference/generated/g.md");

        let text = format!(
            "## Proposed edits\n\
             ### E1 reference/a.md: ok\nTarget blob: {short}\n\
             ### E2 reference/changed.md: changed\nTarget blob: {before}\n\
             ### E3 reference/generated/g.md: off\nTarget blob: {g}\n\
             ### E4 extra/x.md: unlisted\nTarget blob: new\n\
             ### E5 ../outside.md: escape\nTarget blob: new\n\
             ### E6 reference/gone.md: gone\nTarget blob: {a}\n\
             ### E7 reference/new.md: create\nTarget blob: new\n\
             ### E8 reference/a.md: exists\nTarget blob: new\n\
             ### E9 CLAUDE.md: root\nTarget blob: {root}\n\
             ### E10 reference/b.md: no blob\nWhy: x\n\
             ### E11 reference/a.md: bad\nTarget blob: soon\n\
             ### E12 .dream/state.json: state\nTarget blob: new\n\
             ### E13 reference: dir\nTarget blob: {a}\n\
             ### Retitled\n",
            short = &a[..7],
        );
        let cfg = Config::load(b).unwrap();
        let out = verify(b, &cfg, &report(text)).unwrap();
        let verdicts: Vec<_> = out
            .edits
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("-"), e.verdict))
            .collect();
        use Verdict::*;
        assert_eq!(
            verdicts,
            vec![
                ("E1", Ok),
                ("E2", BlobMismatch),
                ("E3", OffLimits),
                ("E4", OutsideFolders),
                ("E5", OutsideFolders),
                ("E6", Missing),
                ("E7", Ok),
                ("E8", BlobMismatch),
                ("E9", Ok),
                ("E10", Malformed),
                ("E11", Malformed),
                ("E12", OutsideFolders),
                ("E13", BlobMismatch),
                ("-", Malformed),
            ]
        );
        assert_eq!(out.edits[0].current_blob.as_deref(), Some(a.as_str()));
        assert!(out.edits[0].detail.is_none());
        assert_eq!(
            out.edits[1].current_blob,
            Some(hash(b, "reference/changed.md"))
        );
        assert!(
            out.edits[9]
                .detail
                .as_deref()
                .unwrap()
                .contains("Target blob")
        );
        assert_eq!(
            out.summary,
            Summary {
                total: 14,
                ok: 3,
                blob_mismatch: 3,
                off_limits: 1,
                outside_folders: 3,
                missing: 1,
                malformed: 3,
            }
        );
        // Read-only: nothing changed on disk.
        assert_eq!(files::read(b, "reference/changed.md"), "after");
        assert!(!b.join("reference/new.md").exists());
    }
}
