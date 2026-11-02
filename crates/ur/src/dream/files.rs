//! Brain file helpers: walking, links, frontmatter, git.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;

const SKIP_DIRS: &[&str] = &[".git", ".dream"];
const TEXT_EXTS: &[&str] = &["md", "markdown", "txt"];
/// Absolute links into the brain as workers see it (the container mount point).
const MOUNT_PREFIX: &str = "/brain/";

static LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]\(([^)\s]+)\)").unwrap());
static IMPORT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^@(\S+)").unwrap());
static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z][\w+.-]*:").unwrap());
static LAST_VERIFIED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?mi)^Last verified:\s*(\d{4}-\d{2}-\d{2})").unwrap());
static FM_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+-\s*([\w-]+)\s*:\s*(.+)$").unwrap());
static FM_KEY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([\w-]+)\s*:\s*(.*)$").unwrap());

/// Relative `/`-separated paths of every file in the brain except `.git/` and `.dream/`.
pub fn walk(brain: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk_into(brain, brain, &mut out);
    out.sort();
    out
}

fn walk_into(brain: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk_into(brain, &path, out);
            }
        } else if name != ".DS_Store" {
            out.push(rel(brain, &path));
        }
    }
}

pub fn rel(brain: &Path, path: &Path) -> String {
    path.strip_prefix(brain)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Parent directory of a relative path (`""` at the root).
pub fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

pub fn is_text(rel: &str) -> bool {
    rel.rsplit_once('.')
        .is_some_and(|(_, ext)| TEXT_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn read(brain: &Path, rel: &str) -> String {
    std::fs::read(brain.join(rel))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

pub struct Link {
    pub line: usize,
    pub raw: String,
    /// Brain-relative target, or `None` when it points outside the brain.
    pub target: Option<String>,
}

/// Every link from `rel` that points at a brain file: markdown links and `@` imports.
pub fn links(rel: &str, text: &str) -> Vec<Link> {
    let caps = LINK_RE
        .captures_iter(text)
        .chain(IMPORT_RE.captures_iter(text));
    let mut out = Vec::new();
    for c in caps {
        let whole = c.get(0).unwrap();
        let target = c[1].split('#').next().unwrap_or("");
        if target.is_empty() || URL_RE.is_match(target) || target.starts_with('~') {
            continue;
        }
        let resolved = if let Some(inside) = target.strip_prefix(MOUNT_PREFIX) {
            normalize(Path::new(inside))
        } else if target.starts_with('/') {
            continue;
        } else {
            normalize(&Path::new(parent(rel)).join(target))
        };
        out.push(Link {
            line: text[..whole.start()].matches('\n').count() + 1,
            raw: whole.as_str().chars().take(160).collect(),
            target: resolved,
        });
    }
    out
}

/// Lexically normalize a relative path; `None` if it climbs above the brain root.
fn normalize(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

/// Minimal YAML frontmatter: scalar `key: value` lines, and `refs:` as `- kind: value` items.
#[derive(Debug, Default)]
pub struct Frontmatter {
    pub fields: BTreeMap<String, String>,
    pub refs: Vec<(String, String)>,
}

pub fn frontmatter(text: &str) -> Option<Frontmatter> {
    let body = text.strip_prefix("---\n")?;
    let end = body.find("\n---")?;
    let mut fm = Frontmatter::default();
    let mut key = String::new();
    for line in body[..end].lines() {
        let line = line.split(" #").next().unwrap_or("").trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if key == "refs"
            && let Some(c) = FM_REF_RE.captures(line)
        {
            fm.refs.push((c[1].to_string(), unquote(&c[2])));
            continue;
        }
        if let Some(c) = FM_KEY_RE.captures(line) {
            key = c[1].to_string();
            fm.fields.insert(key.clone(), unquote(&c[2]));
        }
    }
    Some(fm)
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches(|c| c == '"' || c == '\'').to_string()
}

/// `last-verified` from frontmatter, else a `Last verified: YYYY-MM-DD` body line.
pub fn last_verified(text: &str, fm: Option<&Frontmatter>) -> Option<NaiveDate> {
    let from_fm = fm.and_then(|f| f.fields.get("last-verified").cloned());
    let raw = from_fm.or_else(|| LAST_VERIFIED_RE.captures(text).map(|c| c[1].to_string()))?;
    NaiveDate::parse_from_str(&raw, "%Y-%m-%d").ok()
}

/// Run git with `brain` as the working directory (never `-C`: in a worker only the working
/// directory is mapped to the host). `None` on failure.
pub fn git(brain: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(brain)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Every file under `rel` (itself when it is a file), as absolute paths.
pub fn files_under(brain: &Path, rel: &str) -> Vec<PathBuf> {
    let path = brain.join(rel);
    if path.is_file() {
        return vec![path];
    }
    let mut out = Vec::new();
    walk_into(&path, &path, &mut out);
    out.into_iter().map(|r| path.join(r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_resolve_relative_mount_and_imports() {
        let text = "see [a](../x/a.md#h) and [b](/brain/y/b.md)\n@z.md\n\
                    [web](https://e.example) [up](../../out.md) [abs](/etc/p)";
        let got: Vec<_> = links("dir/f.md", text)
            .into_iter()
            .map(|l| (l.line, l.target))
            .collect();
        assert_eq!(
            got,
            vec![
                (1, Some("x/a.md".into())),
                (1, Some("y/b.md".into())),
                (3, None),
                (2, Some("dir/z.md".into())),
            ]
        );
    }

    #[test]
    fn frontmatter_reads_scalars_and_refs() {
        let text = "---\nstatus: active  # note\nrefs:\n  - ticket: EXAMPLE-1\n  - pr: 'o/r#2'\n\
                    last-verified: 2026-01-15\n---\nbody\n";
        let fm = frontmatter(text).unwrap();
        assert_eq!(fm.fields["status"], "active");
        assert_eq!(
            fm.refs,
            vec![
                ("ticket".into(), "EXAMPLE-1".into()),
                ("pr".into(), "o/r#2".into())
            ]
        );
        assert_eq!(
            last_verified(text, Some(&fm)),
            NaiveDate::from_ymd_opt(2026, 1, 15)
        );
        assert!(frontmatter("no frontmatter").is_none());
        assert_eq!(
            last_verified("x\nLast verified: 2025-02-03\n", None),
            NaiveDate::from_ymd_opt(2025, 2, 3)
        );
    }
}
