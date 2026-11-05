//! Simulated inbox-only host: local herdr delivery must need no broker;
//! an explicit remote pane must still use quarantine, never an inbox publish.
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

const HERDR: &str = r#"#!/bin/sh
case "$1 $2" in
  "pane list") printf '{"result":{"panes":[{"pane_id":"w1:p1","agent":"codex","cwd":"%s"}]}}' "$HOME" ;;
  "pane process-info") printf '{"result":{"process_info":{"shell_pid":%s}}}' "$FAKE_PID" ;;
  "agent list") echo '{"result":{"agents":[{"pane_id":"w1:p1"},{"pane_id":"w1:p2"},{"pane_id":"w1:p3"}]}}' ;;
  "agent get")
    case "$3" in
      own) pane=w1:p1 ;;
      w1:p1|w1:p2|w1:p3) pane="$3" ;;
      *) echo '{"error":{"code":"agent_not_found","message":"closed"}}'; exit 1 ;;
    esac
    printf '{"result":{"agent":{"pane_id":"%s"}}}' "$pane" ;;
  "agent prompt")
    printf '%s\n%s\n' "$3" "$4" >> "$HOME/prompts"
    printf '%s\n' "$4" > "$HOME/current-prompt"
    if test -f "$HOME/new-brief"; then
      mkdir -p "$HOME/state/vega/briefs"
      cat "$HOME/new-brief" > "$HOME/state/vega/briefs/local_w1_p1"
    fi
    echo '{"result":{}}' ;;
  "agent wait")
    if test -f "$HOME/stall"; then
      echo '{"error":{"code":"timeout","message":"not working"}}'; exit 1
    fi
    echo '{"result":{}}' ;;
  "pane read")
    if test -f "$HOME/stage" && ! test -f "$HOME/entered"; then
      printf '› '; cat "$HOME/current-prompt"
    else
      echo '› Ask Codex to do anything'
    fi ;;
  "agent send-keys")
    test "$4" = enter || exit 1
    printf '%s\n' enter >> "$HOME/entered"
    echo '{"result":{}}' ;;
  *) echo '{"result":{}}' ;;
esac
"#;

fn fixture(home: &Path) {
    fs::create_dir_all(home.join("bin")).unwrap();
    let herdr = home.join("bin/herdr");
    fs::write(&herdr, HERDR).unwrap();
    fs::set_permissions(herdr, fs::Permissions::from_mode(0o755)).unwrap();
}

fn command(home: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_vega-post"));
    cmd.args(args)
        .current_dir(home)
        .env_clear()
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        )
        .env("HERDR_PANE_ID", "w1:p1")
        .env("FAKE_PID", std::process::id().to_string())
        .env("VEGA_POST_HOST", "inbox-test")
        .env("VEGA_POST_INBOX_ONLY", "inbox-test")
        .env("VEGA_NATS_URL", "nats://127.0.0.1:9")
        .env("VEGA_NATS_NKEY", "/must-not-read-this-seed");
    cmd
}

fn run(home: &Path, args: &[&str], success: bool) -> String {
    let spool = home.join("state/vega/history-outbox/inbox-test");
    let before = vega_herdr::history::pending(&spool).unwrap().len();
    let out = command(home, args).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.success(), success, "{args:?}: {text}");
    assert_eq!(
        vega_herdr::history::pending(&spool).unwrap().len(),
        before + usize::from(success),
        "only confirmed local delivery should enqueue one history receipt"
    );
    text
}

fn claim(home: &Path, project: &str, pane: &str, owner: &str, alias: &str) {
    let dir = home.join("state/vega/uac/v2");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!(
            "{}.json",
            vega_herdr::project::encode_component(project)
        )),
        json!({"schema":2,"project":project,"pane":pane,"host":owner,"at":1,
            "folder_name":"folder","origin_name":"origin","aliases":[alias]})
        .to_string(),
    )
    .unwrap();
}

