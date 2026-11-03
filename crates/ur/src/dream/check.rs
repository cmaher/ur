//! `ur dream check` / `ur dream try`: run declared check commands and classify the result.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::Local;
use regex::Regex;
use serde::Serialize;

use super::config::{Config, fill, fill_escaped};
use super::refs::{Ref, RefsReport};
use super::state::{RefState, State};

const TIMEOUT: Duration = Duration::from_secs(60);
const WORKERS: usize = 8;

#[derive(Debug, Clone, Serialize)]
pub struct Checked {
    pub state: String,
    pub done: Option<bool>,
    pub checked_at: String,
    /// Reused from `state.json` (already done), not re-run.
    pub cached: bool,
}

/// How a ref came to belong to a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Listed in the stream's frontmatter `refs`.
    Recorded,
    /// Found in the stream's text.
    Discovered,
}

/// One active stream a ref belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct StreamRef {
    pub stream: String,
    pub source: Source,
}

/// A checked ref and the active streams it came from.
#[derive(Debug, Clone, Serialize)]
pub struct RefResult {
    #[serde(flatten)]
    pub checked: Checked,
    pub streams: Vec<StreamRef>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub total: usize,
    pub done: usize,
    pub not_done: usize,
    /// `done` is `null`: the check failed or the state is not declared done or not done.
    pub unknown: usize,
    /// Reused from `state.json` rather than re-run (counted in `done` too).
    pub cached: usize,
}

/// A ref whose done-ness is unknown, with the state that says why.
#[derive(Debug, Clone, Serialize)]
pub struct UnknownRef {
    #[serde(rename = "ref")]
    pub key: String,
    pub state: String,
    pub streams: Vec<StreamRef>,
}

/// `ur dream check` output: totals, the unknown refs, and every ref's detail.
#[derive(Debug, Serialize)]
pub struct CheckReport {
    pub summary: Summary,
    pub unknown: Vec<UnknownRef>,
    pub refs: BTreeMap<String, RefResult>,
}

/// Ref key → the active streams that record or discover it, sorted by stream.
pub fn attribution(report: &RefsReport) -> BTreeMap<String, Vec<StreamRef>> {
    let mut out: BTreeMap<String, Vec<StreamRef>> = BTreeMap::new();
    for (name, stream) in report.streams.iter().filter(|(_, s)| s.is_active()) {
        let tagged = (stream.recorded_refs.iter().map(|r| (r, Source::Recorded))).chain(
            stream
                .discovered_refs
                .iter()
                .map(|r| (r, Source::Discovered)),
        );
        for (r, source) in tagged {
            out.entry(r.key()).or_default().push(StreamRef {
                stream: name.clone(),
                source,
            });
        }
    }
    out
}

/// Join check results with stream attribution and count them.
pub fn summarize(
    results: &BTreeMap<String, Checked>,
    streams: &BTreeMap<String, Vec<StreamRef>>,
) -> CheckReport {
    let mut summary = Summary::default();
    let mut unknown = Vec::new();
    let mut refs = BTreeMap::new();
    for (key, checked) in results {
        let attributed = streams.get(key).cloned().unwrap_or_default();
        summary.total += 1;
        summary.cached += usize::from(checked.cached);
        match checked.done {
            Some(true) => summary.done += 1,
            Some(false) => summary.not_done += 1,
            None => {
                summary.unknown += 1;
                unknown.push(UnknownRef {
                    key: key.clone(),
                    state: checked.state.clone(),
                    streams: attributed.clone(),
                });
            }
        }
        let result = RefResult {
            checked: checked.clone(),
            streams: attributed,
        };
        refs.insert(key.clone(), result);
    }
    CheckReport {
        summary,
        unknown,
        refs,
    }
}

/// The argv a ref's check would run, or `None` if the kind has no check or a placeholder
/// has no value.
pub fn argv(cfg: &Config, r: &Ref) -> Option<Vec<String>> {
    let check = cfg.kind(&r.kind)?.check.as_ref()?;
    check.iter().map(|a| fill(a, &r.groups)).collect()
}

