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
 get) if [ "$3" = t1 ]; then cat "$HOME/me.json"; else cat "$HOME/child.json"; fi ;;
 ls) cat "$HOME/hc.json" ;;
 *) echo '{}' ;;
esac"#,
        );
        script(
            &bin,
            "bd",
            r#"case "$1" in
 show) echo '[{"id":"decision-1","title":"Decision","status":"open"}]' ;;
 create) echo '{"id":"tracker-1"}' ;;
 *) echo '[]' ;;
esac"#,
        );
        script(&bin, "vega", "printf '%s\\n' \"$*\" >> \"$HOME/mail\"");
        script(&bin, "gh", "echo '{}'");
        let me = if hc { "t1" } else { "w1:p1" };
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
                "hc",
                json!({"terminals":[terminal("t1"),terminal("t2"), {"address":"remote/t9","state":"live","cwd":"/not/local"}]}),
            ),
        ] {
            fs::write(root.join(format!("{name}.json")), value.to_string()).unwrap();
        }
        let call = |args: &[&str]| {
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
                    if hc {
                        "HC_TERMINAL_ID"
                    } else {
                        "HERDR_PANE_ID"
                    },
                    me,
                );
            let out = cmd.output().unwrap();
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
        assert!(call(&["park", child, "--bead", "decision-1"]).contains("parked:"));
        assert!(call(&["unpark", child]).contains("unparked:"));
        fs::rename(bin.join("bd"), bin.join("bd-off")).unwrap();
        assert!(call(&["hold", child]).contains("held:"));
        assert!(call(&["hold", child]).contains("held:"));
        assert!(call(&[]).contains("on hold"));
        assert!(call(&["unpark", child]).contains("unparked:"));
        fs::rename(bin.join("bd-off"), bin.join("bd")).unwrap();
        call(&["ask", "status?"]);
        assert!(call(&["answer", "decision-1", "yes"]).contains("you (no mail)"));
        assert!(fs::read_to_string(root.join("mail"))
            .unwrap()
            .contains("post ask"));
        call(&["release"]);
        if hc {
            script(&bin, "herdr", "exit 1");
            assert!(call(&["status"]).contains("t2"));
        }
    }
}
