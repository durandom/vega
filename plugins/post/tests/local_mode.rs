//! vegad without a broker (docs/design/vegad.md, Local mode): it stays up,
//! keeps the upstream check running and types the upgrade proposal into the
//! vega session itself. Temp HOME, a fake herdr, a closed broker port.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Kills the relay when the test ends, pass or fail.
struct Relay(Child);

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const FAKE_HERDR: &str = r#"#!/bin/sh
case "$1 $2" in
  "api schema") echo '{"error":{"code":"unknown","message":"no socket api"}}'; exit 1 ;;
  "agent list") echo '{"result":{"agents":[{"pane_id":"w1:p1","agent_status":"idle","agent":"claude"}]}}' ;;
  "pane list") printf '{"result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","cwd":"%s","agent":"claude"}]}}\n' "$FAKE_CHECKOUT" ;;
  "agent prompt") printf '%s %s\n' "$3" "$4" >> "$FAKE_LOG/prompts"; echo '{"result":{}}' ;;
  *) echo '{"result":{}}' ;;
esac
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn wait_for(what: &str, within: Duration, mut f: impl FnMut() -> Option<String>) -> String {
    let until = Instant::now() + within;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn env(cmd: &mut Command, home: &Path, bin: &Path, checkout: &Path) {
    let git_dir = Command::new("sh")
        .args(["-c", "dirname \"$(command -v git)\""])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    cmd.env_clear()
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("PATH", format!("{}:{git_dir}:/usr/bin:/bin", bin.display()))
        // A configured broker nobody listens on: refused at once.
        .env("VEGA_NATS_URL", "nats://127.0.0.1:9")
        .env("VEGA_POST_HOST", "testhost")
        .env("FAKE_LOG", home)
        .env("FAKE_CHECKOUT", checkout);
}

#[test]
fn without_a_broker_vegad_stays_up_and_types_the_upgrade_proposal() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let herdr = bin.join("herdr");
    fs::write(&herdr, FAKE_HERDR).unwrap();
    fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();

    // The checkout: installed at A, its upstream at B (one commit ahead).
    let checkout: PathBuf = home.join("src/vega");
    fs::create_dir_all(&checkout).unwrap();
    git(&checkout, &["init", "-q"]);
    git(&checkout, &["commit", "-q", "--allow-empty", "-m", "a"]);
    let a = git(&checkout, &["rev-parse", "HEAD"]);
    git(&checkout, &["commit", "-q", "--allow-empty", "-m", "b"]);
    let b = git(&checkout, &["rev-parse", "HEAD"]);

    let state = home.join("state/vega");
    fs::create_dir_all(&state).unwrap();
    // Local mode is available after the one-time identity cutover completed.
    let cutover = state.join("identity-cutover");
    fs::create_dir_all(&cutover).unwrap();
    let plan = serde_json::json!({"schema":2,"host":"testhost","inbox_only":false,"mappings":[],"original":{},"writes":{},"remove":[],"registry":{"host":"testhost","backup":{},"projection":{}},"registry_reads":[],"owned":{}});
    fs::write(cutover.join("journal.json"), plan.to_string()).unwrap();
    fs::write(
        cutover.join("approved.json"),
        serde_json::json!({"schema":2,"hosts":[plan],"grants":{}}).to_string(),
    )
    .unwrap();
    fs::write(
        cutover.join("complete.json"),
        r#"{"schema":2,"host":"testhost"}"#,
    )
    .unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    fs::write(
        state.join("install.json"),
        format!(
            r#"{{"checkout":"{}","commit":"{a}","by":"local","at":{now}}}"#,
            checkout.display()
        ),
    )
    .unwrap();
    // Fetched just now: the thread re-counts with local git, no fetch.
    fs::write(
        state.join("upgrade.json"),
        format!(r#"{{"fetched_at":{now},"upstream":"{b}","upstream_name":"origin/main","behind":1,"changes":[]}}"#),
    )
    .unwrap();

    let exe = env!("CARGO_BIN_EXE_vega-post");
    let mut cmd = Command::new(exe);
    let out = fs::File::create(home.join("relay.log")).unwrap();
    cmd.arg("relay").stdout(out).stderr(Stdio::null());
    env(&mut cmd, &home, &bin, &checkout);
    let mut relay = Relay(cmd.spawn().unwrap());

    let prompts = home.join("prompts");
    let typed = wait_for("the upgrade proposal", Duration::from_secs(30), || {
        fs::read_to_string(&prompts)
            .ok()
            .filter(|t| t.contains("vega upgrade available"))
    });
    assert!(relay.0.try_wait().unwrap().is_none(), "vegad exited");
    let line = typed
        .lines()
        .find(|l| l.contains("vega upgrade available"))
        .unwrap();
    assert!(
        line.starts_with("w1:p1 [tool vega-upgrade] vega upgrade available: 1 commit"),
        "{line}"
    );
    assert!(line.contains("Ask the user"), "{line}");
    assert!(!line.contains('\n'));
    // Proposed once per upstream state.
    let check = fs::read_to_string(state.join("upgrade.json")).unwrap();
    assert!(check.contains(&format!(r#""proposed": "{b}""#)), "{check}");

    let mut st = Command::new(exe);
    st.args(["relay", "status"]);
    env(&mut st, &home, &bin, &checkout);
    let out = String::from_utf8_lossy(&st.output().unwrap().stdout).into_owned();
    assert!(out.contains("vegad: running"), "{out}");
    assert!(
        out.contains("broker: local (broker nats://127.0.0.1:9 unreachable)"),
        "{out}"
    );
    assert!(
        out.contains("off: mail relay, registry, commands, beads sync, ci verdict mail"),
        "{out}"
    );

    let log = fs::read_to_string(home.join("relay.log")).unwrap_or_default();
    // Sample output with `--nocapture`.
    eprintln!("{out}---\n{log}---\n{typed}");
    assert!(
        log.contains("local: tool vega-upgrade mail typed into w1:p1"),
        "{log}"
    );
}