/// `(state, done)` for one ref; `done` is `None` (unknown) for anything not declared done or
/// not done. Never fails: errors become states like `missing-command` or `exit-1`.
pub fn run_one(cfg: &Config, r: &Ref) -> (String, Option<bool>) {
    let Some(kind) = cfg.kind(&r.kind) else {
        return ("unknown-kind".into(), None);
    };
    if kind.check.is_none() {
        return ("uncheckable".into(), None);
    }
    let Some(argv) = argv(cfg, r) else {
        return ("unparsed".into(), None);
    };
    let stdout = match run(&argv) {
        Ok(out) => out,
        Err(state) => return (state, None),
    };
    let state = match &kind.state {
        Some(pattern) => match extract(pattern, &r.groups, &stdout) {
            Some(s) => s,
            None => return ("unrecognized".into(), None),
        },
        None => stdout.lines().next().unwrap_or("").trim().to_string(),
    };
    let done = if kind.done.contains(&state) {
        Some(true)
    } else if kind.not_done.contains(&state) {
        Some(false)
    } else {
        None
    };
    (state, done)
}

fn extract(pattern: &str, groups: &BTreeMap<String, String>, stdout: &str) -> Option<String> {
    let re = Regex::new(&format!("(?m){}", fill_escaped(pattern, groups))).ok()?;
    let caps = re.captures(stdout)?;
    let m = caps.get(1).or_else(|| caps.get(0))?;
    Some(m.as_str().trim().to_string())
}

/// Run argv with a timeout; `Ok(stdout)` on exit 0, else `Err(state)`.
fn run(argv: &[String]) -> Result<String, String> {
    let mut child = match Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("missing-command".into()),
        Err(_) => return Err("spawn-failed".into()),
    };
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timeout".into());
            }
        }
    };
    let out = reader.join().unwrap_or_default();
    match status.code() {
        Some(0) => Ok(out.trim().to_string()),
        Some(code) => Err(format!("exit-{code}")),
        None => Err("killed".into()),
    }
}

/// Check every ref not already cached as done, in parallel.
pub fn check_all(cfg: &Config, refs: &[Ref], state: &State) -> BTreeMap<String, Checked> {
    let today = Local::now().date_naive().to_string();
    let mut out = BTreeMap::new();
    let mut todo = Vec::new();
    for r in refs {
        match state.refs.get(&r.key()) {
            Some(c) if c.done == Some(true) => {
                out.insert(
                    r.key(),
                    Checked {
                        state: c.state.clone(),
                        done: c.done,
                        checked_at: c.checked_at.clone(),
                        cached: true,
                    },
                );
            }
            _ => todo.push(r),
        }
    }
    for chunk in todo.chunks(WORKERS) {
        let results: Vec<(String, Option<bool>)> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk.iter().map(|r| s.spawn(|| run_one(cfg, r))).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (r, (st, done)) in chunk.iter().zip(results) {
            let checked = Checked {
                state: st,
                done,
                checked_at: today.clone(),
                cached: false,
            };
            out.insert(r.key(), checked);
        }
    }
    out
}

