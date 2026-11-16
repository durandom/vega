//! Real command dispatch with isolated terminal, mail and bead executables.
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};
fn script(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
}
#[test]
fn coordinator_commands_work_in_both_worlds() {
    for hc in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "remote",
                "add",
                "origin",
                "https://github.com/example/project.git",
            ],
        ] {
            assert!(Command::new("git")
                .current_dir(root)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        script(
            &bin,
            "herdr",
            r#"
case "$1 $2" in
 'pane list') cat "$HOME/panes.json" ;;
 'pane process-info') printf '{"result":{"process_info":{"shell_pid":%s}}}' "$FAKE_PID" ;;
 'machine list') echo '[]' ;;
 'agent list') cat "$HOME/agents.json" ;;
 'agent get') cat "$HOME/agent.json" ;;
 'workspace list') echo '{"result":{"workspaces":[]}}' ;;
 'tab list') echo '{"result":{"tabs":[]}}' ;;
 *) echo '{"result":{}}' ;;
esac"#,
        );
        script(
            &bin,
            "hc",
            r#"
exec /usr/bin/python3 -c '
import base64,json,os,subprocess,sys
from pathlib import Path
r=json.loads(sys.stdin.readline()); p=r["params"]; root=Path(os.environ["HOME"])
if r["method"]=="machine.exec":
 assert p["on"]=="io" and p["timeout"]==15.0
 assert p["argv"][:2]==["vega-uac","record-adoption"]
 remote=root/"remote-home"
 e=dict(os.environ, HOME=str(remote), XDG_STATE_HOME=str(remote/"state"), XDG_CONFIG_HOME=str(remote/"config"), VEGA_POST_HOST="io")
 for k in ["HC_TERMINAL_ID","HERDR_PANE_ID"]: e.pop(k,None)
 out=subprocess.run([os.environ["TEST_UAC_EXE"]]+p["argv"][1:],env=e,capture_output=True)
 failure=(root/"failure").read_text() if (root/"failure").exists() else ""
 stdout=out.stdout
 if failure=="old": stdout=b"adopted: t9"
 if failure=="json": stdout=b"{}"
 if failure=="mismatch":
  obj=json.loads(stdout); obj["parent"]["host"]="wrong"; stdout=json.dumps(obj).encode()
 v={"ms":1,"signal":None,"stdout":base64.b64encode(stdout).decode(),"stderr":base64.b64encode(out.stderr).decode(),"code":out.returncode if failure!="exit" else 1,"timed_out":failure=="timeout","truncated":failure=="truncated"}
elif r["method"]=="terminal.list": v=json.loads((root/"hc.json").read_text())
else:
 assert r["method"]=="terminal.get"
 name={"t1":"me","t2":"child","t9":"remote","io/t9":"remote","io/remote-worker":"remote"}.get(p["terminal"])
 if name is None:
  print(json.dumps({"jsonrpc":"2.0","id":r["id"],"error":{"code":-32000,"message":"unexpected target","data":{"kind":"not_found"}}})); sys.exit(3)
 v=json.loads((root/(name+".json")).read_text())
print(json.dumps({"jsonrpc":"2.0","id":r["id"],"result":v}))
'
"#,
        );
        script(
            &bin,
            "bd",
            r#"printf '%s\n' "$*" >> "$HOME/bd-calls"
case "$1" in
 show) echo '[{"id":"decision-1","title":"Decision","status":"open"}]' ;;
 create) echo '{"id":"tracker-1"}' ;;
 *) echo '[]' ;;
