//! Source commits for durable-doc reviews: `reviewed --at <repo>=<sha>` records them, and
//! `review` lists what changed in each repo since (`changes_since`).
//!
//! Git runs in each repo's configured clone (`[[repos]]` in DREAM.md) and never fetches: changes
//! are listed up to the repo's `branch` as the clone last saw it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::config::{Config, Repo};
use super::files;
use super::state::ReviewRecord;

/// What changed in one repo since a doc's last recorded review commit. On failure `commits`
/// and `paths` are `null` and `error` says why; they are never empty in place of an error.
#[derive(Debug, Serialize)]
pub struct RepoChanges {
    /// The commit recorded with the review.
    pub since: String,
    /// The date of the review that recorded it.
    pub reviewed_on: String,
    pub branch: Option<String>,
    /// The doc's `sources` paths for this repo; empty means the whole repo.
    pub sources: Vec<String>,
    pub commits: Option<usize>,
    pub paths: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parse a `--at` value, `<repo>=<sha>`.
pub fn parse_at(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((repo, sha)) if !repo.is_empty() && !sha.is_empty() => {
            Ok((repo.to_string(), sha.to_string()))
        }
        _ => Err(format!("expected <repo>=<sha>, got {s:?}")),
    }
}

/// Resolve each `--at` pair to a full commit in its repo's clone. Unknown repos, repeated
/// repos, and shas that aren't commits there are errors.
pub fn resolve_at(
    brain: &Path,
    cfg: &Config,
    at: &[(String, String)],
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (name, sha) in at {
        let Some(repo) = cfg.repo(name) else {
            let known: Vec<_> = cfg.raw.repos.iter().map(|r| r.name.as_str()).collect();
            bail!("--at {name}: not a repo in DREAM.md (repos: {known:?})");
        };
        let dir = repo_dir(brain, repo)?;
        let full = files::git_checked(
            &dir,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{sha}^{{commit}}"),
            ],
        )
        .with_context(|| format!("--at {name}={sha}"))?;
        if out.insert(name.clone(), full.trim().to_string()).is_some() {
            bail!("--at {name}: given more than once");
        }
    }
    Ok(out)
}

/// Changes since `rec`'s latest recorded commit per repo, narrowed by the doc's `sources`.
pub fn since(
    brain: &Path,
    cfg: &Config,
    rec: &ReviewRecord,
    sources: &[(String, String)],
) -> BTreeMap<String, RepoChanges> {
    rec.latest_at()
        .into_iter()
        .map(|(name, (sha, date))| {
            let paths: Vec<String> = sources
                .iter()
                .filter(|(r, _)| *r == name)
                .map(|(_, p)| p.clone())
                .collect();
            let repo = cfg.repo(&name);
            let result = match repo {
                Some(repo) => diff(brain, repo, &sha, &paths),
                None => Err(anyhow::anyhow!("{name} is no longer a repo in DREAM.md")),
            };
            let (commits, changed, error) = match result {
                Ok((n, p)) => (Some(n), Some(p), None),
                Err(e) => (None, None, Some(format!("{e:#}"))),
            };
            let changes = RepoChanges {
                since: sha,
                reviewed_on: date,
                branch: repo.map(|r| r.branch.clone()),
                sources: paths,
                commits,
                paths: changed,
                error,
            };
            (name, changes)
        })
        .collect()
}

fn repo_dir(brain: &Path, repo: &Repo) -> Result<std::path::PathBuf> {
    let dir = repo.dir(brain);
    if !dir.is_dir() {
        bail!("repo {}: {} is not a directory", repo.name, dir.display());
    }
    Ok(dir)
}