#[test]
fn local_panes_and_local_uac_claims_need_no_broker() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    fixture(home);
    // Local history starts empty and queues without contacting the broker.
    assert!(!home.join("state/vega/history-outbox").exists());
    claim(
        home,
        "github.com/test/demo",
        "w1:p2",
        "inbox-test",
        "merged",
    );
    for args in [
        vec!["send", "w1:p2", "hello"],
        vec!["send", "--host", "inbox-test", "w1:p2", "hello"],
        vec!["send", "--now", "w1:p2", "hello"],
        vec!["send", "--uac", "github.com/test/demo", "hello"],
        vec!["send", "--uac", "demo", "hello"],
        vec!["send", "--uac", "folder", "hello"],
        vec!["send", "--uac", "origin", "hello"],
        vec!["send", "--uac", "merged", "hello"],
    ] {
        assert!(run(home, &args, true).contains("locally via herdr"));
    }
    // Resolve `.` from an actual temporary checkout, still without a broker.
    for args in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/test/demo.git",
        ],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(home)
            .status()
            .unwrap()
            .success());
    }
    assert!(run(home, &["send", "--uac", ".", "hello"], true).contains("locally via herdr"));
    let prompts = fs::read_to_string(home.join("prompts")).unwrap();
    assert_eq!(prompts.matches("[vega-mux] from w1:p1").count(), 9);
    assert!(prompts.contains("vega post send w1:p1 --re "));
    assert!(!prompts.contains("quarantine"));
    for target in ["w1:p1", "own"] {
        assert!(run(home, &["send", target, "hello"], false).contains("self_send"));
    }
    assert!(run(home, &["send", "closed", "hello"], false).contains("agent_not_found"));
    let missing = run(home, &["send", "--uac", "missing", "hello"], false);
    assert!(missing.contains("no_uac") && missing.contains("vega mux"));
    claim(home, "github.com/test/dead", "w1:p9", "inbox-test", "dead");
    assert!(run(home, &["send", "--uac", "dead", "hello"], false).contains("no_uac"));
    claim(home, "github.com/test/remote", "w1:p2", "remote", "remote");
    assert!(run(home, &["send", "--uac", "remote", "hello"], false).contains("no_uac"));
    claim(
        home,
        "github.com/test/other",
        "w1:p3",
        "inbox-test",
        "merged",
    );
    assert!(run(home, &["send", "--uac", "merged", "hello"], false).contains("ambiguous_uac"));
    assert!(run(home, &["send", "--uber", "hello"], false).contains("vega mux"));

    // A same-host done report keeps parent supervision and queues local history.
    fs::create_dir_all(home.join("state/vega/parents")).unwrap();
    fs::write(
        home.join("state/vega/parents/local_w1_p1"),
        json!({"name":"child","parent":"w1:p2","host":"inbox-test"}).to_string(),
    )
    .unwrap();
    let done = run(
        home,
        &[
            "done",
            "--to",
            "w1:p2",
            "--outcome",
            "succeeded",
            "finished",
        ],
        true,
    );
    assert!(done.contains("done: succeeded") && done.contains("locally via herdr"));
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(home.join("state/vega/done/local_w1_p1")).unwrap())
            .unwrap();
    assert_eq!(record["outcome"], "succeeded");
    assert_eq!(record["summary"], "finished");

    // A newer parent brief arriving while herdr takes the done prompt must
    // keep the report from becoming the outcome of the newer task.
    fs::remove_file(home.join("state/vega/done/local_w1_p1")).unwrap();
    let future = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 60_000;
    fs::write(home.join("new-brief"), future.to_string()).unwrap();
    run(
        home,
        &["done", "--to", "w1:p2", "--outcome", "failed", "older task"],
        true,
    );
    assert!(!home.join("state/vega/done/local_w1_p1").exists());
    assert!(home.join("state/vega/replied/local_w1_p1").exists());
    let prompts = fs::read_to_string(home.join("prompts")).unwrap();
    assert!(prompts.contains("stale: report from w1:p1"));
    assert!(prompts.contains("your newer brief stands"));
    assert!(prompts.contains("a newer brief supersedes this report"));
}