esac"#,
        );
        script(&bin, "vega", "printf '%s\\n' \"$*\" >> \"$HOME/mail\"");
        script(&bin, "gh", "echo '{}'");
        let child = if hc { "t2" } else { "w1:p2" };
        let tui = |p: &str| json!({"pane_id":p,"name":p,"agent":"codex","agent_status":"idle","cwd":root});
        let terminal = |p: &str| json!({"argv":[],"labels":{},"created_ms":0,"screen":"exact","cols":80,"rows":24,"address":p,"name":p,"state":"live","pid":std::process::id(),"status":{"app":"codex","state":"idle"},"cwd":root});
        for (name, value) in [
            ("panes", json!({"result":{"panes":[tui("w1:p1")]}})),
            (
                "agents",
                json!({"result":{"agents":[tui("w1:p1"),tui("w1:p2")]}}),
            ),
            ("agent", json!({"result":{"agent":tui("w1:p2")}})),
            ("me", terminal("t1")),
            ("child", terminal("t2")),
            (
                "remote",
                json!({"argv":[],"labels":{},"created_ms":0,"screen":"exact","cols":80,"rows":24,"address":"io/t9", "name":"remote-worker", "state":"live", "status":{"app":"codex", "state":"idle"}, "cwd":"/remote/path/never/local"}),
            ),
            (
                "hc",
                json!({"hidden_finished":0,"machines":[],"terminals":[terminal("t1"),terminal("t2"), {"argv":[],"labels":{},"created_ms":0,"screen":"exact","cols":80,"rows":24,"address":"remote/t9","state":"live","cwd":"/not/local"}]}),
            ),
        ] {
            fs::write(root.join(format!("{name}.json")), value.to_string()).unwrap();
        }
        let invoke = |args: &[&str], hc_caller: bool| {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_vega-uac"));
            cmd.args(args)
                .current_dir(root)
                .env_clear()
                .env("HOME", root)
                .env("XDG_STATE_HOME", root.join("state"))
                .env("XDG_CONFIG_HOME", root.join("config"))
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("FAKE_PID", std::process::id().to_string())
                .env("VEGA_POST_HOST", "fixture")
                .env("VEGA_POST_INBOX_ONLY", "fixture,io")
                .env("TEST_UAC_EXE", env!("CARGO_BIN_EXE_vega-uac"))
                .env("VEGA_NATS_URL", "nats://127.0.0.1:9")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(
                    if hc_caller {
                        "HC_TERMINAL_ID"
                    } else {
                        "HERDR_PANE_ID"
                    },
                    if hc_caller { "t1" } else { "w1:p1" },
                );
            cmd.output().unwrap()
        };
        let call = |args: &[&str]| {
            let out = invoke(args, hc);
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            assert!(
                out.status.success(),
                "{hc} {args:?}: {text}\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            text
        };
        assert!(call(&["claim"]).contains("(you)"));
        let json_text = call(&["--json"]);
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        assert_eq!(json["schema"], 1);
        assert_eq!(json["project"], "github.com/example/project");
        assert!(json["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["pane"] == "t2"));
        assert!(json["grants"].is_array());
        let bad = invoke(&["--json", "--bad"], hc);
        assert_eq!(bad.status.code(), Some(2));
        let error: serde_json::Value = serde_json::from_slice(&bad.stdout).unwrap();
        assert_eq!(error["error"]["code"], "usage");
        let listing = call(&[]);
        assert!(listing.contains("t2") && listing.contains("w1:p2"));
        assert!(!listing.contains("remote/t9"));
        let status = call(&["status"]);
        assert!(status.contains("project:"));
        assert!(
            status.contains("registry: off (inbox-only host: local agents only)"),
            "{status}"
        );
        assert!(!status.contains("parents and attention from this machine only"));
        assert!(call(&["adopt", child, "--headless"]).contains("told: yes"));
        let other_child = if hc { "w1:p2" } else { "t2" };
        let other_target = if hc { "herdr:w1:p2" } else { "hc:t2" };
        assert!(call(&["adopt", other_target, "--headless"]).contains("told: yes"));
        assert!(call(&["park", child, "--bead", "decision-1"]).contains("parked:"));
        assert!(call(&["unpark", child]).contains("unparked:"));
        fs::rename(bin.join("bd"), bin.join("bd-off")).unwrap();
        assert!(call(&["hold", child]).contains("held:"));
        // Repeating a hold is an idempotent success.
        assert!(call(&["hold", child]).contains("held:"));
        assert!(call(&[]).contains("on hold"));
        assert!(call(&["unpark", child]).contains("unparked:"));
        fs::rename(bin.join("bd-off"), bin.join("bd")).unwrap();
        // All remote operations below go through the fake hc, never a real core.
        let remote = root.join("remote-home");
        fs::create_dir(&remote).unwrap();
        let mut target: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("remote.json")).unwrap()).unwrap();
        target["address"] = "t9".into();
        fs::write(remote.join("remote.json"), target.to_string()).unwrap();
        // Every unconfirmed reply fails before success, cache writes, or mail.
        let mail_before = fs::read(root.join("mail")).unwrap();
        for failure in ["old", "json", "mismatch", "exit", "timeout", "truncated"] {
            fs::write(root.join("failure"), failure).unwrap();
            let out = invoke(&["adopt", "hc:io/t9"], hc);
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(!out.status.success(), "{failure}: {text}");
            assert!(
                text.contains("remote_adoption_failed") && !text.contains("adopted:"),
                "{text}"
            );
            assert!(!root.join("state/vega/parents/io_hc-7439").exists());
            assert_eq!(fs::read(root.join("mail")).unwrap(), mail_before);
        }
        fs::remove_file(root.join("failure")).unwrap();
        target["name"] = "replacement-agent".into();
        fs::write(remote.join("remote.json"), target.to_string()).unwrap();
        let changed = invoke(&["adopt", "hc:io/t9"], hc);
        let text = String::from_utf8_lossy(&changed.stdout);
        assert!(
            !changed.status.success()
                && text.contains("adoption target changed")
                && !text.contains("adopted:"),
            "{text}"
        );
        target["name"] = "remote-worker".into();
        fs::write(remote.join("remote.json"), target.to_string()).unwrap();
        // Target write failures cannot be acknowledged either.
        for kind in ["parents", "modes", "briefs"] {
            let dir = remote.join(format!("state/vega/{kind}"));
            if dir.exists() {
                fs::remove_dir_all(&dir).unwrap();
            }
            fs::create_dir_all(dir.parent().unwrap()).unwrap();
            fs::write(&dir, "blocked").unwrap();
            let out = invoke(&["adopt", "hc:io/t9"], hc);
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                !out.status.success()
                    && text.contains("remote_adoption_failed")
                    && !text.contains("adopted:"),
                "{text}"
            );
            assert!(!root.join("state/vega/parents/io_hc-7439").exists());
            assert_eq!(fs::read(root.join("mail")).unwrap(), mail_before);
            fs::remove_file(&dir).unwrap();
        }
        assert!(call(&["adopt", "hc:io/t9"]).contains("headed"));
        let read_remote = |kind: &str| -> serde_json::Value {
            serde_json::from_slice(
                &fs::read(remote.join(format!("state/vega/{kind}/local_hc-7439"))).unwrap(),
            )
            .unwrap()
        };
        let check_readers = |mode: &str| {
            for (home, host) in [(root, "fixture"), (remote.as_path(), "io")] {
                let out = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "adopted_state_is_readable_on_each_host"])
                    .env_clear()
                    .env("HOME", home)
                    .env("XDG_STATE_HOME", home.join("state"))
                    .env("XDG_CONFIG_HOME", home.join("config"))
                    .env("PATH", "/usr/bin:/bin")
                    .env("VEGA_POST_HOST", host)
                    .env("TEST_ADOPTED_MODE", mode)
                    .env("TEST_ADOPTED_PARENT", if hc { "t1" } else { "w1:p1" })
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        };
        check_readers("headed");
        assert_eq!(read_remote("modes")["mode"], "headed");
        assert_eq!(read_remote("parents")["host"], "fixture");
        assert_eq!(
            read_remote("parents")["parent"],
            if hc { "t1" } else { "w1:p1" }
        );
        assert!(!remote.join("state/vega/parents/io_hc-7439").exists());
        assert!(!root.join("state/vega/parents/local_hc-7439").exists());
        assert!(call(&["adopt", "hc:io/t9", "--headless"]).contains("adopted: t9"));
        assert_eq!(read_remote("modes")["mode"], "headless");
        check_readers("headless");
        assert_eq!(read_remote("parents")["name"], "remote-worker");
        assert!(
            fs::read_to_string(remote.join("state/vega/briefs/local_hc-7439"))
                .unwrap()
                .parse::<u128>()
                .unwrap()
                > 0
        );
        assert!(call(&["adopt", "hc:remote-worker", "--machine", "io"]).contains("headed"));
        assert!(call(&["park", "hc:io/t9", "--bead", "decision-1"]).contains("parked: t9"));
        let remote_park = root.join("state/vega/parked/io_hc-7439");
        let parked: serde_json::Value =
            serde_json::from_slice(&fs::read(&remote_park).unwrap()).unwrap();
        assert_eq!(parked["bead"], "decision-1");
        assert_eq!(parked["held"], false);
        assert!(!root.join("state/vega/parked/local_hc-7439").exists());
        assert!(!root.join("state/vega/parked/io_hc-696f2f7439").exists());
        assert!(call(&["hold", "hc:t9", "--machine", "io"]).contains("held: t9"));
        let held: serde_json::Value =
            serde_json::from_slice(&fs::read(&remote_park).unwrap()).unwrap();
        assert_eq!(held["held"], true);
        assert_eq!(held["bead"], "");
        assert!(call(&["unpark", "hc:io/t9"]).contains("unparked: t9"));
        assert!(!remote_park.exists());
        assert!(call(&["hold", "hc:remote-worker", "--machine", "io"]).contains("held: t9"));
        assert!(remote_park.exists());
        call(&["unpark", "t9", "--machine", "io"]);
        assert!(!remote_park.exists());
        call(&["ask", "status?"]);
        assert!(call(&["answer", "decision-1", "yes"]).contains("you (no mail)"));
        assert!(fs::read_to_string(root.join("mail"))
            .unwrap()
            .contains("post ask"));
        let conflicting = invoke(&["hold", "hc:io/t7", "--machine", "deimos"], hc);
        assert!(!conflicting.status.success());
        let error = String::from_utf8_lossy(&conflicting.stdout);
        assert!(
            error.contains("usage") && error.contains("different machines"),
            "{error}"
        );
        for target in ["hc:io/worker", "io/worker"] {
            let rejected = invoke(&["hold", target], hc);
            assert!(!rejected.status.success());
            let error = String::from_utf8_lossy(&rejected.stdout);
            assert!(error.contains("terminal ID, not a name"), "{error}");
        }

        // A healthy caller in the other world cannot take over an unverifiable UAC.
        let unavailable = if hc { "hc" } else { "herdr" };
        let saved = bin.join("original-backend");
        fs::rename(bin.join(unavailable), &saved).unwrap();
        script(&bin, unavailable, "exit 1");
        let refused = invoke(&["claim"], !hc);
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stdout).contains("inventory_unavailable"),
            "{}",
            String::from_utf8_lossy(&refused.stdout)
        );
        fs::rename(saved, bin.join(unavailable)).unwrap();
        assert!(
            call(&["claim"]).contains("(you, already registered)"),
            "refusal must preserve the owner"
        );
        call(&["release"]);
        script(&bin, if hc { "herdr" } else { "hc" }, "exit 1");
        let partial = call(&["status"]);
        assert!(partial.contains(child));
        assert!(
            !partial.contains(other_child),
            "failed inventory is not proof of exit: {partial}"
        );
        script(&bin, if hc { "hc" } else { "herdr" }, "exit 1");
        let saved_answer = call(&["answer", "decision-1", "still saved"]);
        assert!(saved_answer.contains("answered: decision-1"));
        assert!(saved_answer.contains("uac: unknown ("));
        assert!(!saved_answer.contains("none alive"));
        assert!(fs::read_to_string(root.join("bd-calls"))
            .unwrap()
            .contains("comments add decision-1 --author Slayer -- still saved"));
    }
}

// Run in a subprocess so host state cannot leak between parallel tests.
#[test]
fn adopted_state_is_readable_on_each_host() {
    let Ok(expected_mode) = std::env::var("TEST_ADOPTED_MODE") else {
        return;
    };
    use vega_herdr::state as st;
    let remote = std::env::var("VEGA_POST_HOST").unwrap() == "io";
    let scope = (!remote).then_some("io");
    assert_eq!(
        st::mode_of(scope, "t9", "remote-worker").as_str(),
        expected_mode
    );
    assert_eq!(st::mode_of(scope, "t9", "reused-name"), st::Mode::Headless);
    let parent = st::parent_of_on(scope, "t9", "remote-worker").unwrap();
    assert_eq!(parent.host, "fixture");
    assert_eq!(parent.pane, std::env::var("TEST_ADOPTED_PARENT").unwrap());
    assert!(st::parent_of_on(scope, "t9", "reused-name").is_none());
    let host = if remote { "io" } else { "fixture" };
    let entries = vega_registry::local_entries(host, &[], &[]);
    let link = entries
        .get(&vega_registry::link_key(host, "io", "t9"))
        .unwrap();
    assert_eq!(link["parent_host"], "fixture");
    assert_eq!(link["parent"], parent.pane);
}
