//! Real CLI processes, isolated state and fake origins. No installed binary or broker.
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
fn command(state: &Path, host: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_vega-notify"));
    c.env("XDG_STATE_HOME", state)
        .env("VEGA_POST_HOST", host)
        .env("XDG_CONFIG_HOME", state.join("config"))
        .env("VEGA_NATS_URL", "nats://127.0.0.1:1");
    c
}
fn run(state: &Path, host: &str, args: &[&str]) -> Output {
    command(state, host).args(args).output().unwrap()
}
fn note(state: &Path) -> Value {
    let dir = state.join("vega/notes");
    let path = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn remote(state: &Path, host: &str, id: u64, ack: bool) {
    let mut child = command(state, host)
        .args(["remote-update", "--from", "viewer"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::json!({"id":id,"ack":ack})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
}
#[test]
fn separate_origins_keep_asks_and_command_updates_origin_only() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let args = [
        "action",
        "demo: please test -> w1:p1",
        "--pane",
        "w1:p1",
        "--project",
        "demo",
        "--cmd",
        "test\u{1b}[31m now\u{1b}[0m",
        "--bead",
        "demo-1",
    ];
    for (dir, host) in [(a.path(), "a"), (b.path(), "b")] {
        let out = run(dir, host, &args);
        assert!(out.status.success(), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("delivery pending"));
    }
    let first = note(a.path());
    let id = first["id"].as_u64().unwrap();
    assert_eq!(first["cmd"], "test now");
    assert_eq!(first["cause"], "ask:demo/w1:p1");
    assert_eq!(first["bead"], "demo-1");
    assert!(run(a.path(), "a", &args).status.success());
    assert_eq!(note(a.path())["id"], id);
    remote(a.path(), "a", id, true);
    assert!(note(a.path())["acked"].is_i64());
    assert!(note(b.path())["acked"].is_null());
    let list = run(a.path(), "a", &["list"]);
    assert!(list.status.success());
    let text = String::from_utf8_lossy(&list.stdout);
    assert!(text.contains("asks[1]"), "{text}");
    assert!(text.contains("test now"));
    remote(a.path(), "a", id, false);
    assert!(note(a.path())["resolved"].is_i64());
    assert!(note(b.path())["resolved"].is_null());
    let list = run(a.path(), "a", &[]);
    assert!(String::from_utf8_lossy(&list.stdout).contains("0 open asks"));
    assert!(run(b.path(), "b", &["resolve", "--pane", "w1:p1"])
        .status
        .success());
    assert!(note(b.path())["resolved"].is_i64());
    assert!(run(b.path(), "b", &["resolve", "--key", "demo/w1:p1"])
        .status
        .success());
}
#[test]
fn probes_help_and_usage_do_not_need_context_or_write_state() {
    let dir = tempfile::tempdir().unwrap();
    for flag in ["--version", "-v", "-V", "--skill", "--help"] {
        assert!(run(dir.path(), "a", &[flag]).status.success());
    }
    for cmd in ["info", "action", "urgent", "list", "resolve", "ack", "test"] {
        assert!(run(dir.path(), "a", &[cmd, "--help"]).status.success());
    }
    let out = run(dir.path(), "a", &["test", "toast"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("channel_unavailable"));
    let out = run(
        dir.path(),
        "a",
        &["action", "demo: go -> pane", "--typo", "value"],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(!dir.path().join("vega/notes").exists());
}
