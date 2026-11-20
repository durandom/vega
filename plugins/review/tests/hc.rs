#[path = "../../../crates/vega-herdr/tests/fixtures/own-terminal.rs"]
mod common;
#[test]
fn hc_review_reads_own_session_and_rejects_headless_child_start() {
    let (_dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-review"));
    let o = c.output().unwrap();
    assert!(o.status.success(), "{o:?}");
    let o = c
        .args(["start", "http://127.0.0.1:5287/"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(!o.status.success());
    assert!(
        text.contains("headless") || text.contains("headed"),
        "{text}"
    );
    assert!(!text.contains("not_in_terminal"), "{text}");
}

#[test]
fn review_uses_the_pane_hosts_local_mode_record() {
    let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-review"));
    // Stop just beyond the ownership check, before any inbox or browser starts.
    let state = dir.path().join("state/vega");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("review"), "not a directory").unwrap();
    c.args(["start", "http://127.0.0.1:5287/"]);
    let modes = state.join("modes");
    std::fs::create_dir(&modes).unwrap();
    // A caller-side remote-scoped record cannot authorize the pane host.
    std::fs::write(
        modes.join("remote_hc-7437"),
        r#"{"name":"child","mode":"headed"}"#,
    )
    .unwrap();
    let out = c.output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("headless_pane"));
    for (name, mode, expected) in [
        ("child", "headed", "write_failed"),
        ("child", "headless", "headless_pane"),
        ("previous", "headed", "headless_pane"),
    ] {
        std::fs::write(
            modes.join("local_hc-7437"),
            format!(r#"{{"name":"{name}","mode":"{mode}"}}"#),
        )
        .unwrap();
        let out = c.output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success());
        assert!(text.contains(expected), "{name}/{mode}: {text}");
    }
}

#[test]
fn shared_profiles_attach_and_close_only_the_review_target() {
    use serde_json::{json, Value};
    use std::{fs, net::TcpListener, os::unix::fs::PermissionsExt, thread, time::Duration};
    use tungstenite::Message;

    for profile in ["global", "vpn"] {
        let (dir, mut start) = common::fixture(env!("CARGO_BIN_EXE_vega-review"));
        let root = dir.path();
        let modes = root.join("state/vega/modes");
        fs::create_dir_all(&modes).unwrap();
        fs::write(
            modes.join("local_hc-7437"),
            r#"{"name":"child","mode":"headed"}"#,
        )
        .unwrap();
        for (name, body) in [
            (
                "vega-browser",
                r#"
printf '%s\n' "$*" >> "$HOME/browser-calls"
case "$1" in
  ab) for last do :; done; printf '%s' "$last" > "$HOME/blank" ;;
  close|vpn) ;;
  *) exit 1 ;;
esac
"#,
            ),
            (
                "agent-browser",
                r#"
printf '%s\n' "$*" >> "$HOME/agent-calls"
case "$3 $4" in
  "get cdp-url") echo "$TEST_CDP" ;;
  "open "*) : > "$HOME/navigated" ;;
esac
"#,
            ),
        ] {
            let path = root.join("bin").join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "ws://{}/devtools/browser/test",
            listener.local_addr().unwrap()
        );
        let root_for_cdp = root.to_owned();
        let cdp = thread::spawn(move || {
            for expected in [
                &[
                    "Target.getTargets",
                    "Target.attachToTarget",
                    "Page.setBypassCSP",
                ][..],
                &["Target.getTargets", "Target.closeTarget"][..],
            ] {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                for method in expected {
                    let request: Value =
                        serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                    assert_eq!(request["method"], *method);
                    let result = match *method {
                        "Target.getTargets" => {
                            let url = if root_for_cdp.join("navigated").exists() {
                                "https://example.test/design".to_owned()
                            } else {
                                fs::read_to_string(root_for_cdp.join("blank")).unwrap()
                            };
                            json!({"targetInfos": [
                                {"type":"page","targetId":"other","url":"https://example.test/design"},
                                {"type":"page","targetId":"review","url":url},
                            ]})
                        }
                        "Target.attachToTarget" => {
                            assert_eq!(request["params"]["targetId"], "review");
                            json!({"sessionId":"csp-session"})
                        }
                        "Page.setBypassCSP" => {
                            assert_eq!(request["sessionId"], "csp-session");
                            assert_eq!(request["params"]["enabled"], true);
                            json!({})
                        }
                        "Target.closeTarget" => {
                            assert_eq!(request["params"]["targetId"], "review");
                            json!({"success":true})
                        }
                        _ => unreachable!(),
                    };
                    ws.send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .unwrap();
                }
            }
        });

        let url = "https://example.test/design";
        let out = start
            .env("TEST_CDP", endpoint)
            .args(["start", url, "--profile", profile, "--session", "example"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let calls = fs::read_to_string(root.join("browser-calls")).unwrap();
        let mut calls = calls.lines();
        if profile == "vpn" {
            assert_eq!(calls.next(), Some("vpn start"));
        }
        let attach = calls.next().unwrap();
        let prefix = if profile == "vpn" {
            "ab --profile vpn"
        } else {
            "ab"
        };
        assert!(attach.starts_with(&format!("{prefix} --session example-")));
        assert!(attach.contains(" --headed --pin-tab --init-script "));
        assert!(attach.contains(" open about:blank#vega-review-"));
        let session = attach
            .split_whitespace()
            .skip_while(|arg| *arg != "--session")
            .nth(1)
            .unwrap();
        assert!(fs::read_to_string(root.join("agent-calls"))
            .unwrap()
            .contains(&format!("--session {session} open {url}")));

        let mut stop = std::process::Command::new(env!("CARGO_BIN_EXE_vega-review"));
        stop.env_clear();
        for (name, value) in start.get_envs() {
            stop.env(name, value.unwrap());
        }
        let out = stop.arg("stop").output().unwrap();
        assert!(out.status.success(), "{out:?}");
        cdp.join().unwrap();
        let calls = fs::read_to_string(root.join("browser-calls")).unwrap();
        assert_eq!(calls.lines().last().unwrap(), format!("close {session}"));
    }
}
