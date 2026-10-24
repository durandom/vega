//! Actual CLI/startup path against a local broker and isolated owner state.
use serde_json::{json, Value};
use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn post(home: &Path, url: &str, args: &[&str]) -> Output {
    post_as(home, url, "a", args)
}
fn post_as(home: &Path, url: &str, host: &str, args: &[&str]) -> Output {
    vega_herdr::output_within(
        &mut post_command(home, url, host, args),
        Duration::from_secs(20),
    )
    .unwrap()
    .expect("command timed out")
}
fn post_command(home: &Path, url: &str, host: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_vega-post"));
    cmd.args(args)
        .env_clear()
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("VEGA_POST_HOST", host)
        .env("VEGA_NATS_URL", url)
        .env(
            "PATH",
            format!(
                "{}:{}",
                home.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .current_dir(home);
    cmd
}
#[track_caller]
fn wait_for(mut ready: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(
            std::time::Instant::now() < until,
            "timed out waiting for daemon state"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn quiesced(home: &Path, url: &str, host: &str, reason: &str) -> Server {
    quiesced_command(home, url, host, reason, &["relay", "--once"])
}
fn quiesced_command(home: &Path, url: &str, host: &str, reason: &str, args: &[&str]) -> Server {
    let calls = fs::read(home.join("herdr-calls")).unwrap_or_default();
    let logfile = home.join("quiesced.log");
    let mut cmd = post_command(home, url, host, args);
    cmd.stdout(fs::File::create(&logfile).unwrap())
        .stderr(Stdio::null());
    let mut child = Server(cmd.spawn().unwrap());
    wait_for(|| {
        fs::read_to_string(&logfile)
            .unwrap_or_default()
            .contains(reason)
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "cutover failure must stay alive"
    );
    // Only the sidebar's identity-free labels (main.rs report_quiesced):
    // herdr's lists and pane metadata, no project work.
    let after = fs::read(home.join("herdr-calls")).unwrap_or_default();
    let new = String::from_utf8_lossy(&after[calls.len()..]).into_owned();
    assert!(
        new.lines()
            .all(|l| ["pane list", "workspace list", "tab list"].contains(&l)
                || l.starts_with("pane report-metadata ")),
        "quiesced daemon started project work: {new}"
    );
    assert!(
        fs::read_to_string(home.join(format!("state/vega/post-relay-{host}.broker")))
            .unwrap()
            .contains("identity quiesced")
    );
    // Check the actual shared state API under the child's isolated HOME,
    // without mutating the test runner's process-global environment.
    success(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "quiesced_state_probe", "--nocapture"])
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("VEGA_POST_HOST", host)
            .env("VEGA_IDENTITY_STATE_PROBE", "1")
            .output()
            .unwrap(),
    );
    let status = post_as(home, url, host, &["relay", "status"]);
    assert!(!String::from_utf8_lossy(&status.stdout).contains("off:"));
    success(status);
    child
}

#[test]
fn quiesced_state_probe() {
    if std::env::var_os("VEGA_IDENTITY_STATE_PROBE").is_some() {
        assert!(vega_herdr::state::relay_running());
        assert!(!vega_herdr::state::relay_local());
    }
}

fn success(out: Output) {
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
#[test]
fn startup_requires_approval_migrates_restarts_and_rollback_blocks_forward_start() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let home = home.as_path();
    fs::create_dir_all(home.join("bin")).unwrap();
    fs::write(
        home.join("bin/herdr"),
        "#!/bin/sh\necho \"$*\" >> \"$HOME/herdr-calls\"\necho '{\"result\":{\"agents\":[],\"workspaces\":[],\"panes\":[]}}'\n",
    )
    .unwrap();
    fs::set_permissions(home.join("bin/herdr"), fs::Permissions::from_mode(0o755)).unwrap();
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
            .arg(conf)
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
    let (c, kv) = rt.block_on(async {
        let mut client = None;
        for _ in 0..50 {
            if let Ok(c) = async_nats::connect(&url).await {
                client = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let c = client.expect("broker starts");
        let kv = async_nats::jetstream::new(c.clone())
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: vega_registry::BUCKET.into(),
                history: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        (c, kv)
    });
    drop(quiesced(home, &url, "a", "identity_cutover"));
    let repo = home.join("repo");
    fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["remote", "add", "origin", "git@github.com:team/repo.git"])
        .status()
        .unwrap()
        .success());
    let m = json!({"old_key":"~/repo/.git","project":"github.com/team/repo","location":{"common_dir":repo.join(".git"),"checkout":repo}});
    fs::write(home.join("mappings.json"), json!([m]).to_string()).unwrap();
    let state = home.join("state/vega");
    fs::create_dir_all(state.join("uac")).unwrap();
    let legacy = state.join("uac/local_~_repo_.git");
    let original=json!({"project":"~/repo/.git","pane":"p1","host":"a","at":7,"tracker":"task","folder_name":"repo"}).to_string();
    fs::write(&legacy, &original).unwrap();
    let old = json!({"project":"~/repo/.git","pane":"p1","host":"a","at":3,"folder_name":"repo","origin_name":"historic","aliases":["registry-only"]});
    // A second owner's pending legacy pin holds ordinary reconciliation on
    // the first owner after its local migration has completed.
    let b = home.join("b");
    fs::create_dir_all(b.join("bin")).unwrap();
    fs::copy(home.join("bin/herdr"), b.join("bin/herdr")).unwrap();
    fs::copy(home.join("mappings.json"), b.join("mappings.json")).unwrap();
    let old_pin = json!({"project":"~/repo/.git","seat":"s","lane":"l","tier":"t","model":null,"at":3,"expires":1});
    rt.block_on(async {
        kv.put("a.uac.old", old.to_string().into()).await.unwrap();
        kv.put("b.lanista.old", old_pin.to_string().into())
            .await
            .unwrap();
    });
    // Cover every local record kind, then generate the file consumed by the
    // real preview CLI. Distinct keys per payload source are covered below in
    // the collector unit fixture; this fixture also exercises pure plan checks.
    for (path, value) in [
        ("uac-status/local_~_repo_.git", json!(1792573200_i64)),
        (
            "badges/__repo_.git.json",
            json!({"project":"~/repo/.git","at":8}),
        ),
        (
            "notes/5000.json",
            json!({"id":5000,"host":"a","project":"~/repo/.git","cause":"test","title":"test","body":"body","resolved":null}),
        ),
        ("notes/seen.json", json!({"read":{"a":4000},"cleared":{}})),
        (
            "hygiene.json",
            json!({"daily":{"~/repo/.git":{"at":8}},"waiting":{}}),
        ),
        (
            "pagerduty/state.json",
            json!({"version":1,"incidents":{"I":{"delivered":{"to":{"Board":{"project":"~/repo/.git"}}}}}}),
        ),
        (
            "bd/projects/__repo_.git.json",
            json!({"key":"~/repo/.git","dir":repo,"database":"TEST","remote":"http://remote/TEST","bd":"/bin/bd","data":repo.join(".beads/dolt"),"since":1,"resumed":2}),
        ),
        (
            "bd/state/__repo_.git.json",
            serde_json::to_value(vega_bd_sync::store::State::default()).unwrap(),
        ),
        (
            "bd/git/__repo_.git.json",
            serde_json::to_value(vega_bd_sync::git::GitState::default()).unwrap(),
        ),
    ] {
        let path = state.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value.to_string()).unwrap();
    }
    fs::create_dir_all(state.join("lanes")).unwrap();
    fs::write(
        state.join("lanes/local_~_repo_.git"),
        "{\"actor\":\"a/fix\",\"since\":3,\"pane\":\"p1\"}\n",
    )
    .unwrap();
    let source_bytes = fs::read(&legacy).unwrap();
    success(post(
        home,
        &url,
        &["identity", "mappings", "--output", "mappings.json"],
    ));
    let generated: Value =
        serde_json::from_slice(&fs::read(home.join("mappings.json")).unwrap()).unwrap();
    assert_eq!(generated, json!([m]));
    assert_eq!(fs::read(&legacy).unwrap(), source_bytes);
    // Failure reports all unresolved paths and keeps an existing export intact.
    let bad_note = state.join("notes/5001.json");
    fs::write(&bad_note, json!({"project":"~/missing/.git"}).to_string()).unwrap();
    for args in [
        vec!["remote", "rename", "origin", "upstream"],
        vec![
            "remote",
            "add",
            "other",
            "https://github.com/team/other.git",
        ],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let failed = post(
        home,
        &url,
        &["identity", "mappings", "--output", "mappings.json"],
    );
    assert_eq!(failed.status.code(), Some(1));
    let error = String::from_utf8(failed.stdout).unwrap();
    assert!(error.contains("error{code,message,hint}") && error.contains("~/missing/.git"));
    assert!(
        error.contains("~/repo/.git")
            && error.contains("project_remote_ambiguous")
            && error.contains("vega.projectRemote")
    );
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "vega.projectRemote", "upstream"])
        .status()
        .unwrap()
        .success());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(home.join("mappings.json")).unwrap()).unwrap(),
        generated
    );
    fs::remove_file(bad_note).unwrap();
    success(post(
        home,
        &url,
        &[
            "identity",
            "preview",
            "--mappings",
            "mappings.json",
            "--output",
            "a.json",
        ],
    ));
    assert_eq!(fs::read_to_string(&legacy).unwrap(), original);
    assert!(!state.join("identity-cutover/journal.json").exists());
    success(post_as(
        &b,
        &url,
        "b",
        &[
            "identity",
            "preview",
            "--mappings",
            "mappings.json",
            "--output",
            "b.json",
        ],
    ));
    fs::write(home.join("grants.json"), "{}").unwrap();
    success(post(
        home,
        &url,
        &[
            "identity",
            "audit",
            "--plan",
            "a.json",
            "--plan",
            b.join("b.json").to_str().unwrap(),
            "--host",
            "a",
            "--host",
            "b",
            "--grants",
            "grants.json",
            "--output",
            "approved.json",
        ],
    ));
    fs::create_dir_all(state.join("identity-cutover")).unwrap();
    // A running, quiesced daemon retries when the audited bundle arrives.
    let mut waiting = quiesced_command(home, &url, "a", "identity_cutover", &["relay"]);
    fs::copy(
        home.join("approved.json"),
        state.join("identity-cutover/approved.json"),
    )
    .unwrap();
    let marker = state.join("identity-cutover/complete.json");
    wait_for(|| marker.exists());
    wait_for(|| {
        fs::read_to_string(home.join("quiesced.log"))
            .unwrap()
            .contains("heartbeat only")
    });
    // Make A's one-shot startup beat stale without sleeping for FRESH. A
    // continuing guard block must refresh it on the normal 15-second cadence,
    // rather than the 120-second failure backoff that exceeds FRESH (90 s).
    rt.block_on(async {
        kv.put("a.beat", json!({"schema":2,"at":1}).to_string().into())
            .await
            .unwrap();
    });
    wait_for(|| {
        rt.block_on(async {
            vega_registry::get_one(&c, "a.beat")
                .await
                .unwrap()
                .is_some_and(|v| v["schema"] == 2 && v["at"].as_i64().unwrap() > 1)
        })
    });
    assert!(waiting.0.try_wait().unwrap().is_none());
    rt.block_on(async {
        assert_eq!(
            vega_registry::get_one(&c, "b.lanista.old").await.unwrap(),
            Some(old_pin)
        );
    });
    let b_cutover = b.join("state/vega/identity-cutover");
    fs::create_dir_all(&b_cutover).unwrap();
    fs::copy(home.join("approved.json"), b_cutover.join("approved.json")).unwrap();
    success(post_as(&b, &url, "b", &["relay", "--once"]));
    success(post(home, &url, &["identity", "verify"]));
    wait_for(|| {
        fs::read_to_string(home.join("quiesced.log"))
            .unwrap()
            .contains("registry: in sync")
    });
    drop(waiting);
    let recovery_log = fs::read_to_string(home.join("quiesced.log")).unwrap();
    assert_eq!(recovery_log.matches("identity: quiesced").count(), 1);
    assert!(recovery_log.contains("leaving quiesced startup"));
    let journal = fs::read(state.join("identity-cutover/journal.json")).unwrap();
    assert!(!legacy.exists());
    let v2 = state.join(format!(
        "uac/v2/{}.json",
        vega_herdr::project::encode_component("github.com/team/repo")
    ));
    let claim: Value = serde_json::from_slice(&fs::read(&v2).unwrap()).unwrap();
    assert_eq!(claim["tracker"], "task");
    assert_eq!(claim["at"], 3);
    assert!(claim["aliases"]
        .as_array()
        .unwrap()
        .contains(&json!("registry-only")));
    let plan: Value = serde_json::from_slice(&fs::read(home.join("a.json")).unwrap()).unwrap();
    let uac_key = vega_registry::uac_key("a", "github.com/team/repo");
    let expected = plan["registry"]["projection"][&uac_key].clone();
    rt.block_on(async {
        assert!(kv.get("a.uac.old").await.unwrap().is_none());
        let published =
            vega_registry::get_one(&c, &vega_registry::uac_key("a", "github.com/team/repo"))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(published, vega_registry::uac_entry("a", &claim).unwrap());
        assert_eq!(published, expected);
    });
    // Even a completed offline host cannot rejoin an online mixed protocol.
    rt.block_on(async {
        kv.put("old.beat",json!({"at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()}).to_string().into()).await.unwrap();
    });
    drop(quiesced(home, &url, "a", "mixed version"));
    rt.block_on(async {
        kv.delete("old.beat").await.unwrap();
    });
    // Even byte-identical planned legacy values may not reappear after completion.
    rt.block_on(async {
        kv.put("a.uac.old", old.to_string().into()).await.unwrap();
    });
    drop(quiesced(
        home,
        &url,
        "a",
        "legacy project key active after completion",
    ));
    rt.block_on(async {
        kv.delete("a.uac.old").await.unwrap();
    });
    // Simulate interruption immediately before completion marker persistence.
    fs::remove_file(&marker).unwrap();
    let _ = post(home, &url, &["relay", "--once"]);
    assert!(marker.exists());
    assert_eq!(
        fs::read(state.join("identity-cutover/journal.json")).unwrap(),
        journal
    );
    // relay --once performs its ordinary registry reconcile as well as startup.
    rt.block_on(async {
        assert_eq!(
            vega_registry::get_one(&c, &uac_key).await.unwrap(),
            Some(expected)
        );
    });
    // The exported ACL alone is insufficient: this deliberately overprivileged
    // broker allows STREAM.INFO. Live inbox startup must fail without a marker.
    let reader = home.join("reader");
    let cutover = reader.join("state/vega/identity-cutover");
    fs::create_dir_all(&cutover).unwrap();
    fs::create_dir_all(reader.join("config/vega")).unwrap();
    fs::write(reader.join("config/vega/inbox-only"), "reader\n").unwrap();
    let mut approved: Value =
        serde_json::from_slice(&fs::read(home.join("approved.json")).unwrap()).unwrap();
    // Inbox-only restricts registry writes, but real inbox hosts have local
    // claims and badges too. Exercise their actual startup and rollback path.
    let mappings: Vec<vega_herdr::cutover::Mapping> =
        serde_json::from_value(approved["hosts"][0]["mappings"].clone()).unwrap();
    let reader_original = vega_herdr::cutover::Snapshot::from([
        ("uac/local_~_repo_.git".into(), json!({"project":"~/repo/.git","host":"reader","pane":"r1","at":7}).to_string().into_bytes()),
        ("badges/__repo_.git.json".into(), json!({"project":"~/repo/.git","at":8,"notes":0,"decisions":0,"waiting":0,"waiting_panes":[]}).to_string().into_bytes()),
    ]);
    for (path, bytes) in &reader_original {
        let path = reader.join("state/vega").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let local = vega_herdr::cutover::prepare(&reader_original, &mappings, "reader").unwrap();
    approved["hosts"].as_array_mut().unwrap().push(json!({"schema":2,"host":"reader","inbox_only":true,"mappings":mappings,"original":reader_original,"writes":local.writes,"remove":local.remove,"registry":{"host":"reader","backup":{},"projection":{}},"owned":{},"registry_reads":[uac_key]}));
    approved["grants"] = json!({"reader":{"publish":[vega_registry::direct_get_subject(&uac_key)],"subscribe":["_INBOX_reader.>"]}});
    fs::write(cutover.join("approved.json"), approved.to_string()).unwrap();
    drop(quiesced(&reader, &url, "reader", "request was allowed"));
    assert!(!cutover.join("complete.json").exists());
    for path in local.writes.keys() {
        assert!(reader.join("state/vega").join(path).exists());
    }
    success(post_as(&reader, &url, "reader", &["identity", "rollback"]));
    for (path, bytes) in &reader_original {
        assert_eq!(
            fs::read(reader.join("state/vega").join(path)).unwrap(),
            *bytes
        );
    }
    for path in local.writes.keys() {
        assert!(!reader.join("state/vega").join(path).exists());
    }
    success(post(home, &url, &["identity", "rollback"]));
    success(post(home, &url, &["identity", "rollback"]));
    assert_eq!(fs::read_to_string(legacy).unwrap(), original);
    assert!(!v2.exists());
    assert!(!marker.exists());
    rt.block_on(async {
        assert_eq!(
            vega_registry::get_one(&c, "a.uac.old").await.unwrap(),
            Some(old)
        );
    });
    drop(quiesced(home, &url, "a", "rollback"));
}

#[test]
fn inbox_mappings_need_no_broker_and_reject_exports_into_live_state() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    // An invalid endpoint would fail if this inbox-only host tried connecting.
    let url = "not-a-nats-url";
    success(post_as(
        &home,
        url,
        "deimos",
        &["identity", "mappings", "--output", "mappings.json"],
    ));
    assert_eq!(
        fs::read_to_string(home.join("mappings.json")).unwrap(),
        "[]"
    );

    let state = home.join("state/vega");
    let original = b"source bytes must stay unchanged";
    fs::write(state.join("existing.json"), original).unwrap();
    std::os::unix::fs::symlink(&state, home.join("state-alias")).unwrap();
    std::os::unix::fs::symlink(state.join("existing.json"), home.join("file-alias.json")).unwrap();
    for output in [
        "state/vega/new.json",
        "state/vega/existing.json",
        "state-alias/new.json",
        "state-alias/existing.json",
        "file-alias.json",
    ] {
        let failed = post_as(
            &home,
            url,
            "deimos",
            &["identity", "mappings", "--output", output],
        );
        assert_eq!(failed.status.code(), Some(1));
        let error = String::from_utf8(failed.stdout).unwrap();
        assert!(
            error.contains("error{code,message,hint}")
                && error.contains("outside the live vega state root"),
            "{error}"
        );
        assert_eq!(fs::read(state.join("existing.json")).unwrap(), original);
        assert!(!state.join("new.json").exists());
        assert!(!state.join("existing.cutover-tmp").exists());
        assert!(!state.join("new.cutover-tmp").exists());
    }
    assert!(fs::symlink_metadata(home.join("file-alias.json"))
        .unwrap()
        .file_type()
        .is_symlink());
    let temp = home.join("mappings.cutover-tmp");
    std::os::unix::fs::symlink(state.join("existing.json"), &temp).unwrap();
    let failed = post_as(
        &home,
        url,
        "deimos",
        &["identity", "mappings", "--output", "mappings.json"],
    );
    assert_eq!(failed.status.code(), Some(1));
    assert!(String::from_utf8(failed.stdout)
        .unwrap()
        .contains("unsafe cutover temporary file"));
    assert_eq!(fs::read(state.join("existing.json")).unwrap(), original);
    assert_eq!(
        fs::read_to_string(home.join("mappings.json")).unwrap(),
        "[]"
    );
    assert!(fs::symlink_metadata(&temp)
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_file(&temp).unwrap();
    // A stale plain temp from an interrupted write is safe to replace. Even
    // when hard-linked, unlinking it must preserve the source bytes.
    fs::hard_link(state.join("existing.json"), &temp).unwrap();
    success(post_as(
        &home,
        url,
        "deimos",
        &["identity", "mappings", "--output", "mappings.json"],
    ));
    assert_eq!(fs::read(state.join("existing.json")).unwrap(), original);
    assert!(!temp.exists());
    let outside = home.join("outside.json");
    fs::write(&outside, b"outside bytes").unwrap();
    let inside_link = state.join("outside-link.json");
    std::os::unix::fs::symlink(&outside, &inside_link).unwrap();
    let failed = post_as(
        &home,
        url,
        "deimos",
        &[
            "identity",
            "mappings",
            "--output",
            "state/vega/outside-link.json",
        ],
    );
    assert_eq!(failed.status.code(), Some(1));
    assert!(String::from_utf8(failed.stdout)
        .unwrap()
        .contains("outside the live vega state root"));
    assert!(fs::symlink_metadata(&inside_link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&inside_link).unwrap(), outside);
    assert_eq!(fs::read(&outside).unwrap(), b"outside bytes");
    success(post_as(
        &home,
        url,
        "deimos",
        &["identity", "mappings", "--output", "suffix.cutover-tmp"],
    ));
    assert_eq!(
        fs::read_to_string(home.join("suffix.cutover-tmp")).unwrap(),
        "[]"
    );
    assert!(!home.join("suffix.cutover-tmp.next").exists());
}
