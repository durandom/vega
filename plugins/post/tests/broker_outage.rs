//! A broker that goes away while vegad runs (docs/design/vegad.md, Broker
//! outage, vega-lhc.205): a local nats-server, killed mid-run and started
//! again on the same port. vegad keeps labelling herdr's panes, logs the
//! outage once, notes it for the human and reconnects. Temp HOME, fake herdr.

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Kills the process when the test ends, pass or fail.
struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Two claude panes; w1:p1 closes once `$FAKE_LOG/p1-closed` exists.
const FAKE_HERDR: &str = r#"#!/bin/sh
echo "$*" >> "$FAKE_LOG/herdr-calls"
p1='{"pane_id":"w1:p1","workspace_id":"w1","cwd":"/","agent":"claude","agent_status":"idle"},'
[ -e "$FAKE_LOG/p1-closed" ] && p1=''
p2='{"pane_id":"w1:p2","workspace_id":"w1","cwd":"/","agent":"claude","agent_status":"working"}'
case "$1 $2" in
  "api schema") echo '{"error":{"code":"unknown","message":"no socket api"}}'; exit 1 ;;
  "agent list") echo "{\"result\":{\"agents\":[$p1$p2]}}" ;;
  "pane list") echo "{\"result\":{\"panes\":[$p1$p2]}}" ;;
  "pane process-info") echo "{\"result\":{\"process_info\":{\"shell_pid\":${FAKE_SENDER_PID:-0}}}}" ;;
  "workspace list") echo '{"result":{"workspaces":[{"workspace_id":"w1","label":"ws"}]}}' ;;
  *) echo '{"result":{}}' ;;
esac
"#;

fn broker(conf: &Path) -> Proc {
    Proc(
        Command::new("nats-server")
            .arg("-c")
            .arg(conf)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("nats-server on PATH (mise)"),
    )
}

