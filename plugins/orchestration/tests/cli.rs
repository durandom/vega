use std::process::Command;

#[test]
fn guide_help_version_and_invalid_arguments() {
    let bin = env!("CARGO_BIN_EXE_vega-orchestration");
    let out = Command::new(bin).arg("--skill").output().unwrap();
    assert!(out.status.success());
    vega_plugin::assert_guide(std::str::from_utf8(&out.stdout).unwrap(), "orchestration");
    for args in [vec!["--help"], vec!["--version"], vec!["-v"], vec!["-V"]] {
        assert!(Command::new(bin)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    for args in [
        vec!["--parent"],
        vec!["--parents", "x"],
        vec!["dispatch"],
        vec!["--help", "--typo"],
        vec!["waiting", "--on", "Slayer"],
        vec!["waiting", "--waiter", "other"],
        vec![
            "waiting", "--on", "Slayer", "--on", "Slayer", "--for", "test",
        ],
        vec!["waiting", "--on", "invalid-owner", "--for", "test"],
        vec!["clear", "invalid-id", "--evidence", "test"],
    ] {
        let out = Command::new(bin).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stdout).contains("usage"));
    }
}

#[test]
fn unavailable_sources_are_explicit_and_home_does_not_create_wait_state() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .unwrap()
        .success());
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    let bd = bin_dir.join("bd");
    std::fs::write(&bd, "#!/bin/sh\nprintf '[]\\n'\n").unwrap();
    std::fs::set_permissions(&bd, std::fs::Permissions::from_mode(0o755)).unwrap();
    // macOS scans a new executable on its first run; keep that out of the
    // CLI's bounded bd probe.
    let _ = Command::new(&bd).output();
    let state = dir.path().join("state");
    let out = Command::new(env!("CARGO_BIN_EXE_vega-orchestration"))
        .current_dir(dir.path())
        .env("XDG_STATE_HOME", &state)
        .env("VEGA_NATS_URL", "nats://127.0.0.1:1")
        .env(
            "PATH",
            format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("registry_questions: unknown (registry unavailable)"));
    assert!(text.contains("post_asks_local: unknown"));
    assert!(text.contains("explicit_waits_local: unknown"));
    assert!(text.contains("ready_count: 0"));
    assert!(!state.join("vega/orchestration/waits.sqlite3").exists());
    // An inbox-only host skips the read and says why.
    let out = Command::new(env!("CARGO_BIN_EXE_vega-orchestration"))
        .current_dir(dir.path())
        .env("XDG_STATE_HOME", &state)
        .env("VEGA_NATS_URL", "nats://127.0.0.1:1")
        .env("VEGA_POST_HOST", "puck")
        .env("VEGA_POST_INBOX_ONLY", "puck")
        .env(
            "PATH",
            format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("registry: off (inbox-only host: local agents only)"),
        "{text}"
    );
}

#[test]
fn remote_update_rejects_host_impersonation_before_opening_state() {
    use std::io::Write;
    use std::process::Stdio;
    use vega_herdr::waits::{Actor, Change, Endpoint, Request};
    let dir = tempfile::tempdir().unwrap();
    let ep = Endpoint {
        host: "worker".into(),
        pane: "w1:p1".into(),
        name: "child".into(),
    };
    let req = Request {
        version: 1,
        actor: Actor {
            endpoint: ep.clone(),
            session: "session".into(),
        },
        change: Change::Waiting {
            coordinator: ep,
            on: "Slayer".into(),
            reason: "decision".into(),
            bead: None,
            thread: None,
        },
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_vega-orchestration"))
        .args(["remote-update", "--from", "impostor"])
        .env("XDG_STATE_HOME", dir.path())
        .env("VEGA_POST_HOST", "owner")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&req).unwrap())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("wait_identity"));
    assert!(!dir.path().join("vega/orchestration/waits.sqlite3").exists());
}

