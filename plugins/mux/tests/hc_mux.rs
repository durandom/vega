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

#[test]
fn qualified_mail_keeps_its_machine_and_never_targets_a_local_collision() {
    let f = Fixture::new();
    f.spawn();
    let path = f.root().join("terminals.json");
    let mut terminals: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut remote = terminals["t8"].clone();
    remote["address"] = "corp/t8".into();
    remote["name"] = "remote".into();
    terminals["corp/t8"] = remote;
    fs::write(path, terminals.to_string()).unwrap();
    let post = f.root().join("bin/vega");
    fs::write(&post, include_str!("fixtures/post.py")).unwrap();
    fs::set_permissions(&post, fs::Permissions::from_mode(0o755)).unwrap();
    let state = f.root().join("state/vega");
    fs::create_dir_all(&state).unwrap();
    let relay = fs::File::create(state.join("post-relay-test.lock")).unwrap();
    relay.lock().unwrap();
    let out = f
        .cmd()
        .args(["send", "corp/t8", "remote mail"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!f.root().join("mail-local-t8").exists());
    let calls: Vec<Value> = fs::read_to_string(f.root().join("post-calls"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        calls[0],
        serde_json::json!(["post", "send", "corp/t8", "remote mail"])
    );
    f.run(&["--machine", "corp", "send", "t8", "explicit host"]);
    assert_eq!(
        fs::read_to_string(f.root().join("mail-corp-t8")).unwrap(),
        "explicit host"
    );
    assert!(!f.root().join("mail-local-t8").exists());
    f.run(&["send", "hc:corp/t8", "direct remote", "--now"]);
    let calls = f.calls();
    let direct = calls.iter().rev().find(|v| v[0] == "send").unwrap();
    assert_eq!(direct[1], "corp/t8");
    assert!(direct[2]
        .as_str()
        .unwrap()
        .contains("vega post send --host test t7"));
}

#[test]
fn hc_worktree_uses_wt_path_and_the_created_app_space() {
    let f = Fixture::new();
    let tree = f.root().join("worktree");
    fs::create_dir(&tree).unwrap();
    let wt = f.root().join("bin/wt");
    fs::write(
        &wt,
        r#"#!/usr/bin/python3
import json, os, sys
from pathlib import Path
root=Path(os.environ['HOME'])
with open(root/'wt-calls','a') as f: f.write(json.dumps(sys.argv[1:])+'\n')
if sys.argv[1:] == ['--version']: print('fixture wt')
else: print(json.dumps({'path':str(root/'worktree'),'action':'created','base_branch':'main'}))
"#,
    )
    .unwrap();
    fs::set_permissions(&wt, fs::Permissions::from_mode(0o755)).unwrap();
    f.run(&[
        "spawn",
        "worker",
        "--kind",
        "codex",
        "--worktree",
        "fix-branch",
        "--base",
        "main",
        "--",
        "-m",
        "chosen",
    ]);
    let calls = f.calls();
    let run = calls
        .iter()
        .find(|v| v[0] == "run")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(run
        .windows(2)
        .any(|w| w[0] == "--cwd" && w[1] == tree.to_str().unwrap()));
    assert!(run
        .windows(2)
        .any(|w| w[0] == "--label" && w[1] == "herdr.space=s2"));
    let app = fs::read_to_string(f.root().join("app-calls")).unwrap();
    assert!(
        app.contains(&format!(
            "space.new --name fix-branch --folder {}",
            tree.display()
        )),
        "{app}"
    );
    let wt_calls: Vec<Value> = fs::read_to_string(f.root().join("wt-calls"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(wt_calls.iter().any(|c| c
        .as_array()
        .unwrap()
        .windows(2)
        .any(|w| w[0] == "--base" && w[1] == "main")));
}

#[test]
fn remote_hc_spawn_routes_to_machine_without_local_space_label() {
    let f = Fixture::new();
    let out = success(
        f.cmd()
            .env("TEST_MACHINES", r#"{"machines":[{"name":"remote","host":"remote","target":"user@remote","via":false}]}"#)
            .args(["spawn", "worker", "--kind", "codex", "--machine", "remote", "--cwd", "/remote/repo", "--", "-m", "chosen"])
            .output().unwrap(),
    );
    assert!(out.contains("remote/t8"), "{out}");
    let calls = f.calls();
    let run = calls
        .iter()
        .find(|v| v[0] == "run")
        .unwrap()
        .as_array()
        .unwrap();
    let separator = run.iter().position(|v| v == "--").unwrap();
    assert!(run[..separator]
        .windows(2)
        .any(|w| w[0] == "--on" && w[1] == "remote"));
    assert!(run
        .windows(2)
        .any(|w| w[0] == "--cwd" && w[1] == "/remote/repo"));
    assert!(!run
        .iter()
        .any(|v| v.as_str().is_some_and(|s| s.starts_with("herdr.space="))));
    for label in ["vega.kind=codex", "vega.mode=headed", "vega.role=sentinel"] {
        assert!(run.windows(2).any(|w| w[0] == "--label" && w[1] == label));
    }
    assert!(!f.root().join("app-calls").exists());
}

#[test]
fn hc_home_filters_space_and_finished_rows_and_reports_empty_state() {
    let f = Fixture::new();
    f.spawn();
    let path = f.root().join("terminals.json");
    let mut ts: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut other = ts["t8"].clone();
    other["address"] = "t9".into();
    other["name"] = "other-space".into();
    other["labels"]["herdr.space"] = "s2".into();
    ts["t9"] = other;
    let mut finished = ts["t8"].clone();
    finished["address"] = "ta".into();
    finished["name"] = "finished-worker".into();
    finished["state"] = "finished".into();
    ts["ta"] = finished;
    fs::write(&path, ts.to_string()).unwrap();

    let home = success(f.cmd().output().unwrap());
    assert!(home.contains("agents[2]"), "{home}");
    assert!(home.contains("t7,codex,parent,"), "{home}");
    assert!(home.contains("t8,codex,worker,"), "{home}");
    assert!(
        !home.contains("other-space") && !home.contains("finished-worker"),
        "{home}"
    );
    let all = f.run(&["--all"]);
    assert!(all.contains("agents[3]"), "{all}");
    assert!(all.contains("t9,codex,other-space,"), "{all}");
    assert!(!all.contains("finished-worker"), "{all}");

    ts["t8"]["state"] = "finished".into();
    fs::write(&path, ts.to_string()).unwrap();
    // A snapshot may omit the caller while get still verifies its identity.
    let empty = success(f.cmd().env("TEST_HIDE_PARENT", "1").output().unwrap());
    assert!(empty.contains("agents: 0 live agents"), "{empty}");
    ts["t9"]["state"] = "finished".into();
    fs::write(&path, ts.to_string()).unwrap();
    let empty_all = success(
        f.cmd()
            .env("TEST_HIDE_PARENT", "1")
            .arg("--all")
            .output()
            .unwrap(),
    );
    assert!(empty_all.contains("agents: 0 live agents"), "{empty_all}");
}

#[test]
fn hc_home_rejects_machine_scope_for_hc_and_overridden_tui_callers() {
    for tui in [false, true] {
        for all in [false, true] {
            let f = Fixture::new();
            let mut cmd = f.cmd();
            cmd.args(["--machine", "corp"]);
            if tui {
                cmd.env_remove("HC_TERMINAL_ID")
                    .env("HERDR_PANE_ID", "w1:p1")
                    .args(["--backend", "hc"]);
            }
            if all {
                cmd.arg("--all");
            }
            let out = cmd.output().unwrap();
            let stdout = String::from_utf8(out.stdout).unwrap();
            assert_eq!(out.status.code(), Some(1), "{stdout}");
            assert!(stdout.contains("error{code,message,hint}:"), "{stdout}");
            assert!(stdout.contains("unsupported_scope"), "{stdout}");
            assert!(
                stdout.contains("run `vega mux` on that machine"),
                "{stdout}"
            );
            assert!(!stdout.contains("agents["), "{stdout}");
            assert!(!stdout.contains("agents: 0"), "{stdout}");
            let calls = fs::read_to_string(f.root().join("calls")).unwrap_or_default();
            assert!(
                !calls.lines().any(|line| {
                    let call: Value = serde_json::from_str(line).unwrap();
                    call[0] == "ls"
                }),
                "{calls}"
            );
            assert!(!f.root().join("herdr-calls").exists());
        }
    }
}
