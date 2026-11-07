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
case "$2" in
 get) case "$3" in
   t1) cat "$HOME/me.json" ;;
   t2) cat "$HOME/child.json" ;;
   io/t9|io/remote-worker) cat "$HOME/remote.json" ;;
   *) echo '{"error":{"kind":"not_found","message":"unexpected target"}}'; exit 3 ;;
 esac ;;
 ls) cat "$HOME/hc.json" ;;
 *) echo '{}' ;;
esac"#,
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
        let terminal = |p: &str| json!({"address":p,"name":p,"state":"live","pid":std::process::id(),"status":{"app":"codex","state":"idle"},"cwd":root});
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
                json!({"address":"io/t9", "name":"remote-worker", "state":"live", "status":{"app":"codex", "state":"idle"}, "cwd":"/remote/path/never/local"}),
            ),
            (
                "hc",
                json!({"terminals":[terminal("t1"),terminal("t2"), {"address":"remote/t9","state":"live","cwd":"/not/local"}]}),
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
                .env("VEGA_POST_INBOX_ONLY", "fixture")
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
        let listing = call(&[]);
        assert!(listing.contains("t2") && listing.contains("w1:p2"));
        assert!(!listing.contains("remote/t9"));
        assert!(call(&["status"]).contains("project:"));
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
        assert!(call(&["adopt", "hc:io/t9", "--headless"]).contains("adopted: t9"));
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
