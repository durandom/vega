use std::process::Command;
fn command(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_vega-history"));
    cmd.env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("VEGA_POST_HOST", "fixture")
        .env("VEGA_NATS_URL", "nats://127.0.0.1:1")
        .env_remove("VEGA_HISTORY_VIEWER_KEY");
    cmd
}
#[test]
fn local_status_needs_no_key_and_validation_precedes_network() {
    let dir = tempfile::tempdir().unwrap();
    let status = command(dir.path()).arg("status").output().unwrap();
    assert!(status.status.success());
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(stdout.contains("fixture,null,null,0,null"), "{stdout}");
    let read = command(dir.path()).arg("read").output().unwrap();
    assert_eq!(read.status.code(), Some(1));
    assert!(String::from_utf8(read.stdout)
        .unwrap()
        .contains("viewer_key"));
    let invalid = command(dir.path())
        .args(["export", "--unknown"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    let output = String::from_utf8(invalid.stdout).unwrap();
    assert!(output.contains("error{code,message,hint}"));
    assert!(output.contains("--unknown"));
    for flag in ["--help", "--version", "-v", "-V", "--skill"] {
        assert!(command(dir.path())
            .arg(flag)
            .output()
            .unwrap()
            .status
            .success());
    }
}

struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn cli_reads_and_exports_full_filtered_jsonl_with_viewer_only() {
    let dir = tempfile::tempdir().unwrap();
    let admin = nkeys::KeyPair::new_user();
    let viewer = nkeys::KeyPair::new_user();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = dir.path().join("nats.conf");
    std::fs::write(&config, format!("listen: 127.0.0.1:{port}\njetstream {{ store_dir: \"{}\" }}\nauthorization {{ users: [{{nkey: {}}}, {{nkey: {}, permissions: {{publish: [\"$JS.API.STREAM.INFO.VEGA_HISTORY\", \"$JS.API.STREAM.MSG.GET.VEGA_HISTORY\"], subscribe: [\"_INBOX_viewer.>\"]}}}}] }}", dir.path().join("js").display(), admin.public_key(), viewer.public_key())).unwrap();
    let _server = Server(
        Command::new("nats-server")
            .arg("-c")
            .arg(config)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("nats-server: run through mise"),
    );
    let url = format!("nats://127.0.0.1:{port}");
    let key = dir.path().join("viewer.nk");
    std::fs::write(&key, viewer.seed().unwrap()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut connected = None;
        for _ in 0..50 {
            match async_nats::ConnectOptions::with_nkey(admin.seed().unwrap()).connect(&url).await {
                Ok(client) => {connected = Some(client); break;}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
        let js = async_nats::jetstream::new(connected.expect("test broker started"));
        js.create_stream(async_nats::jetstream::stream::Config {name:"VEGA_HISTORY".into(), subjects:vec!["vega.history.>".into()], ..Default::default()}).await.unwrap();
        for (i, project) in ["p", "other", "p"].iter().enumerate() {
            let event = serde_json::json!({"schema":1,"id":i.to_string(),"host":"h","kind":"mail.sent","ts":10+i,"project":project,"bead":"b","pane":null,"session":null,"data":{"body":"full body\nwith newline"}});
            js.publish(format!("vega.history.h.{i}"), event.to_string().into()).await.unwrap().await.unwrap();
        }
    });
    let query = |args: &[&str]| {
        command(dir.path())
            .env("VEGA_NATS_URL", &url)
            .arg(args[0])
            .arg("--viewer-key")
            .arg(&key)
            .args(&args[1..])
            .output()
            .unwrap()
    };
    let export = query(&[
        "export",
        "--project",
        "p",
        "--bead",
        "b",
        "--host",
        "h",
        "--since",
        "10",
    ]);
    assert!(
        export.status.success(),
        "{}",
        String::from_utf8_lossy(&export.stdout)
    );
    let text = String::from_utf8(export.stdout).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["data"]["body"], "full body\nwith newline");
    assert_eq!(events[1]["id"], "2");
    assert!(String::from_utf8(export.stderr).unwrap().contains("2 of 2"));
    let read = query(&["read", "--project", "p", "--limit", "1"]);
    assert!(read.status.success());
    let text = String::from_utf8(read.stdout).unwrap();
    assert!(text.contains("count: 1 of 2 matching events"), "{text}");
    assert!(text.contains("events[1]{seq,ts,host,kind,id}"));
    assert!(text.contains("--project p"));
    assert!(!text.contains("full body"));
    let empty = query(&["export", "--host", "absent"]);
    assert!(empty.status.success());
    assert!(empty.stdout.is_empty());
    assert!(String::from_utf8(empty.stderr).unwrap().contains("0 of 0"));
    let after = query(&["export", "--after", "2"]);
    assert!(after.status.success());
    assert_eq!(String::from_utf8(after.stdout).unwrap().lines().count(), 1);
    let limited = query(&["export", "--project", "p", "--limit", "1"]);
    assert!(limited.status.success());
    assert_eq!(
        String::from_utf8(limited.stdout).unwrap().lines().count(),
        1
    );
    let progress = String::from_utf8(limited.stderr).unwrap();
    assert!(
        progress.contains("last emitted sequence 1; snapshot through 3"),
        "{progress}"
    );
    let hostile = query(&["read", "--project", "$(id)' p"]);
    assert!(hostile.status.success());
    assert!(String::from_utf8(hostile.stdout)
        .unwrap()
        .contains("--project '$(id)'\"'\"' p'"));
    rt.block_on(async {
        let client = async_nats::ConnectOptions::with_nkey(admin.seed().unwrap())
            .connect(&url)
            .await
            .unwrap();
        async_nats::jetstream::new(client)
            .publish("vega.history.h.bad", "{}".into())
            .await
            .unwrap()
            .await
            .unwrap();
    });
    let broken = query(&["export"]);
    assert_eq!(broken.status.code(), Some(1));
    assert!(String::from_utf8(broken.stdout)
        .unwrap()
        .contains("history_schema"));
    // Missing viewer path cannot silently use a configured host/admin seed.
    let denied = command(dir.path())
        .env("VEGA_NATS_URL", &url)
        .env("VEGA_NATS_NKEY", &key)
        .arg("read")
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8(denied.stdout)
        .unwrap()
        .contains("viewer_key"));
}

#[test]
fn overflowing_cursor_is_usage_error_before_network() {
    // CLI invalid-input handling is checked without consulting any configured broker.
    let dir = tempfile::tempdir().unwrap();
    let invalid = command(dir.path())
        .args(["read", "--after", "18446744073709551616"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
}
