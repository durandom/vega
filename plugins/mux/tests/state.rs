#[path = "../../../crates/vega-herdr/tests/fixtures/own-terminal.rs"]
mod common;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn run(c: &mut Command, input: &[u8]) {
    let mut child = c
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
}

#[test]
fn documented_events_store_only_metadata_and_bound_history() {
    let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-mux"));
    c.args(["hook", "state"]);
    let path = dir.path().join("state/vega/pane-state/local_hc-7437.jsonl");
    // Event fields from https://code.claude.com/docs/en/hooks.md (2026-11-10).
    let samples = [
        json!({"hook_event_name":"SessionStart", "source":"resume", "model":"claude-opus-5"}),
        json!({"hook_event_name":"UserPromptSubmit", "prompt":"PRIVATE_PROMPT"}),
        json!({"hook_event_name":"Stop", "stop_hook_active":false, "last_assistant_message":"PRIVATE_ANSWER\n\nContinue?"}),
        json!({"hook_event_name":"Notification", "message":"PRIVATE_NOTIFICATION", "title":"PRIVATE_TITLE", "notification_type":"permission_prompt"}),
        json!({"hook_event_name":"SessionEnd", "reason":"other"}),
    ];
    for sample in &samples {
        let mut input = sample.clone();
        input["session_id"] = json!("abc123");
        input["transcript_path"] = json!("/PRIVATE_TRANSCRIPT");
        input["cwd"] = json!("/PRIVATE_CWD");
        run(&mut c, input.to_string().as_bytes());
    }
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("PRIVATE"));
    let records: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records.len(), 5);
    for (i, r) in records.iter().enumerate() {
        assert_eq!(r["event"], samples[i]["hook_event_name"]);
        assert_eq!(r["session_id"], "abc123");
        assert!(r["ts"].as_u64().unwrap() > 0);
        assert_eq!(r.get("question").is_some(), i == 2);
        assert_eq!(r.get("source").is_some(), i == 0);
        assert_eq!(r.get("notification_type").is_some(), i == 3);
    }
    assert_eq!(records[0]["source"], "resume");
    assert_eq!(records[2]["question"], true);
    assert_eq!(records[3]["notification_type"], "permission_prompt");
    // Exercise the same writer without paying for 200 terminal discovery processes.
    let mut record = vega_herdr::pane_state::from_input(
        r#"{"hook_event_name":"Stop","session_id":"abc123","last_assistant_message":"Finished."}"#,
        0,
    )
    .unwrap();
    for ts in 0..210 {
        record.ts = ts;
        vega_herdr::pane_state::append(&path, &record).unwrap();
    }
    let text = std::fs::read_to_string(path).unwrap();
    let records: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records.len(), 200);
    assert_eq!(records[0]["ts"], 10);
    assert_eq!(records[199]["ts"], 209);
    assert_eq!(records[199]["question"], false);
}

#[test]
fn errors_fail_open_silently() {
    let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-mux"));
    c.args(["hook", "state"]);
    for input in [
        b"invalid".as_slice(),
        b"{}",
        b"\xff",
        br#"{"session_id":"abc123","hook_event_name":"NotAnEvent"}"#,
    ] {
        run(&mut c, input);
    }
    let input = br#"{"session_id":"abc123","hook_event_name":"Stop"}"#;
    // Unwritable state root and then unverified terminal ancestry.
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, "file").unwrap();
    c.env("XDG_STATE_HOME", &blocked);
    run(&mut c, input);
    c.env("XDG_STATE_HOME", dir.path().join("state"));
    c.env("TEST_PARENT_PID", "4294967295");
    run(&mut c, input);
    assert!(!dir.path().join("state/vega/pane-state").exists());
}
