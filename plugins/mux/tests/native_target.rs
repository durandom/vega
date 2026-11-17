//! Runs on Windows CI and on Unix with --features target-only; no shell or hc server.
#![cfg(any(windows, feature = "target-only"))]
use base64::Engine;
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn command(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_vega-mux"));
    c.env_clear()
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("PATH", root.join("bin"))
        .current_dir(root);
    c
}
fn success(out: Output) -> Value {
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn preparation_resolves_only_native_exes_without_changing_cwd_or_trust() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let cwd = root.join("checkout Grüße $literal with spaces");
    fs::create_dir(&cwd).unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    for kind in ["codex", "claude"] {
        fs::write(root.join(format!("bin/{kind}.cmd")), "not a native binary").unwrap();
        let args = ["prepare", "--kind", kind, "--cwd", cwd.to_str().unwrap()];
        let out = command(root).args(args).output().unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("executable_not_found"));
        let exe = root.join("bin").join(format!("{kind}.exe"));
        // Preparation locates but never starts the executable.
        fs::write(&exe, "must never execute").unwrap();
        let v = success(command(root).args(args).output().unwrap());
        assert_eq!(v["platform"], "windows");
        assert_eq!(v["executable"], exe.to_str().unwrap());
        assert_eq!(v["cwd"], cwd.to_str().unwrap());
        assert_eq!(v["trust"], Value::Null);
    }
    assert!(!root.join(".claude.json").exists());
    assert!(!root.join(".codex/config.toml").exists());
    let out = command(root)
        .args([
            "prepare",
            "--kind",
            "codex",
            "--cwd",
            cwd.to_str().unwrap(),
            "--worktree",
            "branch",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Windows targets do not support --worktree")
    );
    for path in ["relative", "G:relative", "missing"] {
        let out = command(root)
            .args(["prepare", "--kind", "codex", "--cwd", path])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
    }
}
#[test]
fn records_preserve_coordinator_policy_lineage_and_replace_stale_state() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let record = vega_herdr::state::SpawnRecord {
        backend: "hc".into(),
        pane: "t8".into(),
        name: "worker".into(),
        kind: "codex".into(),
        cwd: root.to_str().unwrap().into(),
        mode: "headed".into(),
        parent: Some(vega_herdr::state::Parent {
            pane: "t7".into(),
            host: "proteus".into(),
        }),
        tier: Some("balanced".into()),
        model: Some("chosen-by-coordinator".into()),
        tier_why: Some("explicit reason".into()),
        bead: Some("vega-nic.28.7".into()),
        argv: vec![
            r"C:\Program Files\Codex\codex.exe".into(),
            "--model".into(),
            "chosen-by-coordinator".into(),
        ],
        env: vec!["SPECIAL=a b $literal Grüße".into()],
        task: Some("implement".into()),
        started_ms: Some(1234),
        ..Default::default()
    };
    let mut request = json!({"schema":1,"record":record,"undelivered":"saved brief"});
    let run = |v: &Value| {
        command(root)
            .args([
                "record-spawn",
                &base64::engine::general_purpose::STANDARD.encode(v.to_string()),
            ])
            .output()
            .unwrap()
    };
    let read = |kind: &str| {
        fs::read_to_string(root.join(format!("state/vega/{kind}/local_hc-7438"))).unwrap()
    };
    assert_eq!(success(run(&request)), request);
    assert_eq!(
        serde_json::from_str::<Value>(&read("spawns")).unwrap(),
        request["record"]
    );
    assert_eq!(
        serde_json::from_str::<Value>(&read("parents")).unwrap()["host"],
        "proteus"
    );
    assert!(read("undelivered").contains("saved brief"));
    assert_eq!(read("briefs"), "1234");
    // Repeated writes must atomically replace files on Windows too.
    request["record"]["parent"] = Value::Null;
    request["record"]["task"] = Value::Null;
    request["record"]["mode"] = "headless".into();
    request["undelivered"] = Value::Null;
    assert_eq!(success(run(&request)), request);
    for kind in ["parents", "tasks", "undelivered"] {
        assert!(!root
            .join(format!("state/vega/{kind}/local_hc-7438"))
            .exists());
    }
    for (key, value) in [
        ("pane", json!("remote/t8")),
        ("cwd", json!("G:relative")),
        ("kind", json!("pi")),
        ("backend", json!("herdr")),
    ] {
        let mut invalid = request.clone();
        invalid["record"][key] = value;
        assert!(!run(&invalid).status.success());
    }
    assert_eq!(
        serde_json::from_str::<Value>(&read("spawns")).unwrap(),
        request["record"]
    );
    // A failed write is reported, not echoed as a confirmed record.
    let path = root.join("state/vega/spawns/local_hc-7438");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let out = run(&request);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("spawn_record_write_failed"));
}
