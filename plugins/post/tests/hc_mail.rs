//! Actual CLI + relay + local broker, with both terminal backends faked.
//! Sending successfully is insufficient: every direction must type a pointer
//! and the recipient must read the original body from its own inbox.
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Proc(Child);
impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const HERDR: &str = r#"#!/bin/sh
case "$1 $2" in
 'api schema') exit 1 ;;
 'agent list'|'pane list')
   key=agents; test "$1" = pane && key=panes
   printf '{"result":{"%s":[{"pane_id":"w1:p1","agent":"codex","agent_status":"idle","cwd":"%s"},{"pane_id":"w1:p2","name":"shared-worker","agent":"codex","agent_status":"idle","cwd":"%s"}]}}\n' "$key" "$HOME" "$HOME" ;;
 'workspace list') echo '{"result":{"workspaces":[]}}' ;;
 'pane process-info') printf '{"result":{"process_info":{"shell_pid":%s}}}\n' "$FAKE_PID" ;;
 'agent get')
   case "$3" in
     shared-worker) set -- agent get w1:p2 ;;
     w1:p1|w1:p2) ;;
     *) echo '{"error":{"code":"not_found","message":"no such agent"}}'; exit 1 ;;
   esac
   printf '{"result":{"agent":{"pane_id":"%s","agent":"codex"}}}\n' "$3" ;;
 'agent prompt') printf '{"pane":"%s","text":"pointer"}\n' "$3" >> "$HOME/prompts"; echo '{"result":{}}' ;;
 'pane read') printf 'done\n────────\n❯ \n────────\n status\n' ;;
 *) echo '{"result":{}}' ;;
esac
"#;

