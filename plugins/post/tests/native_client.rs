//! A real nkey-restricted temporary broker and an owned Rust hc RPC fake.
//! The test executable doubles as hc.exe, requiring no shell/Python on Windows.
#[cfg(not(any(windows, feature = "client-only")))]
fn main() {}
#[cfg(any(windows, feature = "client-only"))]
fn main() {
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("--job-start") {
        native::job_start();
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("server") {
        assert_eq!(std::env::args().nth(2).as_deref(), Some("status"));
        println!("{{\"pid\":{}}}", std::env::var("FAKE_PID").unwrap());
    } else if std::env::args().nth(1).as_deref() == Some("connect") {
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
    #[cfg(windows)]
    pub fn job_start() {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::{JobObjects::*, Threading::GetCurrentProcess},
        };
        let args: Vec<_> = env::args().collect();
        // The helper owns this job, like hc. Closing it after the starter
        // returns must reap inherited workers, exposing false detachment.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            assert!(!job.is_null());
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if args[2] == "terminal" {
                limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            }
            assert_ne!(
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    std::mem::size_of_val(&limits) as u32
                ),
                0
            );
            assert_ne!(AssignProcessToJobObject(job, GetCurrentProcess()), 0);
            let result = Command::new(&args[3])
                .args(["relay", "start"])
                .output()
                .unwrap();
            print!("{}", String::from_utf8_lossy(&result.stdout));
            eprint!("{}", String::from_utf8_lossy(&result.stderr));
            assert!(result.status.success());
            // Closing the last handle kills this helper too; flush the report first.
            use std::io::Write;
            std::io::stdout().flush().unwrap();
            CloseHandle(job);
        }
    }
    impl Drop for Broker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    pub fn fake_hc() {
        for line in std::io::stdin().lock().lines() {
            let req: Value = serde_json::from_str(&line.unwrap()).unwrap();
            fake_hc_request(req);
        }
    }

    fn fake_hc_request(req: Value) {
        let pane =
            req["params"]["terminal"]
                .as_str()
                .unwrap_or(if env::var_os("FAKE_STATE").is_some() {
                    "t4"
                } else {
                    "t1"
                });
        let state: Value = env::var("FAKE_STATE")
            .ok()
            .and_then(|p| fs::read(p).ok())
            .and_then(|s| serde_json::from_slice(&s).ok())
            .unwrap_or(json!({"status":"idle","screen":"❯ "}));
        let cwd = env::var("FAKE_CWD").unwrap();
        let pid = env::var("FAKE_PID").unwrap().parse::<u32>().unwrap();
        if req["method"] == "terminal.list" && state["status"] == "outage" {
            println!(
                "{}",
                json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32000,"message":"owned fake offline","data":{"kind":"unavailable"}}})
            );
            return;
        }
        let terminal = json!({"address":pane,"state":"live","name":pane,"pid":pid,
            "argv":[],"labels":state.get("labels").cloned().unwrap_or_else(|| json!({})),"created_ms":0,"screen":"exact","cols":80,"rows":24,
            "cwd":cwd,"foreground":null,"turn":0,
            "status":{"app":"codex","state":state["status"]},"parent":env::var("FAKE_PARENT").ok()});
        let result = match req["method"].as_str().unwrap() {
            "terminal.get" => terminal,
            "terminal.list" => {
                let mut remote = terminal.clone();
                remote["address"] = "other/t9".into();
                let mut rows = vec![terminal.clone(), remote];
                for (id, role, parent) in [
                    ("t5", "sentinel", None),
                    ("t6", "gate-reviewer", None),
                    ("t7", "", Some("t5")),
                ] {
                    if env::var_os("FAKE_LABEL_WRITES").is_some() {
                        let mut row = terminal.clone();
                        row["address"] = id.into();
                        row["labels"]["vega.role"] = role.into();
                        row["parent"] = json!(parent);
                        rows.push(row);
                    }
                }
                json!({"terminals":rows,"machines":[],"hidden_finished":0})
            }
            "doc.get" => json!({"version":0,"value":null}),
            "terminal.label" => {
                if let Some(path) = env::var_os("FAKE_LABEL_WRITES") {
                    use std::io::Write;
                    let mut f = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .unwrap();
                    writeln!(f, "{}", req["params"]).unwrap();
                }
                terminal
            }
            "terminal.read" => json!({"text":state["screen"],"complete":true,"how":"screen"}),
            "terminal.send" => {
                assert_ne!(state["status"], "blocked");
                use std::io::Write;
                let mut f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(env::var("FAKE_SENDS").unwrap())
                    .unwrap();
                writeln!(f, "{}", req["params"]).unwrap();
                json!({"turn":0,"status":state["status"],"started":true})
            }
            other => panic!("unexpected fake method {other}"),
        };
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
        let label = c
            .get_args()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        use std::io::{Read, Seek};
        let mut stdout = tempfile::tempfile().unwrap();
        let mut stderr = tempfile::tempfile().unwrap();
        let mut child = c
            .stdout(stdout.try_clone().unwrap())
            .stderr(stderr.try_clone().unwrap())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(40);
        let mut timed_out = false;
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= until {
                timed_out = true;
                child.kill().unwrap();
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let status = child.wait().unwrap();
        stdout.rewind().unwrap();
        stderr.rewind().unwrap();
        let mut text = String::new();
        let mut diagnostics = String::new();
        stdout.read_to_string(&mut text).unwrap();
        stderr.read_to_string(&mut diagnostics).unwrap();
        assert!(
            !timed_out,
            "native command timed out ({label}): {text}\n{}",
            diagnostics
        );
        assert_eq!(
            status.success(),
            success,
            "status {:?}: {text}\n{}",
            status,
            diagnostics
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
    struct OwnedWorker(String);
    impl Drop for OwnedWorker {
        fn drop(&mut self) {
            #[cfg(windows)]
            {
                use windows_sys::Win32::{
                    Foundation::CloseHandle,
                    System::Threading::{
                        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
                        PROCESS_TERMINATE,
                    },
                };
                // SAFETY: this PID came from the owned starter, and the handle is closed.
                unsafe {
                    let h = OpenProcess(
                        PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                        0,
                        self.0.parse().unwrap(),
                    );
                    if !h.is_null() {
                        TerminateProcess(h, 1);
                        WaitForSingleObject(h, 5000);
                        CloseHandle(h);
                    }
                }
            }
            #[cfg(unix)]
            let _ = Command::new("kill").args(["-TERM", &self.0]).status();
        }
    }
    #[track_caller]
    fn wait_for(mut check: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(20);
        while !check() {
            assert!(Instant::now() < until, "relay condition timed out");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    fn relay_suite(
        win: &Host,
        peer: &Host,
        rt: &tokio::runtime::Runtime,
        client: &async_nats::Client,
        broker: &mut Broker,
        conf: &Path,
    ) {
        eprintln!("native relay: guarded delivery and ack ownership");
        let state = win.root.join("hc-state.json");
        let sends = win.root.join("sends.jsonl");
        let set = |status: &str, screen: &str| {
            fs::write(
                &state,
                json!({"status":status,"screen":format!("────────\n{screen}\n────────\n  status")})
                    .to_string(),
            )
            .unwrap()
        };
        let lines = || {
            fs::read_to_string(&sends)
                .unwrap_or_default()
                .lines()
                .count()
        };
        set("blocked", "❯ ");
        peer.run(
            &["send", "--host", win.host, "t4", "worker blocked mail"],
            "t2",
        );
        let ready = win.root.join("worker-ready");
        let mut command = win.cmd(
            &["relay", "serve", "--ready-file", ready.to_str().unwrap()],
            "t4",
        );
        command
            .env("FAKE_STATE", &state)
            .env("FAKE_SENDS", &sends)
            .stdout(Stdio::null());
        let mut worker = Broker(command.spawn().unwrap());
        wait_for(|| ready.exists());
        let mut duplicate = win.cmd(&["relay", "start"], "t4");
        duplicate
            .env("FAKE_STATE", &state)
            .env("FAKE_SENDS", &sends);
        assert!(output(duplicate, true).contains("already running"));
        wait_for(|| {
            rt.block_on(async {
                vega_registry::get_one(client, &vega_registry::agent_key(win.host, "t4"))
                    .await
                    .unwrap()
                    .is_some_and(|agent| {
                        assert_eq!(agent["schema"], 2);
                        assert_eq!(agent["project"], "example.invalid/team/native");
                        true
                    })
            })
        });
        wait_for(|| {
            rt.block_on(vega_registry::get_one(
                client,
                &vega_registry::beat_key(win.host),
            ))
            .unwrap()
            .is_some_and(|beat| {
                beat["native"]["inventory"]["last_success"]
                    .as_i64()
                    .is_some()
            })
        });
        assert_eq!(lines(), 0, "blocked waits");
        set("unknown", "❯ ");
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(lines(), 0, "unknown waits");
        set("idle", "❯ unfinished draft");
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(lines(), 0, "draft waits");
        set("idle", "❯ ");
        wait_for(|| lines() == 1);
        let sent = fs::read_to_string(&sends).unwrap();
        assert!(sent.contains("[vega-post]"));
        assert!(sent.contains(" inbox"));
        assert!(!sent.contains("worker blocked mail"));
        assert!(!sent.contains("other/t9"));
        // Worker counts mail but never fetches, leases or acknowledges it.
        assert!(win.run(&["status"], "t4").contains("unread: 1"));
        rt.block_on(async {
            let stream = vega_registry::broker_context(client.clone())
                .get_stream(format!("VEGA_INBOX_{}", win.host))
                .await
                .unwrap();
            assert!(
                stream
                    .consumer_info(format!("inbox-{}-t4", win.host))
                    .await
                    .is_err(),
                "worker must not create or fetch an inbox consumer"
            );
            assert!(vega_registry::get_one(
                client,
                &vega_registry::agent_key(win.host, "other/t9")
            )
            .await
            .unwrap()
            .is_none());
        });
        eprintln!("native relay: pointer state and lock restart");
        // Killing the worker releases its lock; persisted pointer state survives restart.
        worker.0.kill().unwrap();
        worker.0.wait().unwrap();
        fs::remove_file(&ready).unwrap();
        let mut again = win.cmd(
            &["relay", "serve", "--ready-file", ready.to_str().unwrap()],
            "t4",
        );
        again
            .env("FAKE_STATE", &state)
            .env("FAKE_SENDS", &sends)
            .stdout(Stdio::null());
        let _again = Broker(again.spawn().unwrap());
        wait_for(|| ready.exists());
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(lines(), 1, "restart does not duplicate unread pointer");
        eprintln!("native relay: broker restart and hc outage recovery");
        // A broker restart retains inbox state; the same worker reconnects/rescans.
        rt.block_on(async {
            broker.0.kill().unwrap();
            broker.0.wait().unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
            broker.0 = Command::new("nats-server")
                .arg("-c")
                .arg(conf)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            // Drive the test observer's current-thread runtime through reconnect too.
            tokio::time::sleep(Duration::from_secs(3)).await;
            tokio::time::timeout(Duration::from_secs(15), client.flush())
                .await
                .expect("test observer did not reconnect to the restarted broker")
                .unwrap();
        });
        set("outage", "❯ ");
        std::thread::sleep(Duration::from_secs(3));
        // A failed local inventory must not delete published agents.
        assert!(rt
            .block_on(vega_registry::get_one(
                client,
                &vega_registry::agent_key(win.host, "t4")
            ))
            .unwrap()
            .is_some());
        set("idle", "❯ ");
        // An actual inbox read advances ack ownership; subsequent mail gets a pointer.
        win.run(&["inbox"], "t4");
        peer.run(&["send", "--host", win.host, "t4", "next mail"], "t2");
        wait_for(|| lines() == 2);
        eprintln!("native relay: detached start lifetime");
        drop(_again);
        let mut start = win.cmd(&["relay", "start"], "t4");
        start.env("FAKE_STATE", &state).env("FAKE_SENDS", &sends);
        // Read lines and EOF separately: a detached worker must not keep the
        // starter's capture pipe alive after the starter has exited.
        let mut starter = start
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = starter.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                if tx.send(Some(line.unwrap())).is_err() {
                    return;
                }
            }
            let _ = tx.send(None);
        });
        let mut started = String::new();
        while !started.contains("pid: ") {
            let line = rx
                .recv_timeout(Duration::from_secs(10))
                .expect("starter did not report readiness")
                .expect("starter exited without a PID");
            started.push_str(&line);
            started.push('\n');
        }
        assert!(started.contains("relay: started"), "{started}");
        let pid = started
            .lines()
            .find_map(|l| l.strip_prefix("pid: "))
            .unwrap()
            .to_string();
        let _owned = OwnedWorker(pid);
        assert!(starter.wait().unwrap().success());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2))
                .expect("detached worker inherited starter stdout pipe"),
            None
        );
        reader.join().unwrap();
        eprintln!("native relay: starter returned and capture reached EOF");
        // Starter exited, but its detached child still owns the lock.
        assert!(win
            .run(&["relay", "start"], "t4")
            .contains("already running"));
        eprintln!("native relay: duplicate start confirmed");
        drop(_owned);
        eprintln!("native relay: owned worker cleanup finished");
        #[cfg(windows)]
        for mode in ["exec", "terminal"] {
            eprintln!("native relay: survive {mode} job closure");
            let source = win.cmd(&["relay", "start"], "t4");
            let mut command = Command::new(env::current_exe().unwrap());
            command
                .args(["--job-start", mode])
                .arg(source.get_program());
            command.current_dir(source.get_current_dir().unwrap());
            for (k, v) in source.get_envs() {
                if let Some(v) = v {
                    command.env(k, v);
                } else {
                    command.env_remove(k);
                }
            }
            command.env("FAKE_STATE", &state).env("FAKE_SENDS", &sends);
            // The helper itself is reaped with its job; its stdout is the evidence.
            let report = command.output().unwrap();
            let text = String::from_utf8_lossy(&report.stdout);
            assert!(
                text.contains("relay: started"),
                "{text}: {}",
                String::from_utf8_lossy(&report.stderr)
            );
            std::thread::sleep(Duration::from_millis(500));
            let status: Value =
                serde_json::from_str(&win.run(&["relay", "status", "--json"], "t4")).unwrap();
            assert_eq!(status["running"], true, "worker died with {mode} job");
            let _owned = OwnedWorker(status["pid"].to_string());
            assert!(win
                .run(&["relay", "start"], "t4")
                .contains("already running"));
            assert!(win.run(&["relay", "stop"], "t4").contains("relay: stopped"));
            assert!(win
                .run(&["relay", "status"], "t4")
                .contains("relay: stopped"));
            assert!(win
                .run(&["relay", "stop"], "t4")
                .contains("already stopped"));
        }
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
        let sidebar = Host::make(&root, "sidebar-test", &bin, &url);
        // The broker is still offline. Only this test's foreground worker and fake
        // core are used, including when running this case alone on a live host.
        let labels = sidebar.root.join("labels.jsonl");
        let state = sidebar.root.join("sidebar-state.json");
        fs::write(
            &state,
            json!({"status":"idle","labels":{"vega.role":"uac","vega.review":"keep"}}).to_string(),
        )
        .unwrap();
        fs::write(
            sidebar.root.join("roaming/vega/theme.toml"),
            "[symbols.hosts]\nsidebar-test = '🌔'\n",
        )
        .unwrap();
        let claims = sidebar.root.join("local/vega/uac/v2");
        fs::create_dir_all(&claims).unwrap();
        let claimed = "example.invalid/team/claimed";
        fs::write(
            claims.join(format!(
                "{}.json",
                vega_herdr::project::encode_component(claimed)
            )),
            json!({"schema":2,"project":claimed,"pane":"t5","host":sidebar.host}).to_string(),
        )
        .unwrap();
        let mut serve = sidebar.cmd(&["relay", "serve"], "t4");
        let worker = Broker(
            serve
                .env("FAKE_STATE", &state)
                .env("FAKE_LABEL_WRITES", &labels)
                .spawn()
                .unwrap(),
        );
        let expected = json!({
            "t4":{"vega.host":"🌔","vega.project":"native","vega.role":""},
            "t5":{"vega.host":"🌔","vega.project":"claimed","vega.role":"uac"},
            "t6":{"vega.host":"🌔","vega.project":"native"},
            "t7":{"vega.host":"🌔","vega.project":"native","vega.role":"sentinel"}
        });
        // Git identity arrives asynchronously. Apply the writes in order and
        // verify the final labels even if role and project arrive separately.
        wait_for(|| {
            let mut actual = json!({});
            let Ok(writes) = fs::read_to_string(&labels) else {
                return false;
            };
            for line in writes.lines() {
                let Ok(write) = serde_json::from_str::<Value>(line) else {
                    return false;
                };
                let pane = write["terminal"].as_str().unwrap();
                for (key, value) in write["labels"].as_object().unwrap() {
                    actual[pane][key] = value.clone();
                }
            }
            actual == expected
        });
        wait_for(|| {
            fs::read(sidebar.root.join("local/vega/vegad-health.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|h| {
                    ["inventory", "labels"].iter().all(|phase| {
                        h["native"][*phase]["last_success"]
                            .as_i64()
                            .is_some_and(|at| at > 0)
                            && h["native"][*phase]["error"].is_null()
                    })
                })
        });
        drop(worker);
        if env::args().any(|arg| arg == "--sidebar-only") {
            return;
        }
        let win = Host::make(&root, "native-test", &bin, &url);
        let peer = Host::make(&root, "peer-test", &bin, &url);
        let admin = nkeys::KeyPair::new_user();
        let conf = root.join("nats.conf");
        fs::write(&conf,format!("listen: 127.0.0.1:{port}\njetstream {{ store_dir: {}, max_file_store: 64G }}\nauthorization {{ users: [{{ nkey: \"{}\" }}, {}, {}] }}\n",json!(root.join("js").to_string_lossy()),admin.public_key(),permissions(win.host,&win.public()),permissions(peer.host,&peer.public()))).unwrap();
        let mut broker = Broker(
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
        #[cfg(windows)]
        {
            // Exercise the real Windows session API using the owned fake's server PID.
            // GitHub's runner can itself be in Services/session 0.
            let session = vega_herdr::platform::session_id(std::process::id()).unwrap();
            let report = output(win.cmd(&["doctor"], "t1"), session != 0);
            assert!(report.contains(if session == 0 {
                "hc_core_session_zero"
            } else {
                "(desktop)"
            }));
        }
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
        relay_suite(&win, &peer, &rt, &client, &mut broker, &conf);
        assert!(output(win.cmd(&["relay", "start", "--typo"], "t1"), false).contains("usage"));
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
        println!("native client suite: passed (restricted nkeys, enrollment, mail/acks, ancestry, relay guards/lock/detach/restart/reconnect, errors)");
    }
}