#[track_caller]
fn wait_for(log: &Path, what: &str, within: Duration, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + within;
    while !ready() {
        assert!(
            Instant::now() < until,
            "timed out waiting for {what}; vegad log:\n{}",
            fs::read_to_string(log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn metadata_reports(home: &Path) -> usize {
    fs::read_to_string(home.join("herdr-calls"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("pane report-metadata w1:p2"))
        .count()
}

#[test]
fn a_lost_broker_leaves_the_sidebar_running_and_vegad_reconnects() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let home = home.as_path();
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("herdr"), FAKE_HERDR).unwrap();
    fs::set_permissions(bin.join("herdr"), fs::Permissions::from_mode(0o755)).unwrap();
    let state = home.join("state/vega");
    // Startup past the one-time identity cutover.
    let cutover = state.join("identity-cutover");
    fs::create_dir_all(&cutover).unwrap();
    let plan = serde_json::json!({"schema":2,"host":"testhost","inbox_only":false,"mappings":[],"original":{},"writes":{},"remove":[],"registry":{"host":"testhost","backup":{},"projection":{}},"registry_reads":[],"owned":{}});
    fs::write(cutover.join("journal.json"), plan.to_string()).unwrap();
    fs::write(
        cutover.join("approved.json"),
        serde_json::json!({"schema":2,"hosts":[plan],"grants":{}}).to_string(),
    )
    .unwrap();
    fs::write(
        cutover.join("complete.json"),
        r#"{"schema":2,"host":"testhost"}"#,
    )
    .unwrap();

    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let conf = home.join("nats.conf");
    fs::write(
        &conf,
        format!(
            "listen: 127.0.0.1:{port}\njetstream {{ store_dir: \"{}\" }}\n",
            home.join("js").display()
        ),
    )
    .unwrap();
    let mut nats = broker(&conf);
    let url = format!("nats://127.0.0.1:{port}");
    // The registry bucket, as titan has it; identity startup reads it.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut client = None;
            for _ in 0..100 {
                if let Ok(c) = async_nats::connect(&url).await {
                    client = Some(c);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            async_nats::jetstream::new(client.expect("broker starts"))
                .create_key_value(async_nats::jetstream::kv::Config {
                    bucket: vega_registry::BUCKET.into(),
                    history: 1,
                    ..Default::default()
                })
                .await
                .unwrap();
        });

    let exe = env!("CARGO_BIN_EXE_vega-post");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let cmd = |args: &[&str]| {
        let mut c = Command::new(exe);
        c.args(args)
            .env_clear()
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("PATH", &path)
            .env("VEGA_NATS_URL", &url)
            .env("VEGA_POST_HOST", "testhost")
            .env("FAKE_LOG", home)
            .current_dir(home);
        c
    };
    let log_file = home.join("relay.log");
    let mut relay = cmd(&["relay"]);
    relay
        .stdout(fs::File::create(&log_file).unwrap())
        .stderr(Stdio::null());
    let relay = Proc(relay.spawn().unwrap());
    let log = || fs::read_to_string(&log_file).unwrap_or_default();
    let broker_state =
        || fs::read_to_string(state.join("post-relay-testhost.broker")).unwrap_or_default();

    wait_for(
        &log_file,
        "the broker loop",
        Duration::from_secs(30),
        || log().contains("relay: testhost watching"),
    );
    assert_eq!(broker_state().trim(), "connected");
    // w1:p2 mails w1:p1, which closes unread during the outage.
    let sent = cmd(&["send", "w1:p1", "hello"])
        .env("HERDR_PANE_ID", "w1:p2")
        // The test hosts `send` in w1:p2.
        .env("FAKE_SENDER_PID", std::process::id().to_string())
        .output()
        .unwrap();
    assert!(sent.status.success(), "{sent:?}");
    wait_for(&log_file, "the mail", Duration::from_secs(20), || {
        log().contains("seen: w1:p2 -> w1:p1")
    });

    // The broker goes away.
    let _ = nats.0.kill();
    let _ = nats.0.wait();
    wait_for(&log_file, "local mode", Duration::from_secs(40), || {
        broker_state().starts_with("local")
    });
    let lost = log();
    assert_eq!(
        lost.matches("broker: lost").count(),
        1,
        "the outage is logged once:\n{lost}"
    );
    assert!(!lost.contains("count failed"), "{lost}");
    let notes = || -> Vec<serde_json::Value> {
        fs::read_dir(state.join("notes"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok())
            .filter(|n: &serde_json::Value| n["kind"] == "broker")
            .collect()
    };
    let outage = notes();
    assert_eq!(outage.len(), 1, "{outage:?}");
    assert!(outage[0]["resolved"].is_null(), "{outage:?}");
    // The sidebar keeps its labels without the broker.
    let before = metadata_reports(home);
    wait_for(
        &log_file,
        "metadata without the broker",
        Duration::from_secs(40),
        || metadata_reports(home) > before,
    );
    assert!(
        relay.0.id() > 0 && cmd(&["relay", "status"]).output().is_ok(),
        "vegad answers"
    );
    fs::write(home.join("p1-closed"), "").unwrap();
    wait_for(
        &log_file,
        "the closed pane",
        Duration::from_secs(20),
        || log().contains("bounce: w1:p1 closed with unread mail; bounced once the broker is back"),
    );

    // The broker comes back on the same port.
    let _nats = broker(&conf);
    wait_for(&log_file, "the reconnect", Duration::from_secs(60), || {
        broker_state().trim() == "connected"
            && log().matches("relay: testhost watching").count() == 2
    });
    let back = log();
    eprintln!("{back}");
    assert!(
        back.contains("broker: reachable, leaving local mode"),
        "{back}"
    );
    assert!(back.contains("broker: outage note cleared"), "{back}");
    assert_eq!(back.matches("broker: lost").count(), 1, "{back}");
    let outage = notes();
    assert!(outage[0]["resolved"].is_i64(), "{outage:?}");
    // The sender hears of the closed pane after all.
    wait_for(&log_file, "the bounce", Duration::from_secs(20), || {
        log().contains("bounce: w1:p1 unread, mailed w1:p2 on testhost")
    });
}
