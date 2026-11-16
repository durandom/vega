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
        for (name, text) in [("renice","#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/renice-calls\"\n"),("hc",include_str!("fixtures/hc.py")),("herdr-app",include_str!("fixtures/app.py")),("herdr","#!/bin/sh\necho herdr-called >> \"$HOME/herdr-calls\"\necho '{\"error\":{\"code\":\"agent_not_found\"}}'\nexit 1\n"),("lanista","#!/bin/sh\necho '{\"overlay\":{\"loaded\":false,\"route_count\":0},\"routes\":{}}'\n")] {
            fs::write(bin.join(name),text).unwrap();fs::set_permissions(bin.join(name),fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::create_dir_all(d.path().join("config/herdr-app")).unwrap();
        fs::write(d.path().join("config/herdr-app/herdr-app.sock"), "").unwrap();
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
            .env("VEGA_LANISTA_BIN", self.root().join("bin/lanista"))
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
fn broken_python_fake_results_fail_the_wire_contract() {
    for case in ["missing", "wrapped"] {
        let f = Fixture::new();
        let out = f
            .cmd()
            .env_remove("HC_TERMINAL_ID")
            .env("TEST_BROKEN", case)
            .args(["show", "hc:t7"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "broken {case} fake was accepted");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains("hc_bad_output") && text.contains("required field missing"),
            "{text}"
        );
    }
}
#[test]
fn hc_spawn_show_send_capture_close_reopen_and_transcript() {
    let f = Fixture::new();
    f.spawn();
    assert_eq!(
        fs::read_to_string(f.root().join("renice-calls")).unwrap(),
        format!("10 -p {}\n", std::process::id())
    );
    let calls = f.calls();
    let run = calls.iter().find(|v| v[0] == "run").unwrap();
    let p = &run[1];
    assert_eq!(p["argv"][0], "codex");
    assert_eq!(p["outlive_parent"], true);
    assert_eq!(p["wait"], false);
    assert_eq!(p["caller"], "t7");
    assert_eq!(p["labels"]["herdr.space"], "s1");
    assert_eq!(p["labels"]["vega.mode"], "headed");
    assert_eq!(p["env"]["SPECIAL"], "a b $literal");
    assert_eq!(p["env"]["HERDR_PANE_ID"], "");
    assert_eq!(p["env"]["HERDR_SOCKET_PATH"], "");
    let a = p["argv"].as_array().unwrap();
    assert!(a.iter().any(|v| v == "--no-daemon"));
    assert!(!a.iter().any(|v| v == "env" || v == "nice"));
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
    assert!(run[1]["argv"]
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
fn next_on_an_inbox_only_host_skips_the_registry_without_a_warning() {
    let f = Fixture::new();
    f.spawn();
    let run = |inbox_only: bool| {
        let mut c = f.cmd();
        if inbox_only {
            c.env("VEGA_POST_HOST", "puck")
                .env("VEGA_POST_INBOX_ONLY", "puck");
        }
        c.arg("next").output().unwrap()
    };
    let skipped = run(true);
    let err = String::from_utf8_lossy(&skipped.stderr);
    assert!(!err.contains("warning: registry:"), "{err}");
    assert!(String::from_utf8_lossy(&skipped.stdout).contains("none:"));
    // An owner host still says that the registry is unreadable.
    let owner = run(false);
    assert!(String::from_utf8_lossy(&owner.stderr).contains("warning: registry:"));
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
        .env("VEGA_POST_HOST", "puck")
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
fn qualified_mail_preserves_the_machine_host_convention() {
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
    success(out);
    assert_eq!(
        fs::read_to_string(f.root().join("mail-corp-t8")).unwrap(),
        "remote mail"
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
    let scoped = f
        .cmd()
        .args(["--machine", "corp", "send", "t8", "scoped mail"])
        .output()
        .unwrap();
    success(scoped);
    assert_eq!(
        fs::read_to_string(f.root().join("mail-corp-t8")).unwrap(),
        "scoped mail"
    );
    assert!(!f.root().join("mail-local-t8").exists());
    drop(relay);
    f.run(&["send", "corp/t8", "remote without local relay"]);
    assert_eq!(
        fs::read_to_string(f.root().join("mail-corp-t8")).unwrap(),
        "remote without local relay"
    );
    f.run(&["send", "hc:corp/t8", "direct remote", "--now"]);
    let calls = f.calls();
    let direct = calls.iter().rev().find(|v| v[0] == "send").unwrap();
    assert_eq!(direct[1], "corp/t8");
    assert!(direct[2]
        .as_str()
        .unwrap()
        .contains("vega post send --host test t7"));
}

fn fake_worktree(f: &Fixture) {
    fs::create_dir(f.root().join("worktree")).unwrap();
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
    fs::set_permissions(wt, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn hc_worktree_falls_back_to_tab_without_a_local_app() {
    for unavailable in ["binary", "socket", "window", "structured"] {
        let f = Fixture::new();
        fake_worktree(&f);
        match unavailable {
            "binary" => fs::remove_file(f.root().join("bin/herdr-app")).unwrap(),
            "socket" => fs::remove_file(f.root().join("config/herdr-app/herdr-app.sock")).unwrap(),
            _ => fs::write(f.root().join("app-unavailable"), unavailable).unwrap(),
        }
        let o = f
            .cmd()
            .args([
                "spawn",
                "worker",
                "--kind",
                "codex",
                "--worktree",
                "fix-branch",
                "--prompt",
                "do the task",
                "--",
                "-m",
                "chosen",
            ])
            .output()
            .unwrap();
        assert!(
            o.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        let text = success(o);
        assert_eq!(text.matches("no local herdr-app").count(), 1, "{text}");
        let calls = f.calls();
        let run = &calls.iter().find(|v| v[0] == "run").unwrap()[1];
        assert_eq!(run["cwd"], f.root().join("worktree").to_str().unwrap());
        assert_eq!(run["caller"], "t7");
        assert_eq!(run["no_parent"], false);
        assert_eq!(run["labels"]["herdr.space"], "s1");
        assert_eq!(run["labels"]["vega.harness"], "codex");
        assert!(run["labels"].get("vega.worktree-placeholder").is_none());
        let brief = run["argv"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            brief.contains("do the task") && brief.contains("vega post done"),
            "{brief}"
        );
        let shown = f.run(&["show", "worker"]);
        assert!(shown.contains("t7"), "{shown}");
        let app = fs::read_to_string(f.root().join("app-calls")).unwrap_or_default();
        assert_eq!(
            app,
            if unavailable == "window" || unavailable == "structured" {
                "ls\n"
            } else {
                ""
            }
        );
    }
}

#[test]
fn remote_hc_worktree_uses_target_preparation_and_no_local_app_space() {
    let f = Fixture::new();
    // Execute the actual prepare command under a separate target home. Local wt
    // is deliberately absent: all worktree filesystem operations must run there.
    let remote = f.root().join("remote-home");
    fs::create_dir_all(remote.join("bin")).unwrap();
    fs::create_dir(remote.join("worktree")).unwrap();
    let wt = remote.join("bin/wt");
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
    let text = success(f.cmd()
        .env("TEST_MUX_EXE", env!("CARGO_BIN_EXE_vega-mux"))
        .env("TEST_MACHINES", r#"{"machines":[{"name":"remote","host":"remote","id":"id","version":"0.1.18","state":"connected","target":"user@remote","via":false}]}"#)
        .args(["spawn", "worker", "--kind", "codex", "--machine", "remote", "--cwd", "/remote/repo", "--worktree", "fix-branch", "--base", "main", "--prompt", "remote task", "--", "-m", "chosen"]).output().unwrap());
    assert!(text.contains("no local herdr-app"), "{text}");
    let calls = f.calls();
    let run = &calls.iter().find(|v| v[0] == "run").unwrap()[1];
    assert_eq!(run["cwd"], remote.join("worktree").to_str().unwrap());
    assert_eq!(run["on"], "remote");
    assert_eq!(run["caller"], "t7");
    assert_eq!(run["no_parent"], false);
    assert!(run["labels"].get("herdr.space").is_none());
    assert!(run["argv"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap()
        .contains("remote task"));
    assert!(!f.root().join("app-calls").exists());
    let wt_calls = fs::read_to_string(remote.join("wt-calls")).unwrap();
    assert!(
        wt_calls.contains("fix-branch") && wt_calls.contains("--base") && wt_calls.contains("main"),
        "{wt_calls}"
    );
}

#[test]
fn hc_worktree_warns_when_the_shown_space_cannot_be_reselected() {
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
    fs::write(
        f.root().join("app-ls.json"),
        r#"{"spaces":[{"id":"s1","active":true}]}"#,
    )
    .unwrap();
    fs::write(f.root().join("app-select-fails"), "").unwrap();
    let o = f
        .cmd()
        .args([
            "spawn",
            "worker",
            "--kind",
            "codex",
            "--worktree",
            "fix-branch",
            "--",
            "-m",
            "chosen",
        ])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    success(o);
    assert!(err.contains("cannot show space s1 again"), "{err}");
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
    // The person looks at s1 while the sentinel's space is made.
    fs::write(
        f.root().join("app-ls.json"),
        r#"{"spaces":[{"id":"s1","active":true},{"id":"s0","active":false}]}"#,
    )
    .unwrap();
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
    let run = &calls.iter().find(|v| v[0] == "run").unwrap()[1];
    assert_eq!(run["cwd"], tree.to_str().unwrap());
    assert_eq!(run["labels"]["herdr.space"], "s2");
    assert!(run["labels"].get("vega.worktree-placeholder").is_none());
    assert_eq!(run["labels"]["vega.harness"], "codex");
    let worker = run["name"].as_str().unwrap();
    // The space.new shell goes once the agent is in the space: one terminal left.
    let verbs: Vec<_> = calls.iter().map(|v| v[0].as_str().unwrap()).collect();
    let at = |verb: &str, t: &str| calls.iter().position(|v| v[0] == verb && v[1] == t);
    let (killed, removed) = (at("kill", "t0").unwrap(), at("rm", "t0").unwrap());
    let ran = verbs.iter().position(|v| *v == "run").unwrap();
    assert!(ran < killed && killed < removed, "{verbs:?}");
    let left: Value =
        serde_json::from_str(&fs::read_to_string(f.root().join("terminals.json")).unwrap())
            .unwrap();
    assert_eq!(left.as_object().unwrap().len(), 1, "{left}");
    f.run(&["close", worker]);
    let terminals: Value =
        serde_json::from_str(&fs::read_to_string(f.root().join("terminals.json")).unwrap())
            .unwrap();
    assert!(
        terminals.as_object().unwrap().is_empty(),
        "owned space shell must close with its worker: {terminals}"
    );
    let app = fs::read_to_string(f.root().join("app-calls")).unwrap();
    assert!(
        app.contains(&format!(
            "space.new --name fix-branch --folder {}",
            tree.display()
        )),
        "{app}"
    );
    assert!(
        app.contains("do space.select --id s1"),
        "the space shown before the spawn is shown again: {app}"
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
            .env("TEST_MACHINES", r#"{"machines":[{"name":"remote","host":"remote","id":"id","version":"0.1.18","state":"connected","target":"user@remote","via":false}]}"#)
            .args(["spawn", "worker", "--kind", "codex", "--machine", "remote", "--cwd", "/remote/repo", "--", "-m", "chosen"])
            .output().unwrap(),
    );
    assert!(out.contains("remote/t8"), "{out}");
    let calls = f.calls();
    let run = &calls.iter().find(|v| v[0] == "run").unwrap()[1];
    assert_eq!(run["on"], "remote");
    assert_eq!(run["cwd"], "/remote/repo");
    assert_eq!(run["argv"][0], "codex");
    assert!(run["labels"].get("herdr.space").is_none());
    assert_eq!(run["labels"]["vega.kind"], "codex");
    assert_eq!(run["labels"]["vega.mode"], "headed");
    assert_eq!(run["labels"]["vega.role"], "sentinel");
    assert!(!f.root().join("app-calls").exists());
    assert!(!f.root().join("renice-calls").exists());
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

#[test]
fn remote_prepare_uses_target_trust_and_copies_hook_approvals() {
    let f = Fixture::new();
    let remote = f.root().join("remote-home");
    let main = remote.join("repo/main");
    let tree = remote.join("repo/worker");
    fs::create_dir_all(&main).unwrap();
    let git = |cwd: &Path, args: &[&str]| {
        success(
            Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap(),
        );
    };
    git(&main, &["init", "-b", "main"]);
    git(
        &main,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    git(
        &remote,
        &[
            "clone",
            "--shared",
            main.to_str().unwrap(),
            tree.to_str().unwrap(),
        ],
    );
    git(&tree, &["checkout", "--detach"]);
    let main = main.canonicalize().unwrap();
    let tree = tree.canonicalize().unwrap();
    fs::create_dir_all(remote.join(".codex")).unwrap();
    let config = format!("[projects.\"{}\"]\ntrust_level = \"trusted\"\n\n[hooks.state.\"{}/.codex/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:approved\"\n", main.display(), main.display());
    fs::write(remote.join(".codex/config.toml"), &config).unwrap();
    let out = success(
        f.cmd()
            .env("TEST_MUX_EXE", env!("CARGO_BIN_EXE_vega-mux"))
            .env(
                "TEST_MACHINES",
                r#"{"machines":[{"name":"remote","host":"remote","id":"id","version":"0.1.18","state":"connected","target":"user@remote"}]}"#,
            )
            .args([
                "spawn",
                "worker",
                "--kind",
                "codex",
                "--machine",
                "remote",
                "--cwd",
                tree.to_str().unwrap(),
                "--",
                "-m",
                "chosen",
            ])
            .output()
            .unwrap(),
    );
    assert!(out.contains("hook approvals copied"), "{out}");
    let written = fs::read_to_string(remote.join(".codex/config.toml")).unwrap();
    assert!(
        written.contains(&format!("[projects.\"{}\"]", tree.display())),
        "{written}"
    );
    assert!(written.contains(&format!("{}/.codex/hooks.json:stop:0:0", tree.display())));
    assert!(!f.root().join(".codex/config.toml").exists());
    let calls = f.calls();
    let prepare = calls.iter().position(|v| v[0] == "exec").unwrap();
    let launch = calls.iter().position(|v| v[0] == "run").unwrap();
    assert!(prepare < launch);
}

#[test]
fn remote_preparation_failure_never_launches() {
    let f = Fixture::new();
    let out = f
        .cmd()
        .env("TEST_PREPARE_FAIL", "1")
        .env(
            "TEST_MACHINES",
            r#"{"machines":[{"name":"remote","host":"remote","id":"id","version":"0.1.18","state":"connected","target":"user@remote"}]}"#,
        )
        .args([
            "spawn",
            "worker",
            "--kind",
            "codex",
            "--machine",
            "remote",
            "--cwd",
            "/remote/repo",
            "--",
            "-m",
            "chosen",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!f.calls().iter().any(|v| v[0] == "run"));
}

#[test]
fn empty_tui_identity_still_binds_hc_session_before_dispatch() {
    let f = Fixture::new();
    f.spawn();
    success(
        f.cmd()
            .env("HERDR_PANE_ID", "")
            .env("HC_TERMINAL_ID", "t8")
            .env("CODEX_THREAD_ID", "eager-session")
            .args([
                "prepare",
                "--kind",
                "pi",
                "--cwd",
                f.root().to_str().unwrap(),
            ])
            .output()
            .unwrap(),
    );
    let records: Vec<Value> = fs::read_dir(f.root().join("state/vega/spawns"))
        .unwrap()
        .map(|p| serde_json::from_slice(&fs::read(p.unwrap().path()).unwrap()).unwrap())
        .collect();
    let child = records.iter().find(|v| v["pane"] == "t8").unwrap();
    assert_eq!(child["session"], "eager-session");
}

fn mode_remote(f: &Fixture) -> std::path::PathBuf {
    let remote = f.root().join("remote-home");
    fs::create_dir_all(remote.join("bin")).unwrap();
    for name in ["hc", "herdr"] {
        fs::copy(
            f.root().join("bin").join(name),
            remote.join("bin").join(name),
        )
        .unwrap();
    }
    fs::write(
        remote.join("terminals.json"),
        serde_json::json!({"t8":{
            "address":"t8", "name":"worker", "state":"live", "parent":"t7",
            "argv":[], "cwd":"/remote/repo", "created_ms":0,
            "screen":"exact", "cols":80, "rows":24, "labels":{},
            "pid": std::process::id(), "status":{"app":"codex","state":"idle"}
        }})
        .to_string(),
    )
    .unwrap();
    remote
}

#[test]
fn remote_mode_uses_target_local_store_for_reads_and_writes() {
    let f = Fixture::new();
    let remote = mode_remote(&f);
    for (args, expected) in [
        (
            vec!["mode", "--machine", "remote", "t8", "headed"],
            "headed",
        ),
        (vec!["mode", "hc:remote/t8"], "headed"),
        (
            vec!["mode", "--machine", "remote", "worker", "headless"],
            "headless",
        ),
        (vec!["--machine", "remote", "mode", "t8"], "headless"),
        (vec!["mode", "remote/t8", "headed"], "headed"),
    ] {
        let out = success(
            f.cmd()
                .env("TEST_MUX_EXE", env!("CARGO_BIN_EXE_vega-mux"))
                .args(args)
                .output()
                .unwrap(),
        );
        assert!(out.contains(&format!("mode: {expected}")), "{out}");
        let local = success(
            f.cmd()
                .env("HOME", &remote)
                .env_remove("XDG_STATE_HOME")
                .env_remove("XDG_CONFIG_HOME")
                .args(["mode", "t8"])
                .output()
                .unwrap(),
        );
        assert!(local.contains(&format!("mode: {expected}")), "{local}");
        let mode =
            fs::read_to_string(remote.join(".local/state/vega/modes/local_hc-7438")).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&mode).unwrap()["mode"],
            expected
        );
    }
    assert!(!f.root().join("state/vega/modes").exists());
    let calls = f.calls();
    let exec = calls.iter().find(|v| v[0] == "exec").unwrap();
    assert_eq!(exec[2], "remote");
    assert!(exec.as_array().unwrap().iter().any(|v| v == "hc:t8"));
}

#[test]
fn remote_mode_never_reports_success_for_unconfirmed_execution() {
    for failure in [
        "exit",
        "old",
        "json",
        "base64",
        "mismatch",
        "timeout",
        "truncated",
    ] {
        let f = Fixture::new();
        let out = f
            .cmd()
            .env("TEST_MODE_FAILURE", failure)
            .args(["mode", "--machine", "remote", "t8", "headed"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "accepted {failure}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("remote_mode_failed"), "{failure}: {text}");
        assert!(!text.contains("mode: headed"), "{failure}: {text}");
        assert!(!f.root().join("state/vega/modes").exists());
    }
}

#[test]
fn remote_mode_reports_target_write_failures_and_refuses_conflicting_hosts() {
    let f = Fixture::new();
    let remote = mode_remote(&f);
    fs::create_dir_all(remote.join(".local/state/vega")).unwrap();
    fs::write(remote.join(".local/state/vega/modes"), "not a directory").unwrap();
    let out = f
        .cmd()
        .env("TEST_MUX_EXE", env!("CARGO_BIN_EXE_vega-mux"))
        .args(["mode", "--machine", "remote", "t8", "headed"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("remote_mode_failed"));
    let out = f
        .cmd()
        .args(["mode", "--machine", "remote", "other/t8", "headed"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("different hosts"));
    assert_eq!(f.calls().iter().filter(|v| v[0] == "exec").count(), 1);
    assert!(!f.root().join("state/vega/modes").exists());
}

#[test]
fn remote_mode_supports_tui_panes_without_remote_file_caches() {
    let f = Fixture::new();
    let remote = mode_remote(&f);
    fs::write(
        remote.join("bin/herdr"),
        r#"#!/bin/sh
[ "$*" = 'agent get w1:p2' ] || exit 2
printf '%s\n' '{"result":{"agent":{"pane_id":"w1:p2","name":"worker","agent":"codex"}}}'
"#,
    )
    .unwrap();
    for args in [
        vec!["mode", "--machine", "remote", "w1:p2", "headed"],
        vec!["mode", "remote:w1:p2"],
    ] {
        let out = success(
            f.cmd()
                .env("TEST_MUX_EXE", env!("CARGO_BIN_EXE_vega-mux"))
                .args(args)
                .output()
                .unwrap(),
        );
        assert!(out.contains("mode: headed"), "{out}");
    }
    assert!(remote.join(".local/state/vega/modes/local_w1_p2").exists());
    assert!(!f.root().join("state/vega/modes").exists());
}

#[test]
fn remote_mode_refuses_unavailable_transport_and_disallowed_direction() {
    for (variable, value, expected) in [
        ("TEST_PREPARE_FAIL", "1", "remote_mode_failed"),
        ("VEGA_POST_HOST", "puck", "machine_direction"),
    ] {
        let f = Fixture::new();
        let out = f
            .cmd()
            .env(variable, value)
            .args(["mode", "--machine", "remote", "t8", "headed"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success());
        assert!(text.contains(expected), "{text}");
        assert!(!f.root().join("state/vega/modes").exists());
    }
}

const REMOTE_MACHINES: &str = r#"{"machines":[{"name":"remote","host":"remote","id":"id","version":"0.1.18","state":"connected","target":"user@remote","via":false}]}"#;

fn remote_spawn_command(f: &Fixture) -> Command {
    let mut c = f.cmd();
    c.env("TEST_MUX_EXE", env!("CARGO_BIN_EXE_vega-mux"))
        .env("TEST_MACHINES", REMOTE_MACHINES)
        .args([
            "spawn",
            "worker",
            "--kind",
            "codex",
            "--machine",
            "remote",
            "--cwd",
            "/remote/repo",
            "--prompt",
            "implement remote task",
        ]);
    c
}

#[test]
fn remote_spawn_records_are_readable_on_the_pane_host() {
    for headless in [false, true] {
        let f = Fixture::new();
        let remote = mode_remote(&f);
        let mut command = remote_spawn_command(&f);
        if headless {
            command.arg("--headless");
        }
        success(command.args(["--", "-m", "chosen"]).output().unwrap());
        let state = remote.join(".local/state/vega");
        let mode = if headless { "headless" } else { "headed" };
        for verb in ["mode", "show"] {
            let text = success(
                f.cmd()
                    .env("HOME", &remote)
                    .env_remove("XDG_STATE_HOME")
                    .env_remove("XDG_CONFIG_HOME")
                    .env("VEGA_POST_HOST", "remote")
                    .args([verb, "t8"])
                    .output()
                    .unwrap(),
            );
            assert!(text.contains(&format!("mode: {mode}")), "{text}");
            if verb == "show" {
                assert!(text.contains("parent: t7 on test"), "{text}");
                assert!(text.contains("title: implement remote task"), "{text}");
                assert!(text.contains("cwd: /remote/repo"), "{text}");
                assert!(text.contains("model: chosen"), "{text}");
            } else {
                assert!(!text.contains("no parent recorded"), "{text}");
            }
        }
        let read = |kind: &str| fs::read_to_string(state.join(kind).join("local_hc-7438")).unwrap();
        let record: Value = serde_json::from_str(&read("spawns")).unwrap();
        assert_eq!(record["pane"], "t8");
        assert_eq!(record["cwd"], "/remote/repo");
        assert_eq!(
            record["parent"],
            serde_json::json!({"pane":"t7","host":"test"})
        );
        assert!(read("briefs").parse::<u64>().unwrap() > 0);
        assert!(!state.join("undelivered/local_hc-7438").exists());
        let calls = f.calls();
        let exec = calls
            .iter()
            .find(|v| {
                v.as_array()
                    .is_some_and(|a| a.iter().any(|v| v == "record-spawn"))
            })
            .unwrap();
        assert_eq!(exec[4], "15.0");
    }
}

#[test]
fn remote_spawn_removes_stale_parent_title_and_brief_for_no_parent() {
    let f = Fixture::new();
    let remote = mode_remote(&f);
    for kind in ["parents", "tasks", "undelivered"] {
        let dir = remote.join(".local/state/vega").join(kind);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("local_hc-7438"), "stale").unwrap();
    }
    success(
        f.cmd()
            .env("TEST_MUX_EXE", env!("CARGO_BIN_EXE_vega-mux"))
            .env("TEST_MACHINES", REMOTE_MACHINES)
            .args([
                "spawn",
                "worker",
                "--kind",
                "codex",
                "--machine",
                "remote",
                "--cwd",
                "/remote/repo",
                "--no-parent",
                "--headless",
            ])
            .output()
            .unwrap(),
    );
    for kind in ["parents", "tasks", "undelivered"] {
        assert!(!remote
            .join(".local/state/vega")
            .join(kind)
            .join("local_hc-7438")
            .exists());
    }
    let text = success(
        f.cmd()
            .env("HOME", &remote)
            .env_remove("XDG_STATE_HOME")
            .args(["mode", "t8"])
            .output()
            .unwrap(),
    );
    assert!(
        text.contains("mode: headless") && text.contains("no parent recorded"),
        "{text}"
    );
}

#[test]
fn remote_spawn_keeps_undelivered_brief_on_target() {
    let f = Fixture::new();
    let remote = mode_remote(&f);
    let out = remote_spawn_command(&f)
        .env("TEST_UNDELIVERED", "1")
        .args(["--", "-m", "chosen"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("brief_not_delivered"));
    let brief: Value = serde_json::from_slice(
        &fs::read(remote.join(".local/state/vega/undelivered/local_hc-7438")).unwrap(),
    )
    .unwrap();
    assert_eq!(brief["brief"], "implement remote task");
    assert_eq!(brief["name"], "worker");
    assert_eq!(brief["no_parent"], false);
}

#[test]
fn remote_spawn_reports_unconfirmed_records_without_claiming_success() {
    for failure in [
        "unavailable",
        "exit",
        "old",
        "json",
        "base64",
        "mismatch",
        "timeout",
        "truncated",
    ] {
        let f = Fixture::new();
        mode_remote(&f);
        let out = remote_spawn_command(&f)
            .env("TEST_RECORD_FAILURE", failure)
            .args(["--", "-m", "chosen"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "accepted {failure}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains("remote_spawn_record_failed") && text.contains("remote/t8 started"),
            "{failure}: {text}"
        );
        assert!(!text.contains("spawned:"), "{failure}: {text}");
    }
}

#[test]
fn remote_spawn_reports_each_target_record_write_failure() {
    for kind in [
        "spawns",
        "modes",
        "parents",
        "tasks",
        "briefs",
        "undelivered",
    ] {
        let f = Fixture::new();
        let remote = mode_remote(&f);
        let path = remote.join(".local/state/vega").join(kind);
        fs::create_dir_all(&path).unwrap();
        // A directory in place of the record blocks both replacement and removal.
        fs::create_dir(path.join("local_hc-7438")).unwrap();
        let out = remote_spawn_command(&f)
            .args(["--", "-m", "chosen"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "accepted {kind}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains("remote_spawn_record_failed")
                && text.contains(&format!("cannot save {kind}")),
            "{kind}: {text}"
        );
        assert!(!text.contains("spawned:"), "{kind}: {text}");
    }
}

#[test]
fn target_spawn_receiver_supports_tui_and_rejects_invalid_requests() {
    use base64::Engine;
    let f = Fixture::new();
    let record = vega_herdr::state::SpawnRecord {
        backend: "herdr".into(),
        pane: "w1:p2".into(),
        name: "worker".into(),
        kind: "codex".into(),
        cwd: "/remote/tree".into(),
        mode: "headed".into(),
        parent: Some(vega_herdr::state::Parent {
            pane: "t7".into(),
            host: "caller".into(),
        }),
        task: Some("task title".into()),
        started_ms: Some(123),
        ..Default::default()
    };
    let request = serde_json::json!({"schema":1,"record":record,"undelivered":"brief"});
    let encode = |v: &Value| base64::engine::general_purpose::STANDARD.encode(v.to_string());
    let text = success(
        f.cmd()
            .args(["record-spawn", &encode(&request)])
            .output()
            .unwrap(),
    );
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), request);
    for kind in [
        "spawns",
        "modes",
        "parents",
        "tasks",
        "briefs",
        "undelivered",
    ] {
        assert!(f
            .root()
            .join("state/vega")
            .join(kind)
            .join("local_w1_p2")
            .exists());
    }
    for change in [
        serde_json::json!({"schema":2}),
        serde_json::json!({"record":{"pane":"remote:w1:p2"}}),
        serde_json::json!({"record":{"mode":"bogus"}}),
        serde_json::json!({"undelivered":false}),
    ] {
        let mut invalid = request.clone();
        if let Some(r) = change.get("record") {
            for (k, v) in r.as_object().unwrap() {
                invalid["record"][k] = v.clone();
            }
        } else {
            for (k, v) in change.as_object().unwrap() {
                invalid[k] = v.clone();
            }
        }
        let out = f
            .cmd()
            .args(["record-spawn", &encode(&invalid)])
            .output()
            .unwrap();
        assert!(!out.status.success(), "accepted {invalid}");
        assert!(serde_json::from_slice::<Value>(&out.stdout)
            .unwrap()
            .get("error")
            .is_some());
    }
    let out = f
        .cmd()
        .args(["--machine", "remote", "record-spawn", &encode(&request)])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn legacy_remote_tui_spawn_also_confirms_target_records() {
    for failure in [None, Some("timeout")] {
        let f = Fixture::new();
        let remote = mode_remote(&f);
        let script = r#"#!/usr/bin/python3
import json, sys
a=sys.argv[1:]
if a[:1]==['--machine']: a=a[2:]
if a[:3]==['agent','get','worker']:
 print(json.dumps({'error':{'code':'agent_not_found','message':'absent','hint':'spawn'}}));sys.exit(1)
if a[:2]==['pane','list']: result={'panes':[]}
elif a[:2]==['tab','create']: result={'tab':{'tab_id':'tab1'},'root_pane':{'pane_id':'w1:p2'}}
elif a[:2]==['agent','start']: result={}
elif a[:2]==['agent','get']: result={'agent':{'pane_id':'w1:p2','agent_name':'worker','name':'worker','agent':'codex','agent_status':'idle'}}
else: raise AssertionError(a)
print(json.dumps({'result':result}))
"#;
        for path in [f.root().join("bin/herdr"), remote.join("bin/herdr")] {
            fs::write(path, script).unwrap();
        }
        let mut c = f.cmd();
        c.env("TEST_MUX_EXE", env!("CARGO_BIN_EXE_vega-mux")).args([
            "spawn",
            "worker",
            "--backend",
            "herdr",
            "--kind",
            "codex",
            "--machine",
            "remote",
            "--cwd",
            "/remote/repo",
            "--at",
            "tab",
            "--no-parent",
        ]);
        if let Some(failure) = failure {
            c.env("TEST_RECORD_FAILURE", failure);
        }
        let out = c.output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        if failure.is_some() {
            assert!(!out.status.success());
            assert!(text.contains("remote_spawn_record_failed"), "{text}");
        } else {
            assert!(
                out.status.success(),
                "{text} {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let mode: Value = serde_json::from_slice(
                &fs::read(remote.join(".local/state/vega/modes/local_w1_p2")).unwrap(),
            )
            .unwrap();
            assert_eq!(mode["mode"], "headed");
            assert!(!remote
                .join(".local/state/vega/parents/local_w1_p2")
                .exists());
        }
    }
}
