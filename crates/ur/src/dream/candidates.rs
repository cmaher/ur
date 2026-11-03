//! `ur dream candidates`: classify active scratch streams for the Promote pass.
//!
//! Combines `refs::scan`, the cached checks in `.dream/state.json` (never re-run here), and
//! `thresholds.quiet_days`. A ref with no cached check counts as unknown (`unchecked`).

use std::collections::BTreeMap;

use serde::Serialize;

use super::check::Source;
use super::refs::{Ref, RefsReport, Stream};
use super::state::State;

/// State given to a ref `state.json` has never recorded.
pub const UNCHECKED: &str = "unchecked";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// Every ref done and quiet for at least the threshold: promote it.
    Candidate,
    /// Some ref's done-ness is unknown (or it was never checked).
    Blocked,
    /// No refs and quiet for at least the threshold: report, never promote.
    PossiblyAbandoned,
    /// Anything else: a ref not done yet, or changed recently.
    Active,
}

/// One ref of a stream with its cached check.
#[derive(Debug, Clone, Serialize)]
pub struct StreamRefState {
    #[serde(rename = "ref")]
    pub key: String,
    pub source: Source,
    pub state: String,
    pub done: Option<bool>,
    /// `None` when the ref was never checked.
    pub checked_at: Option<String>,
}

/// A ref that blocks promotion, with the state that says why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockingRef {
    #[serde(rename = "ref")]
    pub key: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamClass {
    pub classification: Classification,
    pub quiet_days: Option<i64>,
    pub last_change: Option<String>,
    pub refs: Vec<StreamRefState>,
    /// Refs whose `done` is `null` or that were never checked; set only when blocked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<BlockingRef>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct CandidatesSummary {
    pub candidate: usize,
    pub blocked: usize,
    pub possibly_abandoned: usize,
    pub active: usize,
}

/// `ur dream candidates` output.
#[derive(Debug, Serialize)]
pub struct CandidatesReport {
    /// `thresholds.quiet_days` from the config.
    pub quiet_days_threshold: i64,
    pub summary: CandidatesSummary,
    pub streams: BTreeMap<String, StreamClass>,
    /// Streams not classified because their status is not `active`: stream → status.
    pub inactive: BTreeMap<String, String>,
}

/// Classify every active stream in `report` from the cached checks in `state`.
pub fn classify(report: &RefsReport, state: &State, threshold: i64) -> CandidatesReport {
    let mut summary = CandidatesSummary::default();
    let mut streams = BTreeMap::new();
    let mut inactive = BTreeMap::new();
    for (name, stream) in &report.streams {
        if !stream.is_active() {
            inactive.insert(name.clone(), stream.status.clone().unwrap_or_default());
            continue;
        }
        let class = classify_stream(stream, state, threshold);
        match class.classification {
            Classification::Candidate => summary.candidate += 1,
            Classification::Blocked => summary.blocked += 1,
            Classification::PossiblyAbandoned => summary.possibly_abandoned += 1,
            Classification::Active => summary.active += 1,
        }
        streams.insert(name.clone(), class);
    }
    CandidatesReport {
        quiet_days_threshold: threshold,
        summary,
        streams,
        inactive,
    }
}

fn ref_state(r: &Ref, source: Source, state: &State) -> StreamRefState {
    let key = r.key();
    match state.refs.get(&key) {
        Some(c) => StreamRefState {
            key,
            source,
            state: c.state.clone(),
            done: c.done,
            checked_at: Some(c.checked_at.clone()),
        },
        None => StreamRefState {
            key,
            source,
            state: UNCHECKED.to_string(),
            done: None,
            checked_at: None,
        },
    }
}