/// Commit count and changed paths in `<sha>..<branch>`, limited to `sources` when given.
fn diff(brain: &Path, repo: &Repo, sha: &str, sources: &[String]) -> Result<(usize, Vec<String>)> {
    let dir = repo_dir(brain, repo)?;
    let range = format!("{sha}..{}", repo.branch);
    let with_paths = |head: &[&str]| -> Vec<String> {
        let mut args: Vec<String> = head.iter().map(|s| s.to_string()).collect();
        args.push(range.clone());
        args.push("--".to_string());
        args.extend(sources.iter().cloned());
        args
    };
    let run = |args: Vec<String>| {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        files::git_checked(&dir, &refs)
    };
    let count = run(with_paths(&["rev-list", "--count", "--end-of-options"]))?;
    let commits = count
        .trim()
        .parse()
        .with_context(|| format!("git rev-list --count printed {count:?}"))?;
    let log = run(with_paths(&[
        "log",
        "--format=",
        "--name-only",
        "--end-of-options",
    ]))?;
    let paths: BTreeSet<String> = log
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    Ok((commits, paths.into_iter().collect()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::dream::state::{Outcome, ReviewEntry};

    pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
        files::git_checked(dir, args).unwrap().trim().to_string()
    }

    pub(crate) fn commit(dir: &Path, file: &str, text: &str) -> String {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        git(dir, &["add", "-A"]);
        git(
            dir,
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", file],
        );
        git(dir, &["rev-parse", "HEAD"])
    }

    /// A clone whose `origin/master` is a local ref, so nothing is fetched.
    pub(crate) fn source_repo(dir: &Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "master"]);
        git(dir, &["config", "user.email", "t@example.com"]);
        git(dir, &["config", "user.name", "t"]);
        let first = commit(dir, "src/a.rs", "a");
        commit(dir, "src/b.rs", "b");
        commit(dir, "docs/c.md", "c");
        git(dir, &["update-ref", "refs/remotes/origin/master", "HEAD"]);
        commit(dir, "src/unpushed.rs", "local only");
        first
    }

    fn config(repo: &Path) -> Config {
        let toml = format!(
            "```toml\n[[repos]]\nname = \"src\"\npath = '{}'\n\n\
             [[repos]]\nname = \"gone\"\npath = \"missing-dir\"\n```\n",
            repo.display()
        );
        Config::parse(&toml).unwrap()
    }

    fn record(at: &[(&str, &str)]) -> ReviewRecord {
        ReviewRecord {
            last_reviewed: "2026-01-01".into(),
            interval_days: 30,
            history: vec![ReviewEntry {
                date: "2026-01-01".into(),
                outcome: Outcome::Unchanged,
                note: None,
                at: at
                    .iter()
                    .map(|(r, s)| (r.to_string(), s.to_string()))
                    .collect(),
            }],
        }
    }

    #[test]
    fn resolve_at_expands_shas_and_rejects_unknown_repos_and_bad_shas() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("src");
        let first = source_repo(&repo);
        let cfg = config(&repo);
        let at = |r: &str, s: &str| vec![(r.to_string(), s.to_string())];
        let got = resolve_at(tmp.path(), &cfg, &at("src", &first[..8])).unwrap();
        assert_eq!(got["src"], first);
        let err = |a: Vec<(String, String)>| {
            format!("{:#}", resolve_at(tmp.path(), &cfg, &a).unwrap_err())
        };
        assert!(err(at("nope", &first)).contains("not a repo"));
        assert!(err(at("src", "deadbeef")).contains("--at src=deadbeef"));
        assert!(err(at("gone", &first)).contains("not a directory"));
        let twice = [at("src", &first), at("src", &first)].concat();
        assert!(err(twice).contains("more than once"));
        assert_eq!(parse_at("a=b"), Ok(("a".into(), "b".into())));
        assert!(parse_at("a").is_err() && parse_at("=b").is_err());
    }

    #[test]
    fn since_counts_commits_up_to_branch_and_narrows_by_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("src");
        let first = source_repo(&repo);
        let cfg = config(&repo);
        let rec = record(&[("src", &first), ("gone", &first), ("removed", &first)]);
        let got = since(tmp.path(), &cfg, &rec, &[]);
        let src = &got["src"];
        assert_eq!(src.commits, Some(2));
        assert_eq!(
            src.paths.as_deref(),
            Some(&["docs/c.md".to_string(), "src/b.rs".to_string()][..])
        );
        assert_eq!(src.branch.as_deref(), Some("origin/master"));
        assert!(src.error.is_none());
        for name in ["gone", "removed"] {
            let g = &got[name];
            assert!(g.commits.is_none() && g.paths.is_none(), "{name}");
            assert!(g.error.is_some(), "{name}");
        }

        let narrowed = since(tmp.path(), &cfg, &rec, &[("src".into(), "src".into())]);
        assert_eq!(narrowed["src"].commits, Some(1));
        assert_eq!(narrowed["src"].sources, vec!["src"]);

        let bad = since(tmp.path(), &cfg, &record(&[("src", "0000000")]), &[]);
        assert!(bad["src"].error.as_deref().unwrap().contains("failed"));
    }
}
