//! A real nkey-restricted temporary broker and an owned Rust hc RPC fake.
//! The test executable doubles as hc.exe, requiring no shell/Python on Windows.
#[cfg(not(any(windows, feature = "client-only")))]
fn main() {}
#[cfg(any(windows, feature = "client-only"))]
fn main() {
    if std::env::args().nth(1).as_deref() == Some("connect") {
        native::fake_hc();
    } else {
        native::suite();
    }
}
#[cfg(any(windows, feature = "client-only"))]
mod native {
    use serde_json::{json, Value};
    use std::{
        env, fs,
        io::BufRead,
        net::{TcpListener, TcpStream},
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };
    struct Broker(Child);
    impl Drop for Broker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    pub fn fake_hc() {
        let line = std::io::stdin().lock().lines().next().unwrap().unwrap();
        let req: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(req["method"], "terminal.get");
        let pane = req["params"]["terminal"].as_str().unwrap();
        let cwd = env::var("FAKE_CWD").unwrap();
        let pid = env::var("FAKE_PID").unwrap().parse::<u32>().unwrap();
        let result = json!({"address":pane,"state":"live","name":pane,"pid":pid,
            "argv":[],"labels":{},"created_ms":0,"screen":"exact","cols":80,"rows":24,
            "cwd":cwd,"foreground":null,"turn":0,
            "status":{"app":"codex","state":"idle"},"parent":env::var("FAKE_PARENT").ok()});
        println!(
            "{}",
            json!({"jsonrpc":"2.0","id":req["id"],"result":result})
        );
    }
    struct Host {
        root: PathBuf,
        host: &'static str,
        bin: PathBuf,
        url: String,
    }
    impl Host {
        fn cmd(&self, args: &[&str], pane: &str) -> Command {
            let mut c = Command::new(env!("CARGO_BIN_EXE_vega-post"));
            c.args(args)
                .current_dir(self.root.join("checkout with spaces"))
                .env("HOME", &self.root)
                .env("USERPROFILE", &self.root)
                .env("APPDATA", self.root.join("roaming"))
                .env("LOCALAPPDATA", self.root.join("local"))
                .env(
                    "PATH",
                    env::join_paths(
                        std::iter::once(self.bin.clone())
                            .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
                    )
                    .unwrap(),
                )
                .env("HC_TERMINAL_ID", pane)
                .env("VEGA_POST_HOST", self.host)
                .env("FAKE_PID", std::process::id().to_string())
                .env("FAKE_CWD", self.root.join("checkout with spaces"))
                .env_remove("HERDR_PANE_ID")
                .env_remove("VEGA_POST_INBOX_ONLY")
                .env_remove("VEGA_NATS_URL")
                .env_remove("VEGA_NATS_NKEY")
                .env_remove("VEGA_POST_CMD")
                .env_remove("FAKE_PARENT");
            #[cfg(windows)]
            {
                c.env_remove("XDG_CONFIG_HOME")
                    .env_remove("XDG_STATE_HOME")
                    .env_remove("XDG_CACHE_HOME");
            }
            #[cfg(unix)]
            {
                c.env("XDG_CONFIG_HOME", self.root.join("roaming"))
                    .env("XDG_STATE_HOME", self.root.join("local"))
                    .env("XDG_CACHE_HOME", self.root.join("cache"));
            }
            c
        }
        fn run(&self, args: &[&str], pane: &str) -> String {
            output(self.cmd(args, pane), true)
        }
        fn seed(&self) -> String {
            fs::read_to_string(self.root.join("roaming/vega/nats.nk"))
                .unwrap()
                .trim()
                .into()
        }
        fn public(&self) -> String {
            nkeys::KeyPair::from_seed(&self.seed())
                .unwrap()
                .public_key()
        }
        fn make(root: &Path, host: &'static str, bin: &Path, url: &str) -> Self {
            let root = root.join(host);
            fs::create_dir_all(root.join("checkout with spaces")).unwrap();
            let h = Self {
                root,
                host,
                bin: bin.into(),
                url: url.into(),
            };
            let checkout = h.root.join("checkout with spaces");
            assert!(Command::new("git")
                .arg("init")
                .arg(&checkout)
                .output()
                .unwrap()
                .status
                .success());
            assert!(Command::new("git")
                .current_dir(&checkout)
                .args([
                    "remote",
                    "add",
                    "origin",
                    "https://example.invalid/team/native.git"
                ])
                .output()
                .unwrap()
                .status
                .success());
            let first = h.run(&["keygen"], "t1");
            let seed = h.seed();
            assert!(first.contains(&h.public()));
            assert!(!first.contains(&seed));
            let second = h.run(&["keygen"], "t1");
            assert!(second.contains(&h.public()));
            assert_eq!(h.seed(), seed);
            fs::write(h.root.join("roaming/vega/nats.url"), format!("{}\n", h.url)).unwrap();
            h
        }
    }
    fn output(mut c: Command, success: bool) -> String {
        let o = c.output().unwrap();
        let text = String::from_utf8(o.stdout).unwrap();
        assert_eq!(
            o.status.success(),
            success,
            "status {:?}: {text}\n{}",
            o.status,
            String::from_utf8_lossy(&o.stderr)
        );
        text
    }
    fn permissions(h: &str, key: &str) -> String {
        format!(
            r#"{{ nkey: "{key}", permissions: {{
          publish: ["vega.inbox.*.*.{h}", "$KV.VEGA_REGISTRY.{h}.>",
            "$JS.API.STREAM.INFO.KV_VEGA_REGISTRY", "$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.>",
            "$JS.API.STREAM.INFO.VEGA_INBOX_{h}", "$JS.API.CONSUMER.*.VEGA_INBOX_{h}.>",
            "$JS.API.CONSUMER.MSG.NEXT.VEGA_INBOX_{h}.*", "$JS.ACK.VEGA_INBOX_{h}.>"],
          subscribe: ["_INBOX_{h}.>", "vega.inbox.{h}.>"]
        }} }}"#
        )
    }
    pub fn suite() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bin = root.join("fake bin");
        fs::create_dir_all(&bin).unwrap();
        fs::copy(
            env::current_exe().unwrap(),
            bin.join(format!("hc{}", env::consts::EXE_SUFFIX)),
        )
        .unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let url = format!("nats://127.0.0.1:{port}");
        let win = Host::make(&root, "native-test", &bin, &url);
        let peer = Host::make(&root, "peer-test", &bin, &url);
        let admin = nkeys::KeyPair::new_user();
        let conf = root.join("nats.conf");
        fs::write(&conf,format!("listen: 127.0.0.1:{port}\njetstream {{ store_dir: {} }}\nauthorization {{ users: [{{ nkey: \"{}\" }}, {}, {}] }}\n",json!(root.join("js").to_string_lossy()),admin.public_key(),permissions(win.host,&win.public()),permissions(peer.host,&peer.public()))).unwrap();
        let _broker = Broker(
            Command::new("nats-server")
                .arg("-c")
                .arg(&conf)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("CI must install the pinned nats-server"),
        );
        let until = Instant::now() + Duration::from_secs(15);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < until, "temporary broker failed to start");
            std::thread::sleep(Duration::from_millis(25));
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = rt
            .block_on(async_nats::ConnectOptions::with_nkey(admin.seed().unwrap()).connect(&url))
            .unwrap();
        rt.block_on(async {
            let js = vega_registry::broker_context(client.clone());
            js.create_key_value(async_nats::jetstream::kv::Config {
                bucket: vega_registry::BUCKET.into(),
                ..Default::default()
            })
            .await
            .unwrap();
            for h in [&win, &peer] {
                js.create_stream(async_nats::jetstream::stream::Config {
                    name: format!("VEGA_INBOX_{}", h.host),
                    subjects: vec![format!("vega.inbox.{}.>", h.host)],
                    retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
                    ..Default::default()
                })
                .await
                .unwrap();
            }
        });
        for h in [&win, &peer] {
            assert!(h
                .run(&["identity", "enroll", "--checkout", "."], "t1")
                .contains("v2 enrolled and verified"));
            assert!(h
                .run(&["identity", "verify"], "t1")
                .contains(&format!("{}.identity.v2", h.host)));
            assert!(h
                .run(&["identity", "enroll", "--checkout", "."], "t1")
                .contains("verified"));
            assert!(!h.root.join("local/vega/identity-cutover").exists());
        }
        assert!(win.run(&["status"], "t1").contains("unread: 0"));
        let sent = peer.run(&["send", "--host", win.host, "t1", "hello from peer"], "t2");
        assert!(sent.contains("sent:"));
        assert!(win.run(&["status"], "t1").contains("unread: 1"));
        let archive = win.run(&["log", "--json"], "t1");
        assert!(archive.contains("hello from peer"));
        assert!(win.run(&["status"], "t1").contains("unread: 1"));
        let kept = win.run(&["inbox", "--keep"], "t1");
        assert!(kept.contains("kept: 1"));
        assert!(win.run(&["status"], "t1").contains("unread: 1"));
        let shown = win.run(&["inbox", "--show", "1"], "t1");
        assert!(shown.contains("hello from peer"));
        let token = shown
            .split(" --re ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap();
        assert!(token.contains('@'));
        assert!(win
            .run(
                &[
                    "send",
                    "--host",
                    peer.host,
                    "t2",
                    "--re",
                    token,
                    "reply from Windows"
                ],
                "t1"
            )
            .contains("sent:"));
        assert!(peer.run(&["inbox"], "t2").contains("reply from Windows"));
        let mut parent = peer.cmd(&["inbox", "--show", "1"], "t2");
        parent.env("FAKE_PARENT", format!("{}/t1", win.host));
        assert!(output(parent, true).contains("from_parent: yes"));
        assert!(win
            .run(
                &[
                    "done",
                    "--to",
                    "t2",
                    "--host",
                    peer.host,
                    "--outcome",
                    "succeeded",
                    "native result"
                ],
                "t1"
            )
            .contains("done: succeeded"));
        assert!(peer
            .run(&["inbox"], "t2")
            .contains("done (succeeded): native result"));
        // --show did not acknowledge the kept message. Ack using the broker only
        // to avoid waiting out the deliberately preserved 30s redelivery lease.
        rt.block_on(async {
            let js = vega_registry::broker_context(client.clone());
            let stream = js
                .get_stream(format!("VEGA_INBOX_{}", win.host))
                .await
                .unwrap();
            let consumer: async_nats::jetstream::consumer::Consumer<
                async_nats::jetstream::consumer::pull::Config,
            > = stream
                .get_consumer(&format!("inbox-{}-t1", win.host))
                .await
                .unwrap();
            assert_eq!(consumer.cached_info().num_ack_pending, 1);
        });
        // Fresh non-kept mail is acknowledged and remains in the archive.
        peer.run(&["send", "--host", win.host, "t3", "ack this"], "t2");
        assert!(win.run(&["inbox"], "t3").contains("acked: 1"));
        assert!(win.run(&["inbox"], "t3").contains("0 messages"));
        assert!(win
            .run(&["log", "--pane", "t3", "--json"], "t3")
            .contains("ack this"));
        let mut spoof = win.cmd(&["inbox"], "t1");
        spoof.env("FAKE_PID", "4294967294");
        assert!(output(spoof, false).contains("not_in_terminal"));
        assert!(output(
            win.cmd(
                &["send", "--host", peer.host, "t2", "--re", "thread/8", "loop"],
                "t1"
            ),
            false
        )
        .contains("hop_limit"));
        assert!(output(win.cmd(&["relay", "start"], "t1"), false).contains("usage"));
        assert!(output(win.cmd(&["keygen", "--admin"], "t1"), false).contains("usage"));
        assert!(output(
            win.cmd(&["send", "--host", peer.host, "t2", "--typo", "text"], "t1"),
            false
        )
        .contains("usage"));
        // A copied state record cannot enroll under a different host identity.
        let mut mismatch = win.cmd(&["identity", "verify"], "t1");
        mismatch.env("VEGA_POST_HOST", "wrong-host");
        assert!(output(mismatch, false).contains("identity_enrollment"));
        // Durable parent records drive done without an explicit --to.
        let parents = win.root.join("local/vega/parents");
        fs::create_dir_all(&parents).unwrap();
        let token = vega_herdr::pane::hc_storage_token("t1").unwrap();
        fs::write(
            parents.join(format!("local_{token}")),
            json!({"name":"t1","parent":"t2","host":peer.host}).to_string(),
        )
        .unwrap();
        assert!(win
            .run(
                &["done", "--outcome", "succeeded", "recorded parent result"],
                "t1"
            )
            .contains("done: succeeded to t2"));
        assert!(peer
            .run(&["inbox"], "t2")
            .contains("recorded parent result"));
        // Enrollment is stable across normal registry reconciliation; it must
        // neither silently accept live legacy state nor reject tombstones.
        rt.block_on(async {
            let kv = vega_registry::bucket(&client).await.unwrap();
            let legacy = format!("{}.uac.old", win.host);
            kv.put(legacy.clone(), json!({"project":"old"}).to_string().into())
                .await
                .unwrap();
        });
        assert!(output(win.cmd(&["identity", "verify"], "t1"), false)
            .contains("legacy owned registry key"));
        rt.block_on(async {
            let kv = vega_registry::bucket(&client).await.unwrap();
            kv.delete(format!("{}.uac.old", win.host)).await.unwrap();
            vega_registry::reconcile(
                &kv,
                win.host,
                Default::default(),
                &Default::default(),
                &[vega_registry::identity_key(win.host)],
            )
            .await
            .unwrap();
        });
        assert!(win.run(&["identity", "verify"], "t1").contains("verified"));
        // A foreign host name cannot turn this seed into that host's reader.
        let mut foreign = win.cmd(&["status"], "t1");
        foreign.env("VEGA_POST_HOST", peer.host);
        assert!(output(foreign, false).contains("permission_denied"));
        let link = win.root.join("seed link");
        #[cfg(windows)]
        assert!(Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(win.root.join("roaming/vega"))
            .output()
            .unwrap()
            .status
            .success());
        #[cfg(unix)]
        std::os::unix::fs::symlink(win.root.join("roaming/vega"), &link).unwrap();
        let mut linked = win.cmd(&["keygen"], "t1");
        linked.env("VEGA_NATS_NKEY", link.join("nats.nk"));
        assert!(output(linked, false).contains("private_file"));
        // Ordinary credentials cannot create missing inboxes.
        rt.block_on(async {
            vega_registry::broker_context(client)
                .delete_stream(format!("VEGA_INBOX_{}", peer.host))
                .await
        })
        .unwrap();
        assert!(output(peer.cmd(&["status"], "t2"), false).contains("no_inbox_stream"));
        println!("native client suite: passed (restricted nkeys, enrollment, status/send/reply/done, inbox/archive/acks, ancestry, errors)");
    }
}
