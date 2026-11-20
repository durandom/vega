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
fn review_routes_and_bypasses_csp_before_navigation() {
    use serde_json::{json, Value};
    use std::{fs, net::TcpListener, os::unix::fs::PermissionsExt, thread, time::Duration};
    use tungstenite::Message;

    for route in ["global", "vpn", "dev", "headless", "external"] {
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
  ab|dev)
    for last do :; done
    case "$last" in
      about:blank*) printf '%s' "$last" > "$HOME/blank" ;;
      cdp-url) echo "$TEST_CDP" ;;
      http:*|https:*) : > "$HOME/navigated" ;;
    esac
    ;;
  close|vpn) ;;
  *) exit 1 ;;
esac
"#,
            ),
            (
                "agent-browser",
                r#"
printf '%s\n' "$*" >> "$HOME/agent-calls"
for last do :; done
case "$last" in
  about:blank*) printf '%s' "$last" > "$HOME/blank" ;;
  cdp-url) echo "$TEST_CDP" ;;
  http:*|https:*) : > "$HOME/navigated" ;;
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
            let calls = [
                &[
                    "Target.getTargets",
                    "Target.attachToTarget",
                    "Page.setBypassCSP",
                ][..],
                &["Target.getTargets", "Target.closeTarget"][..],
            ];
            let count = if route == "external" { 2 } else { 1 };
            for expected in calls.iter().take(count).copied() {
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

        let url = if matches!(route, "global" | "vpn") {
            "https://example.test/design"
        } else {
            "http://127.0.0.1:5000/design"
        };
        start
            .env("TEST_CDP", &endpoint)
            .args(["start", url, "--session", "example"]);
        match route {
            "global" | "vpn" => {
                start.args(["--profile", route]);
            }
            "headless" => {
                start.arg("--headless").env("AGENT_BROWSER_CDP", &endpoint);
            }
            "external" => {
                start.env("AGENT_BROWSER_CDP", &endpoint);
            }
            _ => {}
        }
        let out = start.output().unwrap();
        assert!(out.status.success(), "{route}: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let session = stdout
            .lines()
            .find_map(|line| line.strip_prefix("browser: agent-browser session "))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap();
        assert!(session.starts_with("example-"));
        let browser_calls = fs::read_to_string(root.join("browser-calls")).unwrap_or_default();
        let agent_calls = fs::read_to_string(root.join("agent-calls")).unwrap_or_default();
        let blank_call = browser_calls
            .lines()
            .chain(agent_calls.lines())
            .find(|line| line.contains(" open about:blank#vega-review-"))
            .unwrap();
        match route {
            "global" => assert!(
                blank_call.starts_with(&format!("ab --session {session} --headed --pin-tab"))
            ),
            "vpn" => {
                assert!(browser_calls.starts_with("vpn start\n"));
                assert!(blank_call.starts_with(&format!(
                    "ab --profile vpn --session {session} --headed --pin-tab"
                )));
            }
            "dev" => {
                assert!(blank_call.starts_with(&format!("dev --session {session} --init-script ")));
                assert!(agent_calls.is_empty());
                assert_eq!(
                    browser_calls.lines().last().unwrap(),
                    format!("dev --session {session} tab t1")
                );
            }
            "headless" => {
                assert!(blank_call.starts_with(&format!("--session {session} --config ")));
                assert!(blank_call.contains(" --headed false "));
                assert!(browser_calls.is_empty());
            }
            "external" => {
                assert!(blank_call.starts_with(&format!("--session {session} --headed --pin-tab")));
                assert!(browser_calls.is_empty());
            }
            _ => unreachable!(),
        }
        if matches!(route, "global" | "vpn") {
            assert!(agent_calls.is_empty());
            assert!(
                browser_calls
                    .lines()
                    .any(|line| line
                        == format!("ab --profile {route} --session {session} open {url}"))
            );
        }
        assert!(browser_calls
            .lines()
            .chain(agent_calls.lines())
            .any(|line| line.contains(session) && line.ends_with(&format!(" open {url}"))));

        let mut stop = std::process::Command::new(env!("CARGO_BIN_EXE_vega-review"));
        stop.env_clear();
        for (name, value) in start.get_envs() {
            stop.env(name, value.unwrap());
        }
        let out = stop.arg("stop").output().unwrap();
        assert!(out.status.success(), "{out:?}");
        cdp.join().unwrap();
        let (log, expected) = match route {
            "dev" => ("browser-calls", format!("dev --session {session} close")),
            "global" | "vpn" => ("browser-calls", format!("close {session}")),
            "headless" | "external" => ("agent-calls", " close".to_owned()),
            _ => unreachable!(),
        };
        let calls = fs::read_to_string(root.join(log)).unwrap();
        assert!(
            calls.lines().last().unwrap().ends_with(&expected),
            "{calls}"
        );
    }
}
