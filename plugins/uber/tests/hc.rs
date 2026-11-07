#[path = "../../../crates/vega-herdr/tests/fixtures/own-terminal.rs"]
mod common;

use std::process::{Command, Output, Stdio};

fn run(base: &Command, args: &[&str]) -> Output {
    Command::new(base.get_program())
        .env_clear()
        .envs(base.get_envs().filter_map(|(k, v)| v.map(|v| (k, v))))
        .args(args)
        .output()
        .unwrap()
}

struct Broker(std::process::Child);
impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn broker(dir: &std::path::Path) -> (Broker, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let server = Broker(
        Command::new("nats-server")
            .args(["-a", "127.0.0.1", "-p", &port.to_string(), "-js", "-sd"])
            .arg(dir.join("js"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let url = format!("nats://127.0.0.1:{port}");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        for _ in 0..50 {
            if let Ok(client) = async_nats::connect(&url).await {
                async_nats::jetstream::new(client)
                    .create_key_value(async_nats::jetstream::kv::Config {
                        bucket: vega_registry::BUCKET.into(),
                        ..Default::default()
                    })
                    .await
                    .unwrap();
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("test broker did not start");
    });
    (server, url)
}

/// Schema-checked RPC responses, including fleet and finished terminals that
/// must not count as this host's live claim holder.
fn inventory(dir: &std::path::Path, malformed: bool) {
    let terminal = serde_json::json!({"argv":[],"cwd":"/fixture","created_ms":0,
        "screen":"exact","cols":80,"rows":24,"labels":{},"state":"live",
        "address":"t7","pid":std::process::id(),"name":"child","parent":"t2",
        "status":{"app":"claude-code","state":"idle"},
        "foreground":{"pid":1,"name":"claude","argv":["claude"],"cwd":"/project"}});
    let mut remote = terminal.clone();
    remote["address"] = "other/t8".into();
    let mut finished = terminal.clone();
    finished["address"] = "t9".into();
    finished["state"] = "finished".into();
    let list = if malformed {
        serde_json::json!({"terminals":[{"address":"t7"}]})
    } else {
        serde_json::json!({"hidden_finished":0,"machines":[],"terminals":[terminal, remote, finished]})
    };
    std::fs::write(
        dir.join("bin/hc"),
        format!(
            r#"#!/bin/sh
[ "$1" = connect ] || exit 1
read -r request
case "$request" in
  *terminal.get*) printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{terminal}}}' ;;
  *terminal.list*) printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{list}}}' ;;
  *) exit 1 ;;
esac
"#
        ),
    )
    .unwrap();
}

#[test]
fn hc_status_claim_and_release_use_schema_checked_local_inventory() {
    let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-uber"));
    inventory(dir.path(), false);
    let (_server, url) = broker(dir.path());
    c.env("VEGA_NATS_URL", url);
    for args in [["claim"], ["status"], ["release"]] {
        let o = run(&c, &args);
        assert!(o.status.success(), "{o:?}");
        if args == ["status"] {
            assert!(
                String::from_utf8_lossy(&o.stdout).contains("t7 on worker (you)"),
                "{o:?}"
            );
        }
    }
    let file = dir.path().join("state/vega/uber/claim.json");
    for pane in ["t8", "t9"] {
        vega_registry::uber_state::save(
            &file,
            &vega_registry::uber_state::value(pane, "worker", 0, 1),
        )
        .unwrap();
        let o = run(&c, &["claim"]);
        assert!(o.status.success(), "{o:?}");
        assert!(
            String::from_utf8_lossy(&o.stdout)
                .contains(&format!("took_over: {pane} on worker (gone)")),
            "{o:?}"
        );
        assert!(run(&c, &["release"]).status.success());
    }
    inventory(dir.path(), true);
    let o = run(&c, &["status"]);
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("hc_bad_output"),
        "{o:?}"
    );
}

#[test]
fn hc_and_tui_claims_cannot_take_over_each_other_on_the_same_host() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-uber"));
    inventory(dir.path(), false);
    let herdr = dir.path().join("bin/herdr");
    std::fs::write(&herdr, format!(r#"#!/bin/sh
case "$*" in
  *'pane list'*) printf '%s\n' '{{"result":{{"panes":[{{"pane_id":"w1:p1","agent":"claude","cwd":"/project"}}]}}}}' ;;
  *'pane process-info'*) printf '%s\n' '{{"result":{{"process_info":{{"shell_pid":{}}}}}}}' ;;
  *) exit 1 ;;
