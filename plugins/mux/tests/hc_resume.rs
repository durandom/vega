//! After a reboot hc restores kept terminals with a shell: reopen and the
//! vegad boot pass resume claude, codex and pi in place (vega-0h4c.72).
//! One test: the in-process boot pass sets process-wide env.
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn mux(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_vega-mux"));
    c.env_clear()
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .env("HOME", root)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("HC_TERMINAL_ID", "t7")
        .env("TEST_PARENT_PID", std::process::id().to_string())
        .env("VEGA_POST_HOST", "test")
        .env("VEGA_TOKEN_BURN", "/nonexistent/token-burn")
        .env("VEGA_NATS_URL", "nats://127.0.0.1:1")
        .current_dir(root);
    c
}

fn ok(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}

fn sends(root: &Path) -> Vec<(String, String)> {
    fs::read_to_string(root.join("calls"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v[0] == "send")
        .map(|v| (v[1].as_str().unwrap().into(), v[2].as_str().unwrap().into()))
        .collect()
}

/// Model the core loss: hc's keep put a login shell into each terminal.
fn lose_agents(root: &Path, addresses: &[&str]) {
    let path = root.join("terminals.json");
    let mut ts: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for a in addresses {
        let t = &mut ts[*a];
        t["keep"] = true.into();
        t["foreground"] =
            serde_json::json!({"argv":["-zsh"],"name":"zsh","pid":t["pid"],"cwd":"/"});
        t["status"] = serde_json::json!({"state":"idle"});
    }
    fs::write(path, ts.to_string()).unwrap();
}

fn pi_session(root: &Path, cwd: &str, id: &str) {
    let dir = root.join(".pi/agent/sessions").join(format!(
        "--{}--",
        cwd.trim_start_matches('/').replace('/', "-")
    ));
    fs::create_dir_all(&dir).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let (d, s) = (now.div_euclid(86_400), now.rem_euclid(86_400));
    // Civil from days (Howard Hinnant).
    let z = d + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let ts = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    );
    fs::write(
        dir.join(format!("x_{id}.jsonl")),
        format!(
            "{}\n",
            serde_json::json!({"type":"session","version":3,"id":id,"timestamp":ts,"cwd":cwd})
        ),
    )
    .unwrap();
}

#[test]
fn lost_agents_resume_in_place_by_reopen_and_boot_pass() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    for (name, text) in [
        ("renice", "#!/bin/sh\n"),
        ("hc", include_str!("fixtures/hc.py")),
        ("herdr-app", include_str!("fixtures/app.py")),
        (
            "herdr",
            "#!/bin/sh\necho '{\"error\":{\"code\":\"agent_not_found\"}}'\nexit 1\n",
        ),
    ] {
        fs::write(bin.join(name), text).unwrap();
        fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::create_dir_all(root.join("config/herdr-app")).unwrap();
    fs::write(root.join("config/herdr-app/herdr-app.sock"), "").unwrap();
    let cwd = root.display().to_string();

    // t8 claude, t9 codex, t10 pi.
    for (name, kind) in [("wc", "claude"), ("wx", "codex"), ("wp", "pi")] {
        ok(mux(&root).args([
            "spawn", name, "--kind", kind, "--prompt", "task", "--", "--model", "m1",
        ]));
    }
    for (t, key) in [("t8", "CLAUDE_CODE_SESSION_ID"), ("t9", "CODEX_THREAD_ID")] {
        ok(mux(&root)
            .env("HC_TERMINAL_ID", t)
            .env(key, format!("s-{t}"))
            .args(["show", t]));
    }
    pi_session(&root, &cwd, "s-t10");

    // A live agent is no lost agent: reopen refuses as before.
    let o = mux(&root).args(["reopen", "wc"]).output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("agent_running"));

    lose_agents(&root, &["t8", "t9", "t10"]);
    let out = ok(mux(&root).args(["reopen", "wc", "wx", "t10"]));
    assert!(
        out.contains("resumed: t8 (claude, session s-t8) in place"),
        "{out}"
    );
    let typed = sends(&root);
    assert_eq!(typed.len(), 3, "{typed:?}");
    let expect = [
        (
            "t8",
            "claude --dangerously-skip-permissions --model m1 --resume s-t8",
        ),
        ("t9", "resume s-t9"),
        ("t10", "pi --approve --model m1 --session s-t10"),
    ];
    for ((t, line), (et, tail)) in typed.iter().zip(expect) {
        assert_eq!(t, et);
        assert!(line.starts_with(&format!("cd {cwd} && ")), "{line}");
        assert!(line.contains(" HERDR_PANE_ID='' "), "recorded env: {line}");
        assert!(line.ends_with(tail), "{line}");
        assert!(!line.contains("task"), "the brief is not resent: {line}");
    }
    assert!(typed[1].1.contains("codex --no-daemon"), "{}", typed[1].1);

    // The vegad boot pass: offer (default) leaves one note and types nothing.
    std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    std::env::set_var("HOME", &root);
    std::env::set_var("XDG_STATE_HOME", root.join("state"));
    std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
    std::env::set_var("TEST_PARENT_PID", std::process::id().to_string());
    std::env::set_var("VEGA_POST_HOST", "test");
    std::env::remove_var("HC_TERMINAL_ID");
    std::env::set_var("VEGA_BOOT_EPOCH", "boot-1");
    use vega_herdr::resume::{boot_pass, mode, Mode};
    assert_eq!(mode(), Mode::Offer);
    let pass = boot_pass(mode()).unwrap().unwrap();
    assert_eq!(pass.offered, ["t8", "t9", "t10"]);
    assert_eq!(sends(&root).len(), 3);
    let notes = vega_herdr::notes::Store::local().unwrap().list();
    let note = notes.iter().find(|n| n.kind == "resume").unwrap();
    assert_eq!(note.cmd.as_deref(), Some("vega mux reopen t8 t9 t10"));
    assert!(boot_pass(mode()).unwrap().is_none(), "once per boot");

    // auto on the next boot: vegad types the same lines itself.
    fs::write(root.join("config/vega/resume.toml"), "mode = \"auto\"\n")
        .or_else(|_| {
            fs::create_dir_all(root.join("config/vega"))?;
            fs::write(root.join("config/vega/resume.toml"), "mode = \"auto\"\n")
        })
        .unwrap();
    std::env::set_var("VEGA_BOOT_EPOCH", "boot-2");
    let pass = boot_pass(mode()).unwrap().unwrap();
    assert_eq!(pass.resumed, ["t8", "t9", "t10"], "{:?}", pass.failed);
    let typed = sends(&root);
    assert_eq!(typed[3..], typed[..3]);

    // A UAC started by hand: claim records its own terminal (here t7, the
    // fake parent, whose pid is this test process).
    std::env::set_var("HC_TERMINAL_ID", "t7");
    std::env::set_var("CLAUDE_CODE_SESSION_ID", "s-uac");
    let line = vega_herdr::resume::record_own("t7");
    // The fake's t7 is codex without a foreground argv: recorded as codex.
    assert_eq!(
        line.as_deref(),
        Some("resume: recorded (codex, no session yet)")
    );
    std::env::set_var("CODEX_THREAD_ID", "s-uac");
    assert_eq!(
        vega_herdr::resume::record_own("t7").as_deref(),
        Some("resume: recorded (codex, session s-uac)")
    );
    let r = vega_herdr::state::spawn_of_on(None, "t7", "parent", "codex").unwrap();
    assert_eq!(
        (r.backend.as_str(), r.session.as_deref()),
        ("hc", Some("s-uac"))
    );
}
