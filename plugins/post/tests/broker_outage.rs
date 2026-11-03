//! A broker that goes away while vegad runs (docs/design/vegad.md, Broker
//! outage, vega-lhc.205): a local nats-server, killed mid-run and started
//! again on the same port. vegad keeps labelling herdr's panes, logs the
//! outage once, notes it for the human and reconnects. Temp HOME, fake herdr.
//!
//! Each phase may meet any step of the probe backoff, so the waits below come
//! from vegad's own bounds. The sidebar renewal takes 60 s of local mode, which
//! leaves the broker's return in the slow part of the backoff: about 2.5 minutes.

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Local mode's longest gap between probes (`PROBE_EVERY`, main.rs).
const PROBE_GAP: Duration = Duration::from_secs(60);
/// One bounded broker step: the probe's connect, the setup (`outage::CALL`).
const BROKER_CALL: Duration = Duration::from_secs(10);
/// The sidebar sends an unchanged row again after 60 s (`RENEW_AFTER`) on a
/// 5 s tick (`TICK`, sidebar.rs). A row can still change at the first local
/// refresh, up to one `REGISTRY_EVERY` (15 s, main.rs) into local mode.
const RENEWAL: Duration = Duration::from_secs(60 + 5 + 15);
/// Headroom for a machine busy with a full check. Only a failure waits this.
const LOAD: Duration = Duration::from_secs(20);

/// The next probe at the worst backoff step, its connect and the setup.
fn through_next_probe() -> Duration {
    PROBE_GAP + BROKER_CALL + BROKER_CALL + LOAD
}

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

fn metadata_reports(home: &Path) -> Vec<String> {
    fs::read_to_string(home.join("herdr-calls"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("pane report-metadata w1:p2"))
        .map(String::from)
        .collect()
}

/// Whether a report from index `from` on repeats the one before it: a renewal
/// of an unchanged row, not the first report or a changed one.
fn renewed_since(reports: &[String], from: usize) -> bool {
    reports
        .windows(2)
        .enumerate()
        .any(|(i, pair)| i + 1 >= from && pair[0] == pair[1])
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
    // No counts while the client is away: only one in flight at the kill can
    // fail (docs/design/vegad.md, Broker outage), not one per poll.
    assert!(lost.matches("count failed").count() <= 1, "{lost}");
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
    // Where local mode starts in the reports: its first refresh sends within
    // about 100 ms, so it may or may not be counted here. Either way a renewal
    // lies ahead.
    let before = metadata_reports(home).len();
    fs::write(home.join("p1-closed"), "").unwrap();
    wait_for(
        &log_file,
        "the closed pane",
        Duration::from_secs(20),
        || log().contains("bounce: w1:p1 closed with unread mail; bounced once the broker is back"),
    );

    // A broker that accepts the connection but not the setup (no JetStream)
    // does not end the outage: same note, still local.
    let bare = home.join("bare.conf");
    fs::write(&bare, format!("listen: 127.0.0.1:{port}\n")).unwrap();
    let mut half = broker(&bare);
    wait_for(&log_file, "the failed setup", through_next_probe(), || {
        log().contains("broker: setup failed")
    });
    wait_for(
        &log_file,
        "local mode again",
        Duration::from_secs(10),
        || broker_state().starts_with("local"),
    );
    let outage = notes();
    assert_eq!(outage.len(), 1, "one note per outage: {outage:?}");
    assert!(outage[0]["resolved"].is_null(), "{outage:?}");
    assert!(!log().contains("outage note cleared"), "{}", log());
    let _ = half.0.kill();
    let _ = half.0.wait();

    // The sidebar keeps its labels without the broker: the unchanged row is
    // renewed, whichever side of `before` its first report fell on.
    wait_for(
        &log_file,
        "a metadata renewal without the broker",
        RENEWAL + LOAD,
        || renewed_since(&metadata_reports(home), before),
    );
    assert!(
        broker_state().starts_with("local"),
        "renewed in local mode: {}",
        broker_state()
    );
    assert!(
        relay.0.id() > 0 && cmd(&["relay", "status"]).output().is_ok(),
        "vegad answers"
    );

    // The broker comes back on the same port.
    let _nats = broker(&conf);
    wait_for(&log_file, "the reconnect", through_next_probe(), || {
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
    // The sender hears of the closed pane after all: a first try that fails
    // is retried after 30 s (docs/design/vegad.md, Broker outage).
    wait_for(
        &log_file,
        "the bounce",
        Duration::from_secs(30) + BROKER_CALL + LOAD,
        || log().contains("bounce: w1:p1 unread, mailed w1:p2 on testhost"),
    );
}

#[test]
fn only_a_repeated_report_counts_as_a_renewal() {
    let r = |lines: &[&str]| lines.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    assert!(!renewed_since(&r(&[]), 0));
    assert!(!renewed_since(&r(&["a"]), 0), "a first report");
    assert!(!renewed_since(&r(&["a", "b"]), 0), "a changed row");
    assert!(renewed_since(&r(&["a", "a"]), 0));
    assert!(
        renewed_since(&r(&["a", "a"]), 1),
        "counted first, renewed after"
    );
    assert!(
        !renewed_since(&r(&["a", "a"]), 2),
        "renewed before local mode"
    );
    assert!(renewed_since(&r(&["a", "a", "b", "b"]), 2));
}