fn write(path: &Path, v: &Value) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, v.to_string()).unwrap();
    fs::rename(tmp, path).unwrap();
}
fn wait(home: &Path, what: &str, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(35);
    while !ready() {
        assert!(
            Instant::now() < until,
            "waiting for {what}: {}",
            fs::read_to_string(home.join("relay.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
struct Fixture {
    home: tempfile::TempDir,
    url: String,
    exe: PathBuf,
    _nats: Proc,
    relay: Option<Proc>,
}
impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let bin = h.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("herdr"), HERDR).unwrap();
        let py = Command::new("python3")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .unwrap();
        let py = String::from_utf8(py.stdout).unwrap();
        fs::write(
            bin.join("hc"),
            format!("#!{}\n{}", py.trim(), include_str!("fixtures/hc-mail.py")),
        )
        .unwrap();
        for name in ["herdr", "hc"] {
            fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let exe = bin.join("vega-post");
        fs::copy(env!("CARGO_BIN_EXE_vega-post"), &exe).unwrap();
        let terminals:Vec<_>=["t1","t2","io/t9"].iter().map(|p| json!({"address":p,"state":"live","name":p,"cwd":h,"turn":0,"status":{"app":"claude-code","state":"idle"}})).collect();
        write(&h.join("hc.json"), &json!({"terminals":terminals}));
        let cutover = h.join("state/vega/identity-cutover");
        fs::create_dir_all(&cutover).unwrap();
        let plan = json!({"schema":2,"host":"testhost","inbox_only":false,"mappings":[],"original":{},"writes":{},"remove":[],"registry":{"host":"testhost","backup":{},"projection":{}},"registry_reads":[],"owned":{}});
        write(&cutover.join("journal.json"), &plan);
        write(
            &cutover.join("approved.json"),
            &json!({"schema":2,"hosts":[plan],"grants":{}}),
        );
        write(
            &cutover.join("complete.json"),
            &json!({"schema":2,"host":"testhost"}),
        );
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let conf = h.join("nats.conf");
        fs::write(
            &conf,
            format!(
                "listen: 127.0.0.1:{port}\njetstream {{ store_dir: \"{}\" }}\n",
                h.join("js").display()
            ),
        )
        .unwrap();
        let nats = Proc(
            Command::new("nats-server")
                .arg("-c")
                .arg(conf)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let url = format!("nats://127.0.0.1:{port}");
        runtime().block_on(async {
            let until = Instant::now() + Duration::from_secs(10);
            let c = loop {
                if let Ok(c) = async_nats::connect(&url).await {
                    break c;
                }
                assert!(Instant::now() < until);
                tokio::time::sleep(Duration::from_millis(50)).await;
            };
            async_nats::jetstream::new(c)
                .create_key_value(async_nats::jetstream::kv::Config {
                    bucket: vega_registry::BUCKET.into(),
                    ..Default::default()
                })
                .await
                .unwrap();
        });
        Self {
            home,
            url,
            exe,
            _nats: nats,
            relay: None,
        }
    }
    fn command(&self, args: &[&str], pane: Option<&str>) -> Command {
        let h = self.home.path();
        let mut c = Command::new(&self.exe);
        c.args(args)
            .env_clear()
            .env("HOME", h)
            .env("XDG_STATE_HOME", h.join("state"))
            .env("XDG_CONFIG_HOME", h.join("config"))
            .env("PATH", format!("{}:/usr/bin:/bin", h.join("bin").display()))
            .env("FAKE_PID", std::process::id().to_string())
            .env("VEGA_POST_HOST", "testhost")
            .env("VEGA_NATS_URL", &self.url)
            .env("VEGA_POST_TEST_SPEEDUP", "20")
            .current_dir(h);
        if let Some(p) = pane {
            c.env(
                if p.starts_with('t') {
                    "HC_TERMINAL_ID"
                } else {
                    "HERDR_PANE_ID"
                },
                p,
            );
        }
        c
    }
    fn post(&self, args: &[&str], pane: &str) -> String {
        let o = self.command(args, Some(pane)).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(o.status.success(), "{args:?}: {text}");
        text
    }
    fn start(&mut self) {
        let mut c = self.command(&["relay"], None);
        c.stdout(fs::File::create(self.home.path().join("relay.log")).unwrap())
            .stderr(Stdio::null());
        self.relay = Some(Proc(c.spawn().unwrap()));
        wait(self.home.path(), "watchers", || {
            self.log().contains("stream (hc events.subscribe)")
                && self.log().contains("relay: testhost watching")
        });
    }
    fn log(&self) -> String {
        fs::read_to_string(self.home.path().join("relay.log")).unwrap_or_default()
    }
    fn prompts(&self, pane: &str) -> Vec<Value> {
        fs::read_to_string(self.home.path().join("prompts"))
            .unwrap_or_default()
            .lines()
            .filter_map(|s| serde_json::from_str::<Value>(s).ok())
            .filter(|v| v["pane"] == pane)
            .collect()
    }
    fn status(&self, pane: &str, status: &str, seq: u64) {
        let file = self.home.path().join("hc.json");
        let mut v: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        for t in v["terminals"].as_array_mut().unwrap() {
            if t["address"] == pane {
                t["status"]["state"] = json!(status);
            }
        }
        write(&file, &v);
        self.event(json!({"method":"event","params":{"seq":seq,"terminal":pane,"type":"status","data":{"status":{"state":status}}}}));
    }
    fn event(&self, v: Value) {
        writeln!(
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.home.path().join("events"))
                .unwrap(),
            "{v}"
        )
        .unwrap();
    }
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn mixed_mail_is_delivered_and_read_in_all_four_directions() {
    let mut f = Fixture::new();
    f.start();
    for (sender, recipient, target) in [
        ("w1:p1", "w1:p2", "w1:p2"),
        ("w1:p1", "t2", "hc:t2"),
        ("t1", "w1:p2", "herdr:w1:p2"),
        ("t1", "t2", "t2"),
    ] {
        let body = format!("mail-{sender}-{recipient}");
        let before = f.prompts(recipient).len();
        f.post(&["send", target, &body], sender);
        wait(f.home.path(), &body, || f.prompts(recipient).len() > before);
        let inbox = f.post(&["inbox"], recipient);
        assert!(inbox.contains(&body), "{inbox}");
    }
    let entries = runtime().block_on(async {
        let c = async_nats::connect(&f.url).await.unwrap();
        let kv = vega_registry::bucket(&c).await.unwrap();
        vega_registry::read(&c, &kv).await.unwrap()
    });
    let a = entries
        .entries
        .get(&vega_registry::agent_key("testhost", "hc:t2"))
        .unwrap();
    assert_eq!(a["backend"], "hc");
    assert_eq!(a["address"], "hc:t2");
    assert_eq!(a["kind"], "claude");
    assert!(!entries
        .entries
        .contains_key(&vega_registry::agent_key("testhost", "io/t9")));
    assert!(
        !f.log().contains("pane gone:"),
        "one backend's snapshot removed the other: {}",
        f.log()
    );

    // Done uses plain send; a fresh blocked status refuses even urgent mail.
    f.status("t2", "done", 10);
    let n = f.prompts("t2").len();
    f.post(&["send", "t2", "done-recipient"], "t1");
    wait(f.home.path(), "done pointer", || f.prompts("t2").len() > n);
    assert!(!f.prompts("t2").last().unwrap()["args"]
        .as_array()
        .unwrap()
        .contains(&json!("--if")));
    assert!(f.post(&["inbox"], "t2").contains("done-recipient"));
    f.status("t2", "blocked", 11);
    let n = f.prompts("t2").len();
    f.post(&["send", "--now", "t2", "blocked-recipient"], "t1");
    wait(f.home.path(), "blocked refusal", || {
        f.log().contains("t2 is blocked")
    });
    assert_eq!(f.prompts("t2").len(), n);
    f.status("t2", "working", 12);
    wait(f.home.path(), "urgent queued pointer", || {
        f.prompts("t2").len() > n
    });
    assert!(f.post(&["inbox"], "t2").contains("blocked-recipient"));

    // Missed events: restart subscription and recover from a fresh snapshot.
    f.status("t2", "idle", 13);
    f.event(json!({"method":"missed","params":{}}));
    let n = f.prompts("t2").len();
    f.post(&["send", "hc:t2", "after-missed"], "t1");
    wait(f.home.path(), "resnapshot delivery", || {
        f.prompts("t2").len() > n
    });
    assert!(f.post(&["inbox"], "t2").contains("after-missed"));
}

#[test]
fn polling_fallback_delivers_without_an_event_stream() {
    let mut f = Fixture::new();
    let parents = f.home.path().join("state/vega/parents");
    fs::create_dir_all(&parents).unwrap();
    let adopted = json!({"name":"t2","parent":"w1:p1","host":"testhost"});
    write(&parents.join("local_hc-7432"), &adopted);
    let file = f.home.path().join("hc.json");
    let mut terminals: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    terminals["terminals"][1]["parent"] = json!("t1");
    write(&file, &terminals);
    fs::write(f.home.path().join("poll-mode"), "").unwrap();
    let mut c = f.command(&["relay"], None);
    c.stdout(fs::File::create(f.home.path().join("relay.log")).unwrap())
        .stderr(Stdio::null());
    f.relay = Some(Proc(c.spawn().unwrap()));
    wait(f.home.path(), "poll mode", || {
        f.log().contains("hc mode: poll") && f.log().contains("relay: testhost watching")
    });
    f.post(&["send", "hc:t2", "poll-mail"], "w1:p1");
    wait(f.home.path(), "poll delivery", || {
        !f.prompts("t2").is_empty()
    });
    assert!(f.post(&["inbox"], "t2").contains("poll-mail"));
    let saved: Value =
        serde_json::from_slice(&fs::read(parents.join("local_hc-7432")).unwrap()).unwrap();
    assert_eq!(
        saved, adopted,
        "native parent must not overwrite vega adoption"
    );
}

#[test]
fn hc_parent_queue_done_silence_and_closure_use_the_existing_ledger() {
    let mut f = Fixture::new();
    let state = f.home.path().join("state/vega");
    let file = f.home.path().join("hc.json");
    let mut terminals: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    terminals["terminals"][1]["parent"] = json!("t1");
    write(&file, &terminals);
    f.start();
    assert!(state.join("parents/local_hc-7432").exists());
    assert!(
        !state.join("briefs/local_hc-7432").exists(),
        "discovery is not a new brief"
    );
    f.status("t2", "working", 1);
    f.post(&["send", "t2", "ordinary-waits"], "w1:p1");
    wait(f.home.path(), "ordinary mail seen", || {
        f.log().contains("seen: w1:p1 -> t2")
    });
    assert!(f.prompts("t2").is_empty());
    f.post(&["send", "hc:t2", "parent-queued"], "t1");
    wait(f.home.path(), "parent queue", || {
        !f.prompts("t2").is_empty()
    });
    assert!(!f.prompts("t2").last().unwrap()["args"]
        .as_array()
        .unwrap()
        .contains(&json!("--if")));
    let inbox = f.post(&["inbox"], "t2");
    assert!(inbox.contains("ordinary-waits") && inbox.contains("parent-queued"));
    f.post(
        &[
            "done",
            "--to",
            "t1",
            "--outcome",
            "succeeded",
            "finished-task",
        ],
        "t2",
    );
    wait(f.home.path(), "done ledger", || {
        state.join("done/local_hc-7432").exists()
    });
    assert!(fs::read_to_string(state.join("done/local_hc-7432"))
        .unwrap()
        .contains("finished-task"));
    f.status("t2", "done", 2);
    wait(f.home.path(), "parent pointer", || {
        !f.prompts("t1").is_empty()
    });
    assert!(f.post(&["inbox"], "t1").contains("finished-task"));

    // A new brief settles the previous report; an unreported turn is silent.
    f.post(&["send", "t2", "new-task"], "t1");
    wait(f.home.path(), "settled report", || {
        !state.join("done/local_hc-7432").exists()
    });
    f.status("t2", "working", 3);
    f.status("t2", "idle", 4);
    wait(f.home.path(), "silent ledger", || {
        state.join("silent/local_hc-7432").exists()
    });
    f.post(&["inbox"], "t2");

    // Busy child closes before reading ordinary mail: bounce goes to its sender.
    f.status("t2", "working", 5);
    f.post(&["send", "t2", "closed-unread"], "w1:p1");
    wait(f.home.path(), "unread mail seen", || {
        f.log().matches("seen: w1:p1 -> t2").count() >= 2
    });
    let file = f.home.path().join("hc.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    v["terminals"]
        .as_array_mut()
        .unwrap()
        .retain(|t| t["address"] != "t2");
    write(&file, &v);
    f.event(
        json!({"method":"event","params":{"seq":6,"terminal":"t2","type":"finished","data":{}}}),
    );
    wait(f.home.path(), "bounce", || {
        f.log().contains("bounce: t2 unread, mailed w1:p1")
    });
    assert!(f
        .post(&["inbox"], "w1:p1")
        .contains("t2 on testhost closed before it read your mail"));
}

#[test]
fn hc_status_race_keeps_pointer_pending_and_local_delivery_uses_hc() {
    let mut f = Fixture::new();
    f.start();
    fs::write(f.home.path().join("race"), "blocked").unwrap();
    f.post(&["send", "t2", "racing-status"], "w1:p1");
    wait(f.home.path(), "atomic refusal", || {
        f.log().contains("pointer failed: t2: agent_blocked")
    });
    assert!(f.prompts("t2").is_empty());
    fs::remove_file(f.home.path().join("race")).unwrap();
    wait(f.home.path(), "pending retry", || {
        !f.prompts("t2").is_empty()
    });
    assert!(f.post(&["inbox"], "t2").contains("racing-status"));
    // Inbox-only direct path also uses the target backend, without a broker.
    let before = f.prompts("t2").len();
    let out = f
        .command(&["send", "hc:t2", "local-hc-body"], Some("w1:p1"))
        .env("VEGA_POST_INBOX_ONLY", "testhost")
        .env("VEGA_NATS_URL", "nats://127.0.0.1:9")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(f.prompts("t2").len(), before + 1);
    assert!(f.prompts("t2").last().unwrap()["text"]
        .as_str()
        .unwrap()
        .contains("local-hc-body"));
}

#[test]
fn inbox_only_ambiguous_backend_name_requires_an_explicit_terminal_id() {
    let f = Fixture::new();
    let file = f.home.path().join("hc.json");
    let mut terminals: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    terminals["terminals"][1]["name"] = json!("shared-worker");
    write(&file, &terminals);
    let send = |target: &str, body: &str| {
        f.command(&["send", target, body], Some("w1:p1"))
            .env("VEGA_POST_INBOX_ONLY", "testhost")
            .env("VEGA_NATS_URL", "nats://127.0.0.1:9")
            .output()
            .unwrap()
    };
    let out = send("shared-worker", "ambiguous-body");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("ambiguous_agent"), "{text}");
    assert!(text.contains("use an explicit terminal ID"), "{text}");
    assert!(!f.home.path().join("prompts").exists());

    for target in ["t2", "hc:t2"] {
        let body = format!("explicit-{target}");
        let before = f.prompts("t2").len();
        let out = send(target, &body);
        assert!(out.status.success(), "{out:?}");
        let prompts = f.prompts("t2");
        assert_eq!(prompts.len(), before + 1);
        assert!(prompts.last().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains(&body));
    }
    assert!(f.prompts("w1:p2").is_empty());
}

#[test]
fn theme_change_reaches_idle_hc_claude_through_guarded_send() {
    let mut f = Fixture::new();
    let home = f.home.path();
    fs::create_dir_all(home.join("config/vega")).unwrap();
    fs::write(
        home.join("config/vega/theme.toml"),
        "mode = \"light\"\nlive_claude = true\n",
    )
    .unwrap();
    // A recorded different mode makes the watcher emit a real theme event.
    write(
        &home.join("state/vega/theme.json"),
        &json!({"mode":"dark","source":"fixed","at":1}),
    );
    fs::write(
        home.join("hc-screen"),
        "answer\n────────\n❯ \n────────\n  status\n",
    )
    .unwrap();
    f.start();
    wait(f.home.path(), "hc theme command", || {
        f.prompts("t2")
            .iter()
            .any(|p| p["text"] == "/config theme=light")
    });
    let prompts = f.prompts("t2");
    let theme = prompts
        .iter()
        .find(|p| p["text"] == "/config theme=light")
        .unwrap();
    assert_eq!(
        theme["args"],
        json!(["send", "--if", "idle", "t2", "/config theme=light"])
    );
    assert!(f
        .log()
        .contains("claude pane(s) get /config theme=light when idle"));
    assert!(f.prompts("w1:p2").is_empty());
}

#[test]
fn inbox_only_hc_name_resolves_without_a_herdr_match() {
    let f = Fixture::new();
    let file = f.home.path().join("hc.json");
    let mut terminals: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    terminals["terminals"][1]["name"] = json!("hc-worker");
    write(&file, &terminals);
    let out = f
        .command(&["send", "hc-worker", "hc-name-body"], Some("w1:p1"))
        .env("VEGA_POST_INBOX_ONLY", "testhost")
        .env("VEGA_NATS_URL", "nats://127.0.0.1:9")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let prompts = f.prompts("t2");
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0]["text"]
        .as_str()
        .unwrap()
        .contains("hc-name-body"));
    let args = prompts[0]["args"].as_array().unwrap();
    assert_eq!(args[0], "send");
    assert_eq!(args[args.len() - 2], "t2");
    assert!(f.prompts("w1:p2").is_empty());
}