#[test]
fn staged_prompt_gets_enter_only_and_an_unconfirmed_send_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    fixture(home);
    fs::write(home.join("stage"), "").unwrap();
    run(home, &["send", "w1:p2", "staged hello"], true);
    assert_eq!(fs::read_to_string(home.join("entered")).unwrap(), "enter\n");
    assert_eq!(
        fs::read_to_string(home.join("prompts"))
            .unwrap()
            .matches("staged hello")
            .count(),
        1
    );
    fs::write(home.join("stall"), "").unwrap();
    let failed = run(home, &["send", "w1:p2", "never taken"], false);
    assert!(failed.contains("not_taken"));
    assert!(!failed.contains("sent:"));
    assert_eq!(
        fs::read_to_string(home.join("prompts"))
            .unwrap()
            .matches("never taken")
            .count(),
        1
    );
}

#[test]
fn same_host_machine_label_keeps_its_supervision_key() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    fixture(home);
    fs::create_dir_all(home.join("state/vega/parents")).unwrap();
    fs::write(
        home.join("state/vega/parents/inbox-test_w1_p2"),
        json!({"name":"child","parent":"w1:p1","host":"inbox-test"}).to_string(),
    )
    .unwrap();
    run(home, &["send", "w1:p2", "new work"], true);
    assert!(home.join("state/vega/briefs/inbox-test_w1_p2").exists());
    assert!(!home.join("state/vega/briefs/local_w1_p2").exists());
}

#[test]
fn explicit_remote_target_uses_quarantine() {
    use futures_util::StreamExt;
    use std::{net::TcpListener, process::Stdio, time::Duration};
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    fixture(home);
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Server(
        Command::new("nats-server")
            .args(["-a", "127.0.0.1", "-p", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let url = format!("nats://127.0.0.1:{port}");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut client = None;
            for _ in 0..50 {
                if let Ok(c) = async_nats::connect(&url).await {
                    client = Some(c);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let client = client.expect("test broker starts");
            let mut requests = client.subscribe("vega.>").await.unwrap();
            client.flush().await.unwrap();
            let mut cmd = command(home, &["send", "--host", "remote", "w2:p1", "hello"]);
            cmd.env("VEGA_NATS_URL", url).env_remove("VEGA_NATS_NKEY");
            let sender = tokio::task::spawn_blocking(move || cmd.output().unwrap());
            let req = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                req.subject.as_str(),
                "vega.quarantine.remote.w2:p1.inbox-test"
            );
            assert_eq!(req.payload.as_ref(), b"hello");
            client
                .publish(req.reply.unwrap(), r#"{"ok":true,"code":"stored"}"#.into())
                .await
                .unwrap();
            client.flush().await.unwrap();
            let out = sender.await.unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            assert!(String::from_utf8_lossy(&out.stdout).contains("quarantined:"));
            assert!(!home.join("prompts").exists());
        });
}

#[test]
fn log_without_pane_lists_the_archive_across_panes() {
    use std::{net::TcpListener, process::Stdio, time::Duration};
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    fixture(home);
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Server(
        Command::new("nats-server")
            .args(["-js", "-a", "127.0.0.1", "-p", &port.to_string()])
            .arg("-sd")
            .arg(home.join("js"))
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
    let js = rt.block_on(async {
        for _ in 0..50 {
            if let Ok(c) = async_nats::connect(&url).await {
                return async_nats::jetstream::new(c);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("test broker did not start");
    });
    let logcmd = |args: &[&str]| {
        let mut cmd = command(home, args);
        cmd.env("VEGA_NATS_URL", &url).env_remove("VEGA_NATS_NKEY");
        cmd.output().unwrap()
    };
    // The first read creates the inbox stream (no machine key).
    assert!(logcmd(&["log"]).status.success());
    rt.block_on(async {
        for (pane, text) in [("w1:p1", "alpha"), ("w2:p1", "beta"), ("w3:p1", "gamma")] {
            let ack = js
                .publish(format!("vega.inbox.inbox-test.{pane}.other"), text.into())
                .await
                .unwrap();
            ack.await.unwrap();
        }
    });
    let out = logcmd(&["log"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("3 of 3 messages"), "{text}");
    for t in ["alpha", "beta", "gamma"] {
        assert!(text.contains(t), "{text}");
    }
}
