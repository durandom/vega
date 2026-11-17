//! The shim against a fake agent-browser, with the shim itself first on PATH
//! (as `activate.sh` puts it): named calls pass through unchanged and record
//! the owner, unnamed calls are refused, exemptions pass.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const SHIM: &str = env!("CARGO_BIN_EXE_agent-browser");

struct Fix {
    /// Keeps the fixture directory alive as long as the Fix lives.
    _keep: tempfile::TempDir,
    tmp: PathBuf,
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn fix() -> Fix {
    let keep = tempfile::tempdir().expect("tempdir");
    let tmp = keep.path().to_path_buf();
    let f = Fix { _keep: keep, tmp };
    // The fake the shim must resolve: after its own directory on PATH.
    let fake = f.tmp.join("bin");
    let shims = f.tmp.join("shims");
    fs::create_dir_all(&fake).unwrap();
    fs::create_dir_all(&shims).unwrap();
    // A non-git cwd, so the recorded project does not depend on the checkout.
    fs::create_dir_all(f.tmp.join("proj")).unwrap();
    script(
        &fake.join("agent-browser"),
        r#"printf '%s\n' "$*" >> "$VEGA_FIX/ab-argv"; echo ok"#,
    );
    // The shim under test, as installed by activate.sh's PATH order: a
    // symlink canonicalizes to the running binary, so the shim skips it
    // instead of exec-ing itself again (a copy would be a second shim).
    std::os::unix::fs::symlink(SHIM, shims.join("agent-browser")).unwrap();
    f
}

fn run(f: &Fix, args: &[&str], env: &[(&str, String)]) -> Output {
    let mut cmd = Command::new(SHIM);
    cmd.args(args)
        .current_dir(f.tmp.join("proj"))
        .env(
            "PATH",
            format!(
                "{}:{}:/usr/bin:/bin",
                f.tmp.join("shims").display(),
                f.tmp.join("bin").display()
            ),
        )
        .env("VEGA_FIX", &f.tmp)
        .env("XDG_STATE_HOME", f.tmp.join("state"))
        .env("HOME", f.tmp.join("home"))
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("VEGA_AGENT_BROWSER_BIN")
        .env_remove("AGENT_BROWSER_SESSION")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HC_TERMINAL_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("PI_CODING_AGENT");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn records(f: &Fix) -> Value {
    fs::read_to_string(f.tmp.join("state/vega/browser/sessions.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| Value::Object(Default::default()))
}

fn argv(f: &Fix) -> Vec<String> {
    fs::read_to_string(f.tmp.join("ab-argv"))
        .map(|t| t.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn named_headless_calls_pass_through_unchanged_and_headed_ones_carry_the_mark() {
    let f = fix();
    let o = run(
        &f,
        &["--session", "vega-shim-test", "get", "url"],
        &[("HERDR_PANE_ID", "w1:p2".into())],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert_eq!(out(&o).trim(), "ok");
    // A headless call (no --headed, no env) adds nothing to argv.
    assert_eq!(argv(&f), ["--session vega-shim-test get url"]);
    let rec = &records(&f)["vega-shim-test"];
    assert_eq!(rec["pane"], "w1:p2");
    assert_eq!(rec["project"], "proj");
    assert!(rec["first_seen"].as_i64().unwrap() > 0);

    // An explicit --headed flag marks: the mark flag rides along first.
    let o = run(
        &f,
        &[
            "--session",
            "vega-shim-test",
            "--headed",
            "open",
            "https://x",
        ],
        &[],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    let mark = f.tmp.join("state/vega/browser/marks/vega-shim-test.js");
    assert!(mark.is_file(), "the mark script is written");
    assert_eq!(
        argv(&f).last().unwrap(),
        &format!(
            "--init-script {} --session vega-shim-test --headed open https://x",
            mark.display()
        )
    );

    // AGENT_BROWSER_HEADED marks a flagless call too.
    fs::remove_file(f.tmp.join("ab-argv")).ok();
    let o = run(
        &f,
        &["--session", "vega-shim-test", "open", "https://y"],
        &[("AGENT_BROWSER_HEADED", "1".into())],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert!(argv(&f).last().unwrap().starts_with("--init-script "));

    // VEGA_BROWSER_MARK=0 opts out even of a headed call.
    fs::remove_file(f.tmp.join("ab-argv")).ok();
    let o = run(
        &f,
        &[
            "--session",
            "vega-shim-test",
            "--headed",
            "open",
            "https://z",
        ],
        &[("VEGA_BROWSER_MARK", "0".into())],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert_eq!(
        argv(&f).last().unwrap(),
        "--session vega-shim-test --headed open https://z"
    );
}

#[test]
fn the_environment_names_a_session_too() {
    let f = fix();
    let o = run(
        &f,
        &["snapshot", "-i"],
        &[("AGENT_BROWSER_SESSION", "tile-clip-42".into())],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert_eq!(argv(&f), ["snapshot -i"]);
    assert_eq!(records(&f)["tile-clip-42"]["pane"], Value::Null);
    // ... but an empty value does not (spellkave sets it per script).
    let f2 = fix();
    let o = run(
        &f2,
        &["snapshot", "-i"],
        &[("AGENT_BROWSER_SESSION", "".into())],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(argv(&f2).is_empty());
}

#[test]
fn an_unnamed_call_is_refused_with_the_hint() {
    let f = fix();
    for args in [
        vec!["open", "https://example.com"],
        vec!["close"],
        vec!["close", "--all"],
        vec!["--json", "snapshot", "-i"],
        vec!["--session="],
        vec!["--session"],
        vec!["--session", "--json", "get", "url"],
    ] {
        let refs: Vec<&str> = args.to_vec();
        let o = run(&f, &refs, &[]);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        let e = err(&o);
        assert!(
            e.contains("agent-browser: name your session: --session <name> or AGENT_BROWSER_SESSION=<name>")
                && e.contains("see `vega skills browser`"),
            "{args:?}: {e}"
        );
        assert!(out(&o).is_empty(), "{args:?}: {}", out(&o));
    }
    assert!(argv(&f).is_empty());
    assert!(records(&f).as_object().unwrap().is_empty());
}

#[test]
fn help_guides_and_session_listing_pass_without_a_name() {
    let f = fix();
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["skills"],
        vec!["skills", "get", "core"],
        vec!["session", "list"],
        vec!["--json", "session", "list"],
    ] {
        let refs: Vec<&str> = args.to_vec();
        let o = run(&f, &refs, &[]);
        assert!(o.status.success(), "{args:?}: {}{}", out(&o), err(&o));
        assert!(!argv(&f).is_empty(), "{args:?}: the real binary ran");
        assert!(err(&o).is_empty(), "{args:?}: no stderr of its own");
    }
    // Read-only calls with no session to own: no record was written.
    assert!(records(&f).as_object().unwrap().is_empty());
}

#[test]
fn named_calls_keep_every_flag_and_subcommand() {
    // parallax attaches to its own Chromium, spellkave passes configs.
    let f = fix();
    for args in [
        vec!["--session", "px", "connect", "9333"],
        vec![
            "--session",
            "px",
            "--headed",
            "--restore",
            "open",
            "https://x",
        ],
        vec![
            "--session",
            "sk",
            "--config",
            "/tmp/browser.json",
            "open",
            "https://y",
        ],
        vec!["--session", "sk", "--cdp", "9224", "get", "url"],
    ] {
        let refs: Vec<&str> = args.to_vec();
        let o = run(&f, &refs, &[]);
        assert!(o.status.success(), "{args:?}");
        // Every flag and subcommand survives; only the --headed call carries
        // the mark flag in front.
        let line = argv(&f).last().unwrap().clone();
        assert!(line.ends_with(&refs.join(" ")), "{line}");
        if refs.contains(&"--headed") {
            assert!(line.starts_with("--init-script "), "{line}");
        } else {
            assert_eq!(
                line,
                refs.join(" "),
                "headless and attach calls stay unchanged"
            );
        }
        fs::remove_file(f.tmp.join("ab-argv")).ok();
    }
    assert_eq!(records(&f).as_object().unwrap().len(), 2);
}

#[test]
fn upsert_keeps_first_seen() {
    let f = fix();
    run(&f, &["--session", "me", "get", "url"], &[]);
    let first = records(&f)["me"]["first_seen"].as_i64().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    run(&f, &["--session", "me", "get", "url"], &[]);
    let rec = &records(&f)["me"];
    assert_eq!(rec["first_seen"].as_i64().unwrap(), first);
    assert!(rec["last_seen"].as_i64().unwrap() > first);
}

#[test]
fn a_session_key_owner_is_not_a_pane() {
    // A codex thread outside a pane owns its session, but records no pane.
    let f = fix();
    let o = run(
        &f,
        &["--session", "hand-started", "get", "url"],
        &[("CODEX_THREAD_ID", "t123".into())],
    );
    assert!(o.status.success());
    let rec = &records(&f)["hand-started"];
    assert_eq!(rec["pane"], Value::Null);
    assert!(!rec["host"].as_str().unwrap().is_empty());
}
