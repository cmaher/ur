//! `ur dream check` / `ur dream try`: run declared check commands and classify the result.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::Local;
use regex::Regex;
use serde::Serialize;

use super::config::{Config, fill, fill_escaped};
use super::refs::Ref;
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
    use crate::dream::refs::parse_ref;

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
}