pub fn record(state: &mut State, results: &BTreeMap<String, Checked>) {
    for (key, c) in results {
        state.refs.insert(
            key.clone(),
            RefState {
                state: c.state.clone(),
                done: c.done,
                checked_at: c.checked_at.clone(),
            },
        );
    }
    state.last_run = Some(Local::now().date_naive().to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dream::config::Config;
    use crate::dream::refs::{RefsReport, Stream, parse_ref};

    /// A config whose checks are `sh -c` scripts, so tests need no fake binaries.
    fn cfg() -> Config {
        Config::parse(
            r#"```toml
[[ref_kinds]]
kind = "t"
id = "{key}"
patterns = ['(?P<key>T-\d+)']
check = ["sh", "-c", "case {key} in T-1) echo 'T-1  Bug  Done';; T-2) echo 'T-2  Bug  In Progress';; T-3) exit 3;; *) echo 'T-9  Bug  Weird';; esac"]
state = '^{key}\s+\S+\s+(.+)$'
done = ["Done"]
not_done = ["In Progress"]

[[ref_kinds]]
kind = "plain"
patterns = ['P-\d+']
check = ["sh", "-c", "echo MERGED"]
done = ["MERGED"]

[[ref_kinds]]
kind = "nocheck"
patterns = ['N-\d+']

[[ref_kinds]]
kind = "missing"
patterns = ['M-\d+']
check = ["ur-dream-test-no-such-binary"]
done = ["x"]
```"#,
        )
        .unwrap()
    }

    fn one(kind: &str, v: &str) -> (String, Option<bool>) {
        let c = cfg();
        run_one(&c, &parse_ref(&c, kind, v))
    }

    #[test]
    fn classifies_done_not_done_and_unknown() {
        assert_eq!(one("t", "T-1"), ("Done".into(), Some(true)));
        assert_eq!(one("t", "T-2"), ("In Progress".into(), Some(false)));
        assert_eq!(one("t", "T-3"), ("exit-3".into(), None));
        // Output for a different id doesn't match the {key}-anchored state regex.
        assert_eq!(one("t", "T-4"), ("unrecognized".into(), None));
        assert_eq!(one("plain", "P-1"), ("MERGED".into(), Some(true)));
        assert_eq!(one("nocheck", "N-1"), ("uncheckable".into(), None));
        assert_eq!(one("missing", "M-1"), ("missing-command".into(), None));
    }

    #[test]
    fn cached_done_refs_are_not_rerun() {
        let c = cfg();
        let refs = vec![parse_ref(&c, "t", "T-2"), parse_ref(&c, "t", "T-1")];
        let mut state = State::default();
        state.refs.insert(
            "t:T-2".into(),
            RefState {
                state: "Done".into(),
                done: Some(true),
                checked_at: "2026-01-01".into(),
            },
        );
        let got = check_all(&c, &refs, &state);
        assert!(got["t:T-2"].cached);
        assert_eq!(got["t:T-2"].checked_at, "2026-01-01");
        assert!(!got["t:T-1"].cached);
        assert_eq!(got["t:T-1"].done, Some(true));
    }

    fn stream(status: Option<&str>, recorded: Vec<Ref>, discovered: Vec<Ref>) -> Stream {
        Stream {
            entry: None,
            frontmatter: status.is_some(),
            status: status.map(str::to_string),
            recorded_refs: recorded,
            discovered_refs: discovered,
            ignored_refs: Vec::new(),
            unclassified: Vec::new(),
            last_change: None,
            last_change_source: "empty",
            quiet_days: None,
        }
    }

    fn checked(state: &str, done: Option<bool>, cached: bool) -> Checked {
        Checked {
            state: state.into(),
            done,
            checked_at: "2026-01-01".into(),
            cached,
        }
    }

    fn sr(stream: &str, source: Source) -> StreamRef {
        StreamRef {
            stream: stream.into(),
            source,
        }
    }

    #[test]
    fn attribution_lists_active_streams_with_source() {
        let c = cfg();
        let t1 = || parse_ref(&c, "t", "T-1");
        let t2 = || parse_ref(&c, "t", "T-2");
        let mut streams = BTreeMap::new();
        streams.insert("working/a".into(), stream(None, vec![t1()], vec![t2()]));
        streams.insert(
            "working/b".into(),
            stream(Some("active"), vec![], vec![t1()]),
        );
        streams.insert("working/c".into(), stream(Some("done"), vec![t2()], vec![]));
        let report = RefsReport {
            streams,
            active_refs: vec![t1(), t2()],
        };
        let got = attribution(&report);
        assert_eq!(
            got["t:T-1"],
            vec![
                sr("working/a", Source::Recorded),
                sr("working/b", Source::Discovered)
            ]
        );
        // The done stream's recorded T-2 is not attributed: only active streams are checked.
        assert_eq!(got["t:T-2"], vec![sr("working/a", Source::Discovered)]);
    }

    #[test]
    fn summarize_counts_and_lists_unknown_refs() {
        let mut results = BTreeMap::new();
        results.insert("t:T-1".to_string(), checked("Done", Some(true), true));
        results.insert(
            "t:T-2".to_string(),
            checked("In Progress", Some(false), false),
        );
        results.insert("t:T-3".to_string(), checked("exit-3", None, false));
        results.insert(
            "plain:P-1".to_string(),
            checked("MERGED", Some(true), false),
        );
        let mut streams = BTreeMap::new();
        streams.insert("t:T-3".to_string(), vec![sr("working/a", Source::Recorded)]);
        let report = summarize(&results, &streams);
        assert_eq!(
            report.summary,
            Summary {
                total: 4,
                done: 2,
                not_done: 1,
                unknown: 1,
                cached: 1,
            }
        );
        assert_eq!(report.unknown.len(), 1);
        assert_eq!(report.unknown[0].key, "t:T-3");
        assert_eq!(report.unknown[0].state, "exit-3");
        assert_eq!(report.unknown[0].streams, streams["t:T-3"]);
        assert!(report.refs["t:T-1"].streams.is_empty());

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["unknown"][0]["ref"], "t:T-3");
        assert_eq!(json["refs"]["t:T-3"]["state"], "exit-3");
        assert_eq!(json["refs"]["t:T-3"]["streams"][0]["source"], "recorded");
        assert_eq!(json["summary"]["not_done"], 1);
    }
}
