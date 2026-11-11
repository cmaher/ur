//! `ur dream lint`: the mechanical half of the Lint pass.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::config::{Config, Role};
use super::files::{self, join, parent};

const LONG_ENTRY_WORDS: usize = 25;
const INDEX: &str = "CLAUDE.md";
const LINE_REF_EXAMPLES: usize = 5;

/// `path/to/file.ext:123` or `file.ext:12-34` for a source file: line numbers go stale with the
/// next edit to that file.
static LINE_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b[\w./-]*\w\.(?:rs|go|py|ts|tsx|js|jsx|java|kt|rb|sol|sql|proto|lua|sh|ya?ml|tf):\d+(?:-\d+)?\b",
    )
    .unwrap()
});

#[derive(Debug, Serialize)]
pub struct LintReport {
    pub broken_links: Vec<BrokenLink>,
    pub missing_from_index: Vec<IndexedFile>,
    pub orphans: Vec<IndexedFile>,
    pub long_index_entries: Vec<LongEntry>,
    pub empty_folders: Vec<String>,
    pub unlisted_folders: Vec<String>,
    /// Kept docs (durable, orientation, procedure) citing source lines as `file:line`.
    pub line_refs: Vec<LineRefs>,
}

#[derive(Debug, Serialize)]
pub struct LineRefs {
    pub file: String,
    pub count: usize,
    /// The first few, as `(line, ref)`.
    pub examples: Vec<LineRef>,
}

#[derive(Debug, Serialize)]
pub struct LineRef {
    pub line: usize,
    #[serde(rename = "ref")]
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct BrokenLink {
    pub file: String,
    pub line: usize,
    pub link: String,
    pub resolved: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IndexedFile {
    pub file: String,
    pub index: String,
}

#[derive(Debug, Serialize)]
pub struct LongEntry {
    pub file: String,
    pub line: usize,
    pub words: usize,
}

pub fn lint(brain: &Path, cfg: &Config) -> LintReport {
    let all = files::walk(brain);
    let fileset: BTreeSet<&str> = all.iter().map(String::as_str).collect();
    let dirs: BTreeSet<&str> = all.iter().map(|f| parent(f)).collect();

    let mut linked_from: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut broken_links = Vec::new();
    for rel in all.iter().filter(|f| files::is_text(f)) {
        for link in files::links(rel, &files::read(brain, rel)) {
            let exists = link
                .target
                .as_deref()
                .is_some_and(|t| fileset.contains(t) || dirs.contains(t));
            if exists {
                let target = link.target.unwrap();
                linked_from.entry(target).or_default().insert(rel.clone());
            } else {
                broken_links.push(BrokenLink {
                    file: rel.clone(),
                    line: link.line,
                    link: link.raw,
                    resolved: link.target,
                });
            }
        }
    }

    let (missing_from_index, orphans) = index_coverage(cfg, &all, &fileset, &linked_from);
    LintReport {
        broken_links,
        missing_from_index,
        orphans,
        long_index_entries: long_entries(brain, &all),
        empty_folders: empty_folders(brain),
        unlisted_folders: unlisted_folders(cfg, &all),
        line_refs: line_refs(brain, cfg, &all),
    }
}

/// `file:line` citations in kept docs, outside fenced code blocks.
fn line_refs(brain: &Path, cfg: &Config, all: &[String]) -> Vec<LineRefs> {
    let kept: Vec<String> = [Role::Durable, Role::Orientation, Role::Procedure]
        .into_iter()
        .flat_map(|r| cfg.folders(r))
        .collect();
    let mut out = Vec::new();
    for rel in all {
        let in_kept = kept.iter().any(|f| rel.starts_with(&format!("{f}/")));
        if !in_kept || !files::is_text(rel) || cfg.is_off_limits(rel) {
            continue;
        }
        let mut refs = Vec::new();
        let mut fenced = false;
        for (i, line) in files::read(brain, rel).lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                continue;
            }
            refs.extend(LINE_REF_RE.find_iter(line).map(|m| LineRef {
                line: i + 1,
                text: m.as_str().to_string(),
            }));
        }
        if !refs.is_empty() {
            let count = refs.len();
            refs.truncate(LINE_REF_EXAMPLES);
            out.push(LineRefs {
                file: rel.clone(),
                count,
                examples: refs,
            });
        }
    }
    out
}

/// The nearest `CLAUDE.md` above `rel`, excluding the root orientation file.
fn index_for(rel: &str, fileset: &BTreeSet<&str>) -> Option<String> {
    let mut dir = parent(rel);
    while !dir.is_empty() {
        let idx = join(dir, INDEX);
        if idx != rel && fileset.contains(idx.as_str()) {
            return Some(idx);
        }
        dir = parent(dir);
    }
    None
}