fn classify_stream(stream: &Stream, state: &State, threshold: i64) -> StreamClass {
    let refs: Vec<StreamRefState> = (stream.recorded_refs.iter().map(|r| (r, Source::Recorded)))
        .chain(
            stream
                .discovered_refs
                .iter()
                .map(|r| (r, Source::Discovered)),
        )
        .map(|(r, source)| ref_state(r, source, state))
        .collect();
    let blocked_by: Vec<BlockingRef> = refs
        .iter()
        .filter(|r| r.done.is_none())
        .map(|r| BlockingRef {
            key: r.key.clone(),
            state: r.state.clone(),
        })
        .collect();
    let quiet = stream.quiet_days.is_some_and(|d| d >= threshold);
    let classification = if !blocked_by.is_empty() {
        Classification::Blocked
    } else if refs.is_empty() {
        if quiet {
            Classification::PossiblyAbandoned
        } else {
            Classification::Active
        }
    } else if quiet && refs.iter().all(|r| r.done == Some(true)) {
        Classification::Candidate
    } else {
        Classification::Active
    };
    StreamClass {
        classification,
        quiet_days: stream.quiet_days,
        last_change: stream.last_change.clone(),
        refs,
        blocked_by,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dream::state::RefState;

    fn r(id: &str) -> Ref {
        Ref {
            kind: "t".into(),
            id: id.into(),
            groups: BTreeMap::new(),
            problem: None,
        }
    }

    fn stream(
        status: Option<&str>,
        recorded: Vec<Ref>,
        discovered: Vec<Ref>,
        quiet: Option<i64>,
    ) -> Stream {
        Stream {
            entry: None,
            frontmatter: status.is_some(),
            status: status.map(str::to_string),
            recorded_refs: recorded,
            discovered_refs: discovered,
            ignored_refs: Vec::new(),
            unclassified: Vec::new(),
            last_change: quiet.map(|_| "2026-01-01".to_string()),
            last_change_source: "git",
            quiet_days: quiet,
        }
    }

    fn state() -> State {
        let mut st = State::default();
        let mut put = |id: &str, s: &str, done: Option<bool>| {
            st.refs.insert(
                format!("t:{id}"),
                RefState {
                    state: s.into(),
                    done,
                    checked_at: "2026-01-01".into(),
                },
            );
        };
        put("T-1", "Done", Some(true));
        put("T-2", "Done", Some(true));
        put("T-3", "In Progress", Some(false));
        put("T-4", "exit-1", None);
        st
    }

    fn report(streams: Vec<(&str, Stream)>) -> RefsReport {
        RefsReport {
            streams: streams
                .into_iter()
                .map(|(n, s)| (n.to_string(), s))
                .collect(),
            active_refs: Vec::new(),
        }
    }

    fn class_of(got: &CandidatesReport, name: &str) -> Classification {
        got.streams[name].classification
    }

    #[test]
    fn classifies_all_four_kinds() {
        let rep = report(vec![
            (
                "working/cand",
                stream(None, vec![r("T-1")], vec![r("T-2")], Some(20)),
            ),
            (
                "working/blocked",
                stream(Some("active"), vec![r("T-1"), r("T-4")], vec![], Some(30)),
            ),
            ("working/abandoned", stream(None, vec![], vec![], Some(14))),
            (
                "working/notdone",
                stream(None, vec![r("T-1"), r("T-3")], vec![], Some(40)),
            ),
            (
                "working/recent",
                stream(None, vec![r("T-1")], vec![], Some(3)),
            ),
            ("working/fresh", stream(None, vec![], vec![], Some(1))),
            ("working/empty", stream(None, vec![], vec![], None)),
            (
                "working/finished",
                stream(Some("done"), vec![r("T-4")], vec![], Some(99)),
            ),
        ]);
        let got = classify(&rep, &state(), 14);
        assert_eq!(class_of(&got, "working/cand"), Classification::Candidate);
        assert_eq!(class_of(&got, "working/blocked"), Classification::Blocked);
        assert_eq!(
            class_of(&got, "working/abandoned"),
            Classification::PossiblyAbandoned
        );
        assert_eq!(class_of(&got, "working/notdone"), Classification::Active);
        assert_eq!(class_of(&got, "working/recent"), Classification::Active);
        assert_eq!(class_of(&got, "working/fresh"), Classification::Active);
        assert_eq!(class_of(&got, "working/empty"), Classification::Active);
        assert_eq!(
            got.streams["working/blocked"].blocked_by,
            vec![BlockingRef {
                key: "t:T-4".into(),
                state: "exit-1".into()
            }]
        );
        assert!(!got.streams.contains_key("working/finished"));
        assert_eq!(got.inactive["working/finished"], "done");
        assert_eq!(
            got.summary,
            CandidatesSummary {
                candidate: 1,
                blocked: 1,
                possibly_abandoned: 1,
                active: 4,
            }
        );
    }

    #[test]
    fn unchecked_ref_blocks_even_when_quiet() {
        let rep = report(vec![(
            "working/s",
            stream(None, vec![r("T-1")], vec![r("T-9")], Some(50)),
        )]);
        let got = classify(&rep, &state(), 14);
        let s = &got.streams["working/s"];
        assert_eq!(s.classification, Classification::Blocked);
        assert_eq!(s.blocked_by[0].key, "t:T-9");
        assert_eq!(s.blocked_by[0].state, UNCHECKED);
        assert_eq!(s.refs[1].source, Source::Discovered);
        assert_eq!(s.refs[1].checked_at, None);
    }

    #[test]
    fn blocked_wins_over_not_done_and_json_shape() {
        let rep = report(vec![(
            "working/s",
            stream(None, vec![r("T-3"), r("T-4")], vec![], Some(2)),
        )]);
        let got = classify(&rep, &state(), 14);
        assert_eq!(class_of(&got, "working/s"), Classification::Blocked);
        let json = serde_json::to_value(&got).unwrap();
        assert_eq!(json["quiet_days_threshold"], 14);
        let s = &json["streams"]["working/s"];
        assert_eq!(s["classification"], "blocked");
        assert_eq!(s["blocked_by"][0]["ref"], "t:T-4");
        assert_eq!(s["refs"][0]["source"], "recorded");
        assert_eq!(json["summary"]["possibly_abandoned"], 0);
    }
}