esac
"#, std::process::id())).unwrap();
    std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (_server, url) = broker(dir.path());
    c.env("VEGA_NATS_URL", url);
    for tui_first in [false, true] {
        let context = |c: &mut Command, tui: bool| {
            if tui {
                c.env("HERDR_PANE_ID", "w1:p1");
            } else {
                c.env_remove("HERDR_PANE_ID");
            }
        };
        context(&mut c, tui_first);
        let o = run(&c, &["claim"]);
        assert!(o.status.success(), "{o:?}");
        context(&mut c, !tui_first);
        let o = run(&c, &["claim"]);
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("uber_exists"),
            "{o:?}"
        );
        let o = run(&c, &["release"]);
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("not_uber"),
            "{o:?}"
        );
        context(&mut c, tui_first);
        assert!(run(&c, &["status"]).status.success());
        assert!(run(&c, &["release"]).status.success());
    }
    // The TUI also works alone, with no hc executable.
    c.env("HERDR_PANE_ID", "w1:p1");
    std::fs::rename(
        dir.path().join("bin/hc"),
        dir.path().join("bin/hc-disabled"),
    )
    .unwrap();
    let o = run(&c, &["claim"]);
    assert!(o.status.success(), "{o:?}");
    assert!(run(&c, &["release"]).status.success());
    std::fs::rename(
        dir.path().join("bin/hc-disabled"),
        dir.path().join("bin/hc"),
    )
    .unwrap();
    // Losing the incumbent's backend is not evidence that its terminal died.
    vega_registry::uber_state::save(
        &dir.path().join("state/vega/uber/claim.json"),
        &vega_registry::uber_state::value("t7", "worker", 0, 1),
    )
    .unwrap();
    c.env("HERDR_PANE_ID", "w1:p1");
    inventory(dir.path(), true);
    let o = run(&c, &["claim"]);
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("hc_bad_output"),
        "{o:?}"
    );
    // Mirror direction: an hc caller cannot replace a herdr-held claim when
    // the herdr inventory is unavailable.
    inventory(dir.path(), false);
    vega_registry::uber_state::remove(&dir.path().join("state/vega/uber/claim.json")).unwrap();
    let o = run(&c, &["claim"]);
    assert!(o.status.success(), "{o:?}");
    std::fs::rename(&herdr, dir.path().join("bin/herdr-disabled")).unwrap();
    c.env_remove("HERDR_PANE_ID");
    let o = run(&c, &["claim"]);
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stdout.contains("herdr_not_found"), "{o:?}");
    assert!(!stdout.contains("took_over:"), "{o:?}");
    std::fs::rename(dir.path().join("bin/herdr-disabled"), &herdr).unwrap();
    std::fs::write(&herdr, "#!/bin/sh\nprintf '%s\\n' '{\"result\":{}}'\n").unwrap();
    let o = run(&c, &["claim"]);
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("herdr_bad_output"),
        "{o:?}"
    );
    assert_eq!(
        vega_registry::uber_state::local(&dir.path().join("state/vega/uber/claim.json"), "worker")
            .unwrap()["pane"],
        "w1:p1"
    );
}

#[test]
fn hc_answer_checks_claim_and_reads_exact_harness_session_log() {
    for (key, path) in [
        (
            "CLAUDE_CODE_SESSION_ID",
            ".claude/projects/-project/session-7.jsonl",
        ),
        (
            "CODEX_THREAD_ID",
            ".codex/sessions/2026/11/06/rollout-test-session-7.jsonl",
        ),
    ] {
        let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-uber"));
        c.env_remove("CODEX_THREAD_ID")
            .env(key, "session-7")
            .args(["answer", "missing"]);
        let o = c.output().unwrap();
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("not_uber"),
            "{o:?}"
        );
        let claim = dir.path().join("state/vega/uber/claim.json");
        std::fs::create_dir_all(claim.parent().unwrap()).unwrap();
        std::fs::write(
            claim,
            r#"{"schema":1,"pane":"t7","host":"worker","at":1,"epoch":1}"#,
        )
        .unwrap();
        let o = c.output().unwrap();
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("no_session_log"),
            "{o:?}"
        );
        let log = dir.path().join(path);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(log, "{}\n").unwrap();
        let o = c.output().unwrap();
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("no_ask"),
            "{o:?}"
        );
        let o = c.env("TEST_PARENT_PID", "4294967295").output().unwrap();
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("not_in_terminal"),
            "{o:?}"
        );
    }
}
