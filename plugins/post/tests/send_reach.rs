//! `vega post send` checks the target in the registry (vega-lhc.129): a pane
//! its fresh host no longer lists fails with pane_gone and names the
//! project's UAC; a stale host gets the mail with a warning; `--uac`
//! reaches the project's UAC on another host. A local nats-server without
//! auth in a temp dir, a temp HOME and a fake herdr outside any pane.

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{self, kv, stream};
use serde_json::json;

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Outside herdr: send mails as "the user". This host (ganymede) runs one
/// agent, w1:p1.
const FAKE_HERDR: &str = r#"#!/bin/sh
case "$1 $2" in
  "pane list") echo '{"result":{"panes":[]}}' ;;
  "agent list") echo '{"result":{"agents":[{"pane_id":"w1:p1","agent_status":"idle","agent":"claude"}]}}' ;;
  *) echo '{"result":{}}' ;;
esac
"#;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Runs `vega-post send <args>` on host ganymede; (exit ok, stdout).
fn send(home: &Path, url: &str, args: &[&str]) -> (bool, String) {
    post(home, url, "send", args)
}

fn post(home: &Path, url: &str, command: &str, args: &[&str]) -> (bool, String) {
    let bin = home.join("bin");
    let out = Command::new(env!("CARGO_BIN_EXE_vega-post"))
        .arg(command)
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("VEGA_NATS_URL", url)
        .env("VEGA_POST_HOST", "ganymede")
        .current_dir(home)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn send_finds_uac_of_plain_clone_by_origin() {
    check_send("~/src/spellkave/main/.git", Some("spellkave"));
}

#[test]
fn send_finds_uac_of_bare_layout_by_folder() {
    check_send("~/src/spellkave/.bare", None);
}

fn check_send(local_common: &str, origin_name: Option<&str>) {
    let project = "github.com/b4arena/spellkave";
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let herdr = bin.join("herdr");
    fs::write(&herdr, FAKE_HERDR).unwrap();
    fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();

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
    let _server = Server(
        Command::new("nats-server")
            .arg("-c")
            .arg(&conf)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("nats-server not on PATH: run the tests via `mise run check`"),
    );
    let url = format!("nats://127.0.0.1:{port}");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let js = rt.block_on(async {
        let mut client = None;
        for _ in 0..50 {
            if let Ok(c) = async_nats::connect(&url).await {
                client = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let js = jetstream::new(client.expect("local nats-server did not come up"));
        for host in ["proteus", "io", "titan", "ganymede"] {
            js.create_stream(stream::Config {
                name: format!("VEGA_INBOX_{host}"),
                subjects: vec![format!("vega.inbox.{host}.>")],
                ..Default::default()
            })
            .await
            .unwrap();
        }
        let kv = js
            .create_key_value(kv::Config {
                bucket: vega_registry::BUCKET.into(),
                history: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        let t = now();
        // proteus and io reconcile; titan's vegad stopped 10 minutes ago.
        // spellkave's old UAC w30:p1 on io is closed, its new one runs on
        // proteus.
        let entries = [
            (vega_registry::beat_key("proteus"), json!({"at": t})),
            (vega_registry::beat_key("io"), json!({"at": t})),
            (vega_registry::beat_key("titan"), json!({"at": t - 600})),
            (
                vega_registry::agent_key("proteus", "w3V:p1"),
                json!({"schema":2,"pane": "w3V:p1", "status": "idle", "project":project,"repo_key": local_common}),
            ),
            (
                vega_registry::uac_key("proteus", project),
                json!({"schema":2,"pane": "w3V:p1", "host": "proteus", "project": project, "origin_name": origin_name,"folder_name":vega_registry::project_folder(local_common)}),
            ),
            (
                vega_registry::uac_key("io", project),
                json!({"schema":2,"pane": "w30:p1", "host": "io", "project": project}),
            ),
        ];
        for (k, v) in entries {
            kv.put(k, v.to_string().into()).await.unwrap();
        }
        js
    });
    let count = |host: &str| {
        rt.block_on(async {
            js.get_stream(format!("VEGA_INBOX_{host}"))
                .await
                .unwrap()
                .info()
                .await
                .unwrap()
                .state
                .messages
        })
    };

    let (ok, out) = send(home, &url, &["--uac", ".", "outside git"]);
    assert!(!ok, "{out}");
    assert!(
        out.contains("no_uac") && out.contains("this directory is in no git project"),
        "{out}"
    );

    // A gone pane: refused, nothing stored, the hint names the new UAC.
    let (ok, out) = send(home, &url, &["--host", "io", "w30:p1", "hi"]);
    assert!(!ok, "{out}");
    assert!(out.contains("pane_gone"), "{out}");
    assert!(out.contains("w3V:p1 on proteus"), "{out}");
    assert!(out.contains("--uac"), "{out}");
    assert_eq!(count("io"), 0);

    // An open pane: sent, no warning.
    let (ok, out) = send(home, &url, &["--host", "proteus", "w3V:p1", "hi"]);
    assert!(ok, "{out}");
    assert!(out.contains("sent: w3V:p1 on proteus"), "{out}");
    assert!(!out.contains("warning"), "{out}");
    assert_eq!(count("proteus"), 1);

    // A stale host proves nothing: sent with a warning.
    let (ok, out) = send(home, &url, &["--host", "titan", "w9:p9", "hi"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("warning: the registry entry of host titan is stale"),
        "{out}"
    );
    assert_eq!(count("titan"), 1);

    // By role: the project's UAC on another host.
    let (ok, out) = send(home, &url, &["--uac", "spellkave", "hi"]);
    assert!(ok, "{out}");
    assert!(out.contains("sent: w3V:p1 on proteus"), "{out}");
    assert_eq!(count("proteus"), 2);

    let (ok, out) = send(home, &url, &["--uac", project, "full ID"]);
    assert!(ok, "{out}");
    // This host's checkout is at a different path but resolves to the same ID.
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(home)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(home)
        .args([
            "remote",
            "add",
            "origin",
            "git@github.com:b4arena/spellkave.git"
        ])
        .status()
        .unwrap()
        .success());
    let (ok, out) = send(home, &url, &["--uac", ".", "from local clone"]);
    assert!(ok, "{out}");
    assert!(out.contains("sent: w3V:p1 on proteus"), "{out}");

    let (ok, out) = send(home, &url, &["--uac", "nope", "hi"]);
    assert!(!ok, "{out}");
    assert!(out.contains("no_uac"), "{out}");

    // This host has no registry entry (its vegad in local mode): herdr is
    // the truth for its own panes, without a warning.
    let (ok, out) = send(home, &url, &["w1:p1", "hi"]);
    assert!(ok, "{out}");
    assert!(!out.contains("warning"), "{out}");
    let (ok, out) = send(home, &url, &["w2:p1", "hi"]);
    assert!(!ok, "{out}");
    assert!(out.contains("pane_gone"), "{out}");
    assert_eq!(count("ganymede"), 1);

    // Timing is checked by send, before publish. A capped fast thread stores
    // nothing; after work, the same thread resumes at hop 1.
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let fast = format!("work/8@{ms}");
    let (ok, out) = send(home, &url, &["w1:p1", "--re", &fast, "loop"]);
    assert!(!ok, "{out}");
    assert!(out.contains("hop_limit"), "{out}");
    assert!(out.contains("120 s"), "{out}");
    assert_eq!(count("ganymede"), 1);
    let slow = format!("work/8@{}", ms - 120_001);
    let (ok, out) = send(home, &url, &["w1:p1", "--re", &slow, "work completed"]);
    assert!(ok, "{out}");
    assert!(out.contains("thread: work/1"), "{out}");
    assert_eq!(count("ganymede"), 2);
    rt.block_on(async {
        let stream = js.get_stream("VEGA_INBOX_ganymede").await.unwrap();
        let message = stream.get_raw_message(2).await.unwrap();
        assert_eq!(message.headers.get("Vega-Thread").unwrap().as_str(), "work");
        assert_eq!(message.headers.get("Vega-Hop").unwrap().as_str(), "1");
    });

    // Exercise the real inbox renderer with broker metadata. Deliberately
    // wrong sender timing must not enter the reply token; showing mail again
    // must retain its publication time, including at the fast limit.
    fs::write(
        &herdr,
        format!(
            r#"#!/bin/sh
case "$1 $2" in
  "pane list") echo '{{"result":{{"panes":[{{"pane_id":"w1:pR","agent":"claude"}}]}}}}' ;;
  "pane process-info") echo '{{"result":{{"process_info":{{"shell_pid":{}}}}}}}' ;;
  *) echo '{{"result":{{}}}}' ;;
esac
"#,
            std::process::id()
        ),
    )
    .unwrap();
    for hop in [1, 8] {
        let (id, published) = rt.block_on(async {
            let mut headers = async_nats::HeaderMap::new();
            for (key, value) in [
                ("Vega-From-Pane", "w3V:p1"),
                ("Vega-From-Agent", "claude"),
                ("Vega-Thread", "render"),
                ("Vega-Sent-Ms", "1"),
            ] {
                headers.insert(key, value);
            }
            headers.insert("Vega-Hop", hop.to_string().as_str());
            let ack = js
                .publish_with_headers("vega.inbox.ganymede.w1:pR.proteus", headers, "work".into())
                .await
                .unwrap()
                .await
                .unwrap();
            let stream = js.get_stream("VEGA_INBOX_ganymede").await.unwrap();
            let message = stream.get_raw_message(ack.sequence).await.unwrap();
            (
                ack.sequence,
                message.time.unix_timestamp_nanos() / 1_000_000,
            )
        });
        let token = format!("render/{hop}@{published}");
        let id = id.to_string();
        for args in [vec![], vec!["--show", id.as_str()]] {
            let (ok, out) = post(home, &url, "inbox", &args);
            assert!(ok, "{out}");
            assert!(out.contains(&format!("hop {hop}")), "{out}");
            assert!(
                out.contains(&format!(
                    "reply: vega post send --host proteus w3V:p1 --re {token} "
                )),
                "{out}"
            );
            assert_eq!(out.contains("fast hop limit 8 reached"), hop == 8, "{out}");
            assert!(!out.contains("reply: none"), "{out}");
            if args.is_empty() {
                assert!(out.contains("acked: 1"), "{out}");
            }
        }
        if hop == 8 {
            let (ok, out) = send(
                home,
                &url,
                &["--host", "proteus", "w3V:p1", "--re", &token, "loop"],
            );
            assert!(!ok, "{out}");
            assert!(out.contains("hop_limit"), "{out}");
        }
    }
}
