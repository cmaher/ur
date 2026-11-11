//! Dream reports: `<brain>/.dream/reports/YYYY-MM-DD.md`, the changelog the brain:dream skill
//! writes as it edits the brain.
//!
//! Sections are Markdown headings; a heading inside a fenced code block (e.g. a quoted diff) is
//! not a section boundary.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::NaiveDate;

/// Brain-relative directory holding the reports.
pub const REPORTS_DIR: &str = ".dream/reports";
/// Heading for findings the agent could not act on.
pub const NEEDS_HUMAN: &str = "Needs a human";

/// One dream report as read from disk.
#[derive(Debug, Clone)]
pub struct Report {
    pub date: NaiveDate,
    /// Brain-relative path, e.g. `.dream/reports/2026-01-15.md`.
    pub path: String,
    pub text: String,
}

impl Report {
    /// The newest report by filename date, or `None` when there are no reports yet (missing
    /// directory or no `YYYY-MM-DD.md` file in it). Other filenames are ignored.
    pub fn newest(brain: &Path) -> Result<Option<Report>> {
        let dir = brain.join(REPORTS_DIR);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        let mut newest: Option<NaiveDate> = None;
        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
            let name = entry.file_name();
            let Some(date) = name.to_str().and_then(date_of) else {
                continue;
            };
            if newest.is_none_or(|n| date > n) {
                newest = Some(date);
            }
        }
        newest.map(|d| Self::load(brain, d)).transpose()
    }

    /// Read the report for `date`.
    pub fn load(brain: &Path, date: NaiveDate) -> Result<Report> {
        let path = format!("{REPORTS_DIR}/{}.md", date.format("%Y-%m-%d"));
        let text = std::fs::read_to_string(brain.join(&path))
            .with_context(|| format!("reading report {path}"))?;
        Ok(Report { date, path, text })
    }

    /// Subject of the commit a dream run makes for this report.
    pub fn run_subject(&self) -> String {
        format!("brain:dream: {}", self.date.format("%Y-%m-%d"))
    }

    /// Body of the first section titled `title` (any heading level) in the report's last run —
    /// the text from its last level-1 heading, since a second run on the same day appends a new
    /// `# Dream report <date> (run N)` block — up to the next heading of the same or a higher
    /// level. `None` when there is no such heading.
    pub fn section(&self, title: &str) -> Option<String> {
        let lines = lines_with_headings(&self.text);
        let last_run = lines
            .iter()
            .rposition(|(_, h)| matches!(h, Some((1, _))))
            .unwrap_or(0);
        let mut body: Option<(usize, Vec<&str>)> = None;
        for (line, heading) in lines.into_iter().skip(last_run) {
            match (&mut body, heading) {
                (None, Some((level, text))) if text == title => body = Some((level, Vec::new())),
                (Some((open, _)), Some((level, _))) if level <= *open => break,
                (Some((_, lines)), _) => lines.push(line),
                (None, _) => {}
            }
        }
        body.map(|(_, lines)| lines.join("\n"))
    }
}

/// `YYYY-MM-DD.md` → its date.
fn date_of(name: &str) -> Option<NaiveDate> {
    let stem = name.strip_suffix(".md")?;
    (stem.len() == 10)
        .then(|| NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok())
        .flatten()
}

/// Each line with its heading `(level, text)` when it is one outside a code fence.
pub(super) fn lines_with_headings(text: &str) -> Vec<(&str, Option<(usize, &str)>)> {
    let mut fenced = false;
    text.lines()
        .map(|line| {
            let t = line.trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                fenced = !fenced;
                return (line, None);
            }
            (line, if fenced { None } else { heading(line) })
        })
        .collect()
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    (rest.is_empty() || rest.starts_with(' ')).then(|| (level, rest.trim()))
}

/// The items of a section: each top-level list item (`-`, `*`, `+`, `1.`, `1)`) with its
/// continuation lines, and each other paragraph. Markers are stripped; nested lines are kept.
pub fn items(section: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut after_blank = true;
    for line in section.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            after_blank = true;
            continue;
        }
        let indented = line.len() > trimmed.len();
        match (indented, list_item(trimmed), items.last_mut()) {
            (false, Some(rest), _) => items.push(rest.trim_end().to_string()),
            (_, _, Some(last)) if indented || !after_blank => {
                last.push('\n');
                last.push_str(line.trim_end());
            }
            _ => items.push(trimmed.trim_end().to_string()),
        }
        after_blank = false;
    }
    items
}

/// The text after a list marker, or `None` when `line` is not a list item.
fn list_item(line: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest);
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = &line[digits..];
    (digits > 0)
        .then(|| rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(text: &str) -> Report {
        Report {
            date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            path: format!("{REPORTS_DIR}/2026-01-15.md"),
            text: text.to_string(),
        }
    }

    #[test]
    fn section_stops_at_same_level_and_ignores_fenced_headings() {
        let r = report(
            "# Dream report 2026-01-15\n## Changes\n### C1 a.md: x\n```markdown\n\
             ## Needs a human\nfake\n```\n## Needs a human\n\
             - one\n### sub\n- two\n## Open questions\n- why\n",
        );
        assert_eq!(r.section(NEEDS_HUMAN).unwrap(), "- one\n### sub\n- two");
        assert_eq!(r.section("Open questions").unwrap(), "- why");
        assert!(r.section("Reviewed").is_none());
        assert_eq!(r.run_subject(), "brain:dream: 2026-01-15");
    }

    #[test]
    fn section_reads_the_last_run_block() {
        let r = report(
            "# Dream report 2026-01-15\n## Needs a human\n- old\n\
             # Dream report 2026-01-15 (run 2)\n## Changes\n- c\n\
             ```\n# not a run\n## Needs a human\n- fenced\n```\n\
             ## Needs a human\n- new\n",
        );
        assert_eq!(r.section(NEEDS_HUMAN).unwrap(), "- new");
        let one_run = report("# Dream report 2026-01-15\n## Needs a human\n- only\n");
        assert_eq!(one_run.section(NEEDS_HUMAN).unwrap(), "- only");
        let no_run = report("## Needs a human\n- headless\n");
        assert_eq!(no_run.section(NEEDS_HUMAN).unwrap(), "- headless");
    }

    #[test]
    fn items_split_list_entries_and_paragraphs() {
        let got = items(
            "- first\n  continued\n* second\n1. third\n10) fourth\n\nA paragraph\nwrapped\n\n\
             another\n",
        );
        assert_eq!(
            got,
            vec![
                "first\n  continued",
                "second",
                "third",
                "fourth",
                "A paragraph\nwrapped",
                "another"
            ]
        );
        assert!(items("\n\n").is_empty());
    }

    #[test]
    fn newest_picks_latest_dated_file() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path();
        assert!(Report::newest(b).unwrap().is_none());
        let dir = b.join(REPORTS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Report::newest(b).unwrap().is_none());
        std::fs::write(dir.join("2026-01-02.md"), "old").unwrap();
        std::fs::write(dir.join("2026-02-01.md"), "new").unwrap();
        std::fs::write(dir.join("2026-13-01.md"), "bad date").unwrap();
        std::fs::write(dir.join("notes.md"), "other").unwrap();
        let r = Report::newest(b).unwrap().unwrap();
        assert_eq!(r.path, ".dream/reports/2026-02-01.md");
        assert_eq!(r.text, "new");
    }

    #[test]
    fn unreadable_report_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(REPORTS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026-01-02.md"), [0xff, 0xfe]).unwrap();
        let err = Report::newest(tmp.path()).unwrap_err();
        assert!(format!("{err:#}").contains("2026-01-02.md"));
    }
}