/// Files their index doesn't reach, and files nothing links to. A file counts as indexed when
/// its index links the file, its folder, or its folder's `README.md` (a stream's entry).
fn index_coverage(
    cfg: &Config,
    all: &[String],
    fileset: &BTreeSet<&str>,
    linked_from: &BTreeMap<String, BTreeSet<String>>,
) -> (Vec<IndexedFile>, Vec<IndexedFile>) {
    let links_to =
        |target: &str, from: &str| linked_from.get(target).is_some_and(|s| s.contains(from));
    let mut missing = Vec::new();
    let mut orphans = Vec::new();
    for rel in all {
        if !rel.ends_with(".md") || rel.ends_with(INDEX) || cfg.is_off_limits(rel) {
            continue;
        }
        let Some(index) = index_for(rel, fileset) else {
            continue;
        };
        let dir = parent(rel);
        let entry = [rel.clone(), join(dir, "README.md"), dir.to_string()];
        if !entry.iter().any(|t| links_to(t, &index)) {
            missing.push(IndexedFile {
                file: rel.clone(),
                index: index.clone(),
            });
        }
        let linked = linked_from
            .get(rel)
            .is_some_and(|s| s.iter().any(|f| f != rel));
        if !linked {
            orphans.push(IndexedFile {
                file: rel.clone(),
                index,
            });
        }
    }
    (missing, orphans)
}

fn long_entries(brain: &Path, all: &[String]) -> Vec<LongEntry> {
    let mut out = Vec::new();
    for idx in all.iter().filter(|f| f.ends_with(INDEX)) {
        for (i, line) in files::read(brain, idx).lines().enumerate() {
            let t = line.trim_start();
            let is_entry = t.starts_with("- [") || t.starts_with("* [");
            let words = line.split_whitespace().count();
            if is_entry && words > LONG_ENTRY_WORDS {
                out.push(LongEntry {
                    file: idx.clone(),
                    line: i + 1,
                    words,
                });
            }
        }
    }
    out
}

fn empty_folders(brain: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect_empty(brain, brain, &mut out);
    out.sort();
    out
}

/// Whether `dir` holds nothing but `.DS_Store`; records empty leaf folders.
fn collect_empty(brain: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut has_content = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".DS_Store" || (dir == brain && [".git", ".dream"].contains(&name.as_str())) {
            continue;
        }
        has_content = true;
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_empty(brain, &entry.path(), out);
        }
    }
    if !has_content && dir != brain {
        out.push(files::rel(brain, dir));
    }
}

fn unlisted_folders(cfg: &Config, all: &[String]) -> Vec<String> {
    let listed: BTreeSet<String> = cfg
        .raw
        .folders
        .iter()
        .map(|f| f.path.trim_matches('/').to_string())
        .collect();
    all.iter()
        .filter_map(|f| f.split_once('/').map(|(top, _)| top.to_string()))
        .filter(|top| !listed.contains(top))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn lint_finds_each_problem_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path();
        write(b, "DREAM.md", super::super::config::tests::SAMPLE);
        write(b, "CLAUDE.md", "- [Reference](reference/CLAUDE.md)\n");
        let long = format!("- [Indexed](indexed.md) — {}\n", "word ".repeat(30));
        write(
            b,
            "reference/CLAUDE.md",
            &format!("{long}- [Gone](gone.md) — x\n"),
        );
        write(
            b,
            "reference/indexed.md",
            "links [unindexed](unindexed.md), see `pkg/a/b.go:33` and x.rs:1-9\n\
             ```\nfenced.go:1\n```\nversion 1.2:3 and https://e.example/p.go:4x",
        );
        write(b, "reference/unindexed.md", "x");
        write(b, "reference/orphan.md", "x");
        write(b, "reference/generated/dump.md", "x");
        write(b, "working/CLAUDE.md", "- [Stream](stream/README.md) — s\n");
        write(b, "working/stream/README.md", "[d](design.md)");
        write(b, "working/stream/design.md", "scratch.go:12");
        write(b, "extra/note.md", "x");
        std::fs::create_dir_all(b.join("reference/empty")).unwrap();

        let cfg = Config::load(b).unwrap();
        let r = lint(b, &cfg);
        let names = |v: &[IndexedFile]| v.iter().map(|f| f.file.clone()).collect::<Vec<_>>();
        assert_eq!(r.broken_links.len(), 1, "{:?}", r.broken_links);
        assert_eq!(
            r.broken_links[0].resolved.as_deref(),
            Some("reference/gone.md")
        );
        assert_eq!(
            names(&r.missing_from_index),
            vec!["reference/orphan.md", "reference/unindexed.md"]
        );
        assert_eq!(names(&r.orphans), vec!["reference/orphan.md"]);
        assert_eq!(r.long_index_entries.len(), 1);
        assert_eq!(r.empty_folders, vec!["reference/empty"]);
        assert_eq!(r.unlisted_folders, vec!["extra"]);
        assert_eq!(r.line_refs.len(), 1, "{:?}", r.line_refs);
        let lr = &r.line_refs[0];
        assert_eq!(lr.file, "reference/indexed.md");
        let got: Vec<_> = lr.examples.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(got, vec!["pkg/a/b.go:33", "x.rs:1-9"]);
        assert_eq!(lr.count, 2);
    }
}
