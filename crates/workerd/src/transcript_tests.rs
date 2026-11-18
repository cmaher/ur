use super::*;
use serde_json::json;
use std::time::UNIX_EPOCH;

fn lines(values: &[Value]) -> String {
    values
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn claude_turns_merge_tool_round_trips_and_skip_sidechains() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let cwd = Path::new("/work/space");
    let dir = home.join(".claude/projects/-work-space");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("old.jsonl"),
        lines(&[json!({"type":"user","message":{"content":"stale"}})]),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let log = lines(&[
        json!({"type":"summary","summary":"x"}),
        json!({"type":"user","message":{"role":"user","content":"fix the bug"}}),
        json!({"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"Looking."}]}}),
        json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test\nmore","description":"run"}}]}}),
        json!({"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}),
        json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}),
        json!({"type":"user","isMeta":true,"message":{"content":"meta"}}),
        json!({"type":"assistant","message":{"content":[{"type":"text","text":"Fixed."}]}}),
    ]) + "\n{\"type\":\"assistant\",\"message\":{\"con";
    std::fs::write(dir.join("live.jsonl"), log).unwrap();

    let t = read_transcript(AgentType::Claude, home, cwd, UNIX_EPOCH, 10).unwrap();
    assert_eq!(t.source_path, dir.join("live.jsonl"));
    assert_eq!(t.turns.len(), 2);
    assert_eq!(t.turns[0].role, "user");
    assert_eq!(t.turns[0].text, "fix the bug");
    assert_eq!(t.turns[1].role, "assistant");
    assert_eq!(t.turns[1].text, "Looking.\n\nFixed.");
    assert_eq!(
        t.turns[1].tool_calls,
        vec![TranscriptToolCall {
            name: "Bash".into(),
            summary: "cargo test".into()
        }]
    );

    let last = read_transcript(AgentType::Claude, home, cwd, UNIX_EPOCH, 1).unwrap();
    assert_eq!(last.turns.len(), 1);
    assert_eq!(last.turns[0].role, "assistant");
}

#[test]
fn codex_picks_rollout_for_cwd_and_skips_context() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let day = home.join(".codex/sessions/2026/11/18");
    std::fs::create_dir_all(&day).unwrap();
    let meta = |cwd: &str| json!({"type":"session_meta","payload":{"id":"s","cwd":cwd}});
    std::fs::write(
        day.join("rollout-a.jsonl"),
        lines(&[
            meta("/workspace"),
            json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"sys"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}),
            json!({"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"ls\"]}"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","output":"x"}}),
            json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n..."}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}),
        ]),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    // Newer, but for a different cwd.
    std::fs::write(day.join("rollout-b.jsonl"), lines(&[meta("/elsewhere")])).unwrap();

    let t = read_transcript(
        AgentType::Codex,
        home,
        Path::new("/workspace"),
        UNIX_EPOCH,
        5,
    )
    .unwrap();
    assert_eq!(t.source_path, day.join("rollout-a.jsonl"));
    assert_eq!(t.turns.len(), 2);
    assert_eq!(t.turns[0].text, "hello");
    assert_eq!(t.turns[1].text, "done");
    let names: Vec<_> = t.turns[1]
        .tool_calls
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["shell", "apply_patch"]);
    assert_eq!(t.turns[1].tool_calls[0].summary, "bash -lc ls");
    assert_eq!(t.turns[1].tool_calls[1].summary, "*** Begin Patch");
}

#[test]
fn missing_empty_and_unsupported_logs_are_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let cwd = Path::new("/workspace");
    assert!(matches!(
        read_transcript(AgentType::Claude, home, cwd, UNIX_EPOCH, 3),
        Err(TranscriptError::NotFound { .. })
    ));
    assert!(matches!(
        read_transcript(AgentType::Codex, home, cwd, UNIX_EPOCH, 3),
        Err(TranscriptError::NotFound { .. })
    ));
    assert_eq!(
        read_transcript(AgentType::Agy, home, cwd, UNIX_EPOCH, 3).unwrap_err(),
        TranscriptError::Unsupported { agent: "agy" }
    );

    let dir = home.join(".claude/projects/-workspace");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s.jsonl"), "not json\n").unwrap();
    assert!(matches!(
        read_transcript(AgentType::Claude, home, cwd, UNIX_EPOCH, 3),
        Err(TranscriptError::Empty { .. })
    ));
}

#[test]
fn summary_truncates_long_input() {
    let long = "x".repeat(300);
    let s = summarize_input(&json!({"other": long}));
    assert_eq!(s.chars().count(), SUMMARY_MAX_CHARS);
    assert!(s.ends_with('…'));
    assert_eq!(summarize_input(&json!({"file_path":"/a/b.rs"})), "/a/b.rs");
}

/// A log last written before this worker started belongs to an earlier
/// worker in the same slot: it is never picked, even as the only file.
#[test]
fn logs_older_than_worker_start_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let cwd = Path::new("/workspace");
    let user = |text: &str| lines(&[json!({"type":"user","message":{"content":text}})]);

    let dir = home.join(".claude/projects/-workspace");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("previous.jsonl"), user("old worker")).unwrap();
    let day = home.join(".codex/sessions/2026/11/18");
    std::fs::create_dir_all(&day).unwrap();
    let meta = json!({"type":"session_meta","payload":{"cwd":"/workspace"}});
    let rollout = |text: &str| {
        lines(&[
            meta.clone(),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}),
        ])
    };
    std::fs::write(day.join("rollout-old.jsonl"), rollout("old worker")).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    let started = SystemTime::now();
    for agent in [AgentType::Claude, AgentType::Codex] {
        assert!(
            matches!(
                read_transcript(agent, home, cwd, started, 3),
                Err(TranscriptError::NotFound { .. })
            ),
            "{agent:?}"
        );
    }

    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(dir.join("current.jsonl"), user("new worker")).unwrap();
    std::fs::write(day.join("rollout-new.jsonl"), rollout("new worker")).unwrap();
    for agent in [AgentType::Claude, AgentType::Codex] {
        let t = read_transcript(agent, home, cwd, started, 3).unwrap();
        assert_eq!(t.turns[0].text, "new worker", "{agent:?}");
    }
}
