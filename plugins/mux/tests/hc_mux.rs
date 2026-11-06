use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
};
struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new() -> Self {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        fs::create_dir(&bin).unwrap();
        for (name, text) in [("hc",include_str!("fixtures/hc.py")),("herdr-app","#!/bin/sh\necho \"$*\" >> \"$HOME/app-calls\"\necho '{\"space\":\"s2\"}'\n"),("herdr","#!/bin/sh\necho herdr-called >> \"$HOME/herdr-calls\"\necho '{\"error\":{\"code\":\"agent_not_found\"}}'\nexit 1\n")] {
            fs::write(bin.join(name),text).unwrap();fs::set_permissions(bin.join(name),fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self(d)
    }
    fn root(&self) -> &Path {
        self.0.path()
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_vega-mux"));
        c.env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root().join("bin").display()),
            )
            .env("HOME", self.root())
            .env("XDG_STATE_HOME", self.root().join("state"))
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("HC_TERMINAL_ID", "t7")
            .env("TEST_PARENT_PID", std::process::id().to_string())
            .env("VEGA_POST_HOST", "test")
            .env("VEGA_TOKEN_BURN", "/nonexistent/token-burn")
            .env("VEGA_NATS_URL", "nats://127.0.0.1:1")
            .current_dir(self.root());
        c
    }
    fn run(&self, args: &[&str]) -> String {
        let o = self.cmd().args(args).output().unwrap();
        success(o)
    }
    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.root().join("calls"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    fn spawn(&self) {
        self.run(&[
            "spawn",
            "worker",
            "--kind",
            "codex",
            "--prompt",
            "do the task",
            "--env",
            "SPECIAL=a b $literal",
            "--",
            "-m",
            "chosen",
        ]);
    }
}
fn success(o: Output) -> String {
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}
#[test]
fn hc_spawn_show_send_capture_close_reopen_and_transcript() {
    let f = Fixture::new();
    f.spawn();
    let calls = f.calls();
    let run = calls.iter().find(|v| v[0] == "run").unwrap();
    let a = run.as_array().unwrap();
    for item in [
        "--bg",
        "--outlive-parent",
        "herdr.space=s1",
        "vega.mode=headed",
        "--no-daemon",
        "SPECIAL=a b $literal",
    ] {
        assert!(a.iter().any(|v| v == item), "{run}");
    }
    assert!(!a.iter().any(|v| v == "--no-notify"));
    assert!(a
        .last()
        .unwrap()
        .as_str()
        .unwrap()
        .contains("vega post done"));
    assert!(f.run(&["show", "worker"]).contains("recent output"));
    f.run(&["send", "hc:t8", "hello", "--now"]);
    success(
        f.cmd()
            .env("HC_TERMINAL_ID", "t8")
            .env("CODEX_THREAD_ID", "session-8")
            .args(["show", "t8"])
            .output()
            .unwrap(),
    );
    let log = f.root().join("session.jsonl");
    fs::write(&log,"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"finished task\"}]}}\n").unwrap();
    assert!(f
        .run(&[
            "transcript",
            "t8",
            "--session",
            log.to_str().unwrap(),
            "--last"
        ])
        .contains("finished task"));
    f.run(&["close", "worker"]);
    f.run(&["close", "hc:t8"]);
    f.run(&["reopen", "worker"]);
    let calls = f.calls();
    let run = calls.iter().rev().find(|v| v[0] == "run").unwrap();
    assert!(run
        .as_array()
        .unwrap()
        .windows(2)
        .any(|w| w[0] == "resume" && w[1] == "session-8"));
    assert!(!f.root().join("herdr-calls").exists());
}
#[test]
fn explicit_hc_target_from_tui_and_self_close_refusal() {
    let f = Fixture::new();
    f.spawn();
    success(
        f.cmd()
            .env("HERDR_PANE_ID", "w1:p1")
            .args(["show", "hc:t8"])
            .output()
            .unwrap(),
    );
    let o = f.cmd().args(["close", "t7"]).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("self_close"));
    assert!(!f.calls().iter().any(|v| v[0] == "kill"));
}
#[test]
fn blocked_send_and_trust_answer_never_force_or_press_keys() {
    let f = Fixture::new();
    f.spawn();
    let path = f.root().join("terminals.json");
    let mut ts: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    ts["t8"]["status"]["state"] = "blocked".into();
    fs::write(path, ts.to_string()).unwrap();
    let o = f
        .cmd()
        .args(["send", "t8", "go", "--now"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    let o = f
        .cmd()
        .env("TEST_DIALOG", "trust")
        .args(["answer", "t8", "--option", "1"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(!f
        .calls()
        .iter()
        .any(|v| v[0] == "keys" || v.as_array().unwrap().iter().any(|s| s == "--force")));
}
#[test]
fn hc_next_focuses_through_app() {
    let f = Fixture::new();
    f.spawn();
    let state = f.root().join("state/vega");
    fs::create_dir_all(&state).unwrap();
    // Ledger filename/shape is owned by the shared waiting module.
    let ledger = serde_json::json!({"panes":{"t8":{"status":"done","since":1}}});
    fs::write(state.join("waiting.json"), ledger.to_string()).unwrap();
    f.run(&["next"]);
    assert!(fs::read_to_string(f.root().join("app-calls"))
        .unwrap()
        .contains("agent.focus --terminal t8"));
}

#[test]
fn hc_dialog_answer_rechecks_and_moves_question() {
    let f = Fixture::new();
    f.spawn();
    let path = f.root().join("terminals.json");
    let mut ts: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    ts["t8"]["status"]["state"] = "blocked".into();
    fs::write(path, ts.to_string()).unwrap();
    let out = success(
        f.cmd()
            .env("TEST_DIALOG", "question")
            .args(["dialog", "t8", "--json"])
            .output()
            .unwrap(),
    );
    assert!(out.contains("question"), "{out}");
    success(
        f.cmd()
            .env("TEST_DIALOG", "question")
            .args(["answer", "t8", "--option", "2"])
            .output()
            .unwrap(),
    );
    let keys: Vec<_> = f.calls().into_iter().filter(|v| v[0] == "keys").collect();
    assert_eq!(
        keys,
        vec![
            serde_json::json!(["keys", "t8", "down"]),
            serde_json::json!(["keys", "t8", "enter"])
        ]
    );
}

#[test]
fn hc_backend_override_and_remote_direction_are_checked_before_run() {
    let f = Fixture::new();
    success(
        f.cmd()
            .env_remove("HC_TERMINAL_ID")
            .args([
                "spawn",
                "worker",
                "--backend",
                "hc",
                "--kind",
                "codex",
                "--no-parent",
            ])
            .output()
            .unwrap(),
    );
    let o = f
        .cmd()
        .env("VEGA_POST_HOST", "deimos")
        .args([
            "spawn",
            "remote",
            "--kind",
            "codex",
            "--no-parent",
            "--machine",
            "private-host",
            "--cwd",
            "/repo",
        ])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("machine_direction"));
    assert_eq!(f.calls().iter().filter(|v| v[0] == "run").count(), 1);
}

#[test]
fn explicitly_undelivered_brief_can_resume_without_spawning_twice() {
    let f = Fixture::new();
    let o = f
        .cmd()
        .env("TEST_UNDELIVERED", "1")
        .args([
            "spawn",
            "worker",
            "--kind",
            "codex",
            "--headless",
            "--prompt",
            "saved task",
            "--",
            "-m",
            "chosen",
        ])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("brief_not_delivered"));
    f.run(&["spawn", "--resume", "t8"]);
    let calls = f.calls();
    assert_eq!(calls.iter().filter(|v| v[0] == "run").count(), 1);
    let sent = calls.iter().find(|v| v[0] == "send").unwrap();
    assert!(sent[2].as_str().unwrap().contains("saved task"));
    assert!(f.run(&["mode", "t8"]).contains("headless"));
}