#[test]
fn local_wait_and_clear_cli_survive_restarts_and_fence_reused_waiter_sessions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let herdr = bin.join("herdr");
    std::fs::write(
        &herdr,
        r#"#!/bin/sh
case "$*" in
  *"pane list"*) printf '%s\n' '{"result":{"panes":[{"pane_id":"w1:p1","agent":"codex"}]}}' ;;
  *"pane process-info"*) printf '{"result":{"process_info":{"shell_pid":%s}}}\n' "$TEST_PARENT_PID" ;;
  *"agent get w1:p1"*) printf '%s\n' '{"result":{"agent":{"pane_id":"w1:p1","name":"child"}}}' ;;
  *"agent get w2:p1"*) printf '%s\n' '{"result":{"agent":{"pane_id":"w2:p1","name":"parent"}}}' ;;
  *) exit 1 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
    // macOS scans a new executable on its first run, seconds on a loaded
    // host: run the fake once so the CLI's 2 s herdr bound sees only herdr.
    let _ = std::process::Command::new(&herdr).output();
    let state = dir.path().join("state");
    let parents = state.join("vega/parents");
    std::fs::create_dir_all(&parents).unwrap();
    std::fs::write(
        parents.join("local_w1_p1"),
        r#"{"name":"child","parent":"w2:p1","host":"worker"}"#,
    )
    .unwrap();
    let command = |args: &[&str], session: &str| {
        Command::new(env!("CARGO_BIN_EXE_vega-orchestration"))
            .args(args)
            .env("XDG_STATE_HOME", &state)
            .env("VEGA_POST_HOST", "worker")
            .env("HERDR_PANE_ID", "w1:p1")
            .env("TEST_PARENT_PID", std::process::id().to_string())
            .env("CODEX_THREAD_ID", session)
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap()
    };
    let waiting = [
        "waiting",
        "--on",
        "Slayer",
        "--for",
        "decision pending",
        "--bead",
        "b-1",
    ];
    let out = command(&waiting, "s1");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let file = state.join("vega/orchestration/waits.sqlite3");
    let first = vega_herdr::waits::Store::read(&file)
        .unwrap()
        .pop()
        .unwrap();
    let out = command(&waiting, "s1");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("already_waiting"));
    assert_eq!(vega_herdr::waits::Store::read(&file).unwrap().len(), 1);
    let clear = [
        "clear",
        first.id.as_str(),
        "--evidence",
        "inspected decision",
    ];
    let out = command(&clear, "reused");
    assert_eq!(out.status.code(), Some(1));
    assert!(vega_herdr::waits::Store::read(&file).unwrap()[0]
        .resolved
        .is_none());
    assert!(command(&clear, "s1").status.success());
    let settled = vega_herdr::waits::Store::read(&file)
        .unwrap()
        .pop()
        .unwrap();
    let out = command(&clear, "s1");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("already_cleared"));
    assert_eq!(vega_herdr::waits::Store::read(&file).unwrap()[0], settled);
}

#[test]
fn remote_update_on_an_inbox_only_host_skips_the_registry_wait() {
    use std::io::Write;
    use std::process::Stdio;
    use vega_herdr::waits::{Actor, Change, Endpoint, Request};
    let dir = tempfile::tempdir().unwrap();
    let ep = Endpoint {
        host: "worker".into(),
        pane: "w1:p1".into(),
        name: "child".into(),
    };
    let req = Request {
        version: 1,
        actor: Actor {
            endpoint: ep.clone(),
            session: "session".into(),
        },
        change: Change::Waiting {
            coordinator: ep,
            on: "Slayer".into(),
            reason: "decision".into(),
            bead: None,
            thread: None,
        },
    };
    // A broker that accepts and never answers: a registry read would wait out its timeout.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("nats://{}", silent.local_addr().unwrap());
    let run = |inbox_only: bool| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_vega-orchestration"));
        c.args(["remote-update", "--from", "worker"])
            .env("XDG_STATE_HOME", dir.path())
            .env("VEGA_POST_HOST", "puck")
            .env("VEGA_NATS_URL", &url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        c.env(
            "VEGA_POST_INBOX_ONLY",
            if inbox_only { "puck" } else { "none" },
        );
        let start = std::time::Instant::now();
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&req).unwrap())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("wait_identity"));
        start.elapsed()
    };
    assert!(run(true) < std::time::Duration::from_secs(2));
}
