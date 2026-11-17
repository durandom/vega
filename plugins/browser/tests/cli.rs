//! `vega browser` against fakes: a fake agent-browser (`VEGA_AGENT_BROWSER_BIN`),
//! a fake `ps`, `herdr` and `hc` on PATH, a fake Chromium that starts the
//! in-test DevTools responder, and temp homes. Nothing real is started.

use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use vega_browser::mark;

const BIN: &str = env!("CARGO_BIN_EXE_vega-browser");

struct Fix {
    /// Keeps the fixture directory alive as long as the Fix lives.
    _keep: tempfile::TempDir,
    tmp: PathBuf,
    bin: PathBuf,
    state: PathBuf,
    home: PathBuf,
    port: u16,
}

impl Fix {
    fn path(&self, name: &str) -> PathBuf {
        self.tmp.join(name)
    }
}

impl Drop for Fix {
    fn drop(&mut self) {
        if let Ok(pids) = fs::read_to_string(self.path("responders")) {
            for pid in pids.lines().filter_map(|l| l.trim().parse::<u32>().ok()) {
                let _ = Command::new("kill").arg(pid.to_string()).status();
            }
        }
    }
}

/// A free localhost port (briefly unbound; tests rebind it or leave it free).
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("a free port")
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A DevTools endpoint answering in-process until the process ends; with
/// `<fix>/hang` set it closes connections instead (probe sees Held).
fn serve(f: &Fix, port: u16) {
    let hang = f.path("hang");
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("responder binds");
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            if !hang.exists() {
                let _ = stream.write_all(
                    b"HTTP/1.0 200 OK\r\nContent-Length: 17\r\n\r\n{\"Browser\":\"x\"}\n",
                );
                let _ = stream.flush();
            }
            drop(stream);
        }
    });
}

fn fix() -> Fix {
    let keep = tempfile::tempdir().expect("tempdir");
    let tmp = keep.path().to_path_buf();
    let f = Fix {
        _keep: keep,
        bin: tmp.join("bin"),
        state: tmp.join("state"),
        home: tmp.join("home"),
        tmp,
        port: free_port(),
    };
    for dir in [&f.bin, &f.state, &f.home, &f.tmp.join("tmp")] {
        fs::create_dir_all(dir).unwrap();
    }
    // agent-browser: records argv and env, answers the subcommands vega uses.
    script(
        &f.bin.join("agent-browser"),
        r#"
printf '%s\n' "$*" >> "$VEGA_FIX/ab-argv"
env | grep '^AGENT_BROWSER_' | sort >> "$VEGA_FIX/ab-env"
[ -e "$VEGA_FIX/ab-hang" ] && : > "$VEGA_FIX/hang"
[ -e "$VEGA_FIX/ab-fail" ] && { cat "$VEGA_FIX/ab-fail" >&2; exit 1; }
case " $* " in
  *" session list "*)
    echo "Active sessions:"
    [ -e "$VEGA_FIX/sessions" ] && cat "$VEGA_FIX/sessions"
    ;;
  *" get url "*) cat "$VEGA_FIX/get-url" 2>/dev/null || echo "https://example.com/x" ;;
  *" connect "*) echo "connected" ;;
  *" close "*)
    for p in $(cat "$VEGA_FIX/responders" 2>/dev/null); do kill "$p" 2>/dev/null; done
    : > "$VEGA_FIX/ps"
    echo "closed"
    ;;
  *) echo "ok" ;;
esac
"#,
    );
    // ps: the table under test, from a file.
    script(&f.bin.join("ps"), r#"cat "$VEGA_FIX/ps" 2>/dev/null"#);
    // herdr: pane list from a file (`{"pane_id":"w1:p2"}` lines).
    script(
        &f.bin.join("herdr"),
        r#"
case "$*" in
  "pane list") printf '{"result":{"panes":[%s]}}\n' "$(cat "$VEGA_FIX/herdr-panes" 2>/dev/null)" ;;
  *) exit 9 ;;
esac
"#,
    );
    // hc: one JSON-RPC reply per stdin line, by terminal id.
    script(
        &f.bin.join("hc"),
        r#"
IFS= read -r req
case "$req" in
  *'"terminal":"t1"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"address":"t1","state":"live"}}\n' ;;
  *'"terminal":"t9"'*) printf '{"jsonrpc":"2.0","id":1,"error":{"message":"no terminal t9","data":{"kind":"not_found"}}}\n' ;;
  *) printf '{"jsonrpc":"2.0","id":1,"error":{"message":"boom","data":{"kind":"internal"}}}\n' ;;
esac
"#,
    );
    // chromium: records its argv, starts the DevTools responder, reports the
    // process in the ps table.
    script(
        &f.bin.join("chromium"),
        r#"
printf '%s\n' "$*" > "$VEGA_FIX/chromium-argv"
[ -e "$VEGA_FIX/dead" ] && exit 0
port=$(printf '%s\n' "$*" | tr ' ' '\n' | sed -n 's/^--remote-debugging-port=//p')
prof=$(printf '%s\n' "$*" | tr ' ' '\n' | sed -n 's/^--user-data-dir=//p')
mkdir -p "$VEGA_FIX/fake-profile"
printf ' %s 1:00 %s\n' "4242" "$(printf '%s' "$*" | sed "s#--user-data-dir=[^ ]*#--user-data-dir=$prof#")" > "$VEGA_FIX/ps"
python3 "$VEGA_FIX/responder.py" "$port" "$VEGA_FIX" &
"#,
    );
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/responder.py"),
        f.path("responder.py"),
    )
    .unwrap();
    f
}

/// The process line of our global Chromium for the ps table.
fn ours_line(f: &Fix) -> String {
    format!(
        " 4242 1:00 Chromium --remote-debugging-port={} --user-data-dir={}",
        f.port,
        f.home.join(".local/share/vega-chromium").display()
    )
}

fn herdr_panes(f: &Fix, panes: &str) {
    fs::write(f.path("herdr-panes"), panes).unwrap();
}

fn hostname() -> String {
    String::from_utf8_lossy(&Command::new("hostname").arg("-s").output().unwrap().stdout)
        .trim()
        .to_owned()
}

/// A record in the documented shape, written straight to the sessions file.
fn record(f: &Fix, session: &str, pane: Option<&str>, host: &str, first: i64, last: i64) {
    let file = f.state.join("vega/browser/sessions.json");
    let mut all: Value = fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| Value::Object(Default::default()));
    all[session] = serde_json::json!({
        "pane": pane, "host": host, "project": "vega-browser",
        "cwd": "/nowhere", "first_seen": first, "last_seen": last,
    });
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, format!("{all}\n")).unwrap();
}

fn records(f: &Fix) -> Value {
    fs::read_to_string(f.state.join("vega/browser/sessions.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| Value::Object(Default::default()))
}

fn run(f: &Fix, args: &[&str], env: &[(&str, String)]) -> Output {
    // A non-git cwd, so the recorded project does not depend on the checkout.
    let proj = f.tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(&proj)
        .env("PATH", format!("{}:/usr/bin:/bin", f.bin.display()))
        .env("XDG_STATE_HOME", &f.state)
        .env("HOME", &f.home)
        .env_remove("XDG_CONFIG_HOME")
        .env("TMPDIR", f.tmp.join("tmp"))
        .env("VEGA_FIX", &f.tmp)
        .env("CDP_PORT", f.port.to_string())
        .env("CDP_CHROMIUM", f.bin.join("chromium"))
        .env("VEGA_AGENT_BROWSER_BIN", f.bin.join("agent-browser"))
        .env_remove("CDP_PROFILE")
        .env_remove("AGENT_BROWSER_SESSION")
        .env_remove("AGENT_BROWSER_AUTO_CONNECT")
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

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn lines_of(f: &Fix, name: &str) -> Vec<String> {
    fs::read_to_string(f.path(name))
        .map(|t| t.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

#[test]
fn chromium_attaches_to_the_answering_global() {
    let f = fix();
    serve(&f, f.port);
    fs::write(f.path("ps"), ours_line(&f)).unwrap();
    let o = run(&f, &["chromium"], &[]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert_eq!(
        out(&o).trim(),
        format!(
            "attached: 127.0.0.1:{}, profile {}",
            f.port,
            f.home.join(".local/share/vega-chromium").display()
        )
    );
    assert!(!f.path("chromium-argv").exists(), "must not start Chromium");
}

#[test]
fn chromium_starts_the_global_when_it_is_down() {
    let f = fix();
    let o = run(&f, &["chromium", "https://example.com"], &[]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    let profile = f.home.join(".local/share/vega-chromium");
    assert_eq!(
        out(&o).trim(),
        format!(
            "started: 127.0.0.1:{}, profile {}",
            f.port,
            profile.display()
        )
    );
    let argv = fs::read_to_string(f.path("chromium-argv")).unwrap();
    assert_eq!(
        argv.trim(),
        format!(
            "--remote-debugging-port={} --remote-debugging-address=127.0.0.1 \
             --user-data-dir={} --no-first-run --no-default-browser-check https://example.com",
            f.port,
            profile.display()
        )
    );
    assert!(profile.is_dir(), "profile created");
}

#[test]
fn chromium_refuses_foreign_and_silent_listeners() {
    // Answers, but not our Chromium.
    let f = fix();
    serve(&f, f.port);
    fs::write(f.path("ps"), "").unwrap();
    let o = run(&f, &["chromium"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("not from our Chromium"), "{}", out(&o));
    assert!(!f.path("chromium-argv").exists());

    // Something holds the port that does not speak CDP: bind and say nothing.
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let held_port = held.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in held.incoming() {
            drop(stream);
        }
    });
    let f2 = fix();
    thread::sleep(Duration::from_millis(100));
    let o = run(&f2, &["chromium"], &[("CDP_PORT", held_port.to_string())]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("does not speak CDP"), "{}", out(&o));
    assert!(!f2.path("chromium-argv").exists());
}

#[test]
fn chromium_fails_loudly_when_it_does_not_come_up() {
    let f = fix();
    fs::write(f.path("dead"), "").unwrap();
    let o = run(&f, &["chromium"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("did not open CDP"), "{}", out(&o));
}

#[test]
fn ab_attaches_passes_through_and_records_the_owner() {
    let f = fix();
    serve(&f, f.port);
    fs::write(f.path("ps"), ours_line(&f)).unwrap();
    herdr_panes(&f, r#"{"pane_id":"w1:p2"}"#);
    let o = run(
        &f,
        &["ab", "--session", "me", "get", "url"],
        &[
            ("HERDR_PANE_ID", "w1:p2".into()),
            ("AGENT_BROWSER_AUTO_CONNECT", "1".into()),
        ],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    // The fake answers `get url` with a URL.
    assert_eq!(out(&o).trim(), "https://example.com/x");
    // A call without --headed (attaching to the global Chromium) is unchanged.
    assert_eq!(
        lines_of(&f, "ab-argv"),
        [format!("--cdp {} --session me get url", f.port)]
    );
    // Browser-picking env never reaches agent-browser.
    assert!(!fs::read_to_string(f.path("ab-env"))
        .unwrap()
        .contains("AUTO_CONNECT"));
    let me = &records(&f)["me"];
    assert_eq!(me["pane"], "w1:p2");
    assert_eq!(me["host"], hostname());
    assert_eq!(me["project"], "proj");
    assert!(me["first_seen"].as_i64().unwrap() > 0);
    assert_eq!(me["first_seen"], me["last_seen"]);

    // An explicitly headed ab call carries the window mark; the opt-out wins.
    let o = run(
        &f,
        &["ab", "--session", "me", "--headed", "open", "https://x"],
        &[],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    let mark = f.state.join("vega/browser/marks/me.js");
    assert!(mark.is_file(), "the mark script is written");
    assert!(fs::read_to_string(&mark)
        .unwrap()
        .contains("DOMContentLoaded"));
    assert_eq!(
        lines_of(&f, "ab-argv").last().unwrap(),
        &format!(
            "--cdp {} --init-script {} --session me --headed open https://x",
            f.port,
            mark.display()
        )
    );
    let o = run(
        &f,
        &["ab", "--session", "me", "--headed", "open", "https://y"],
        &[("VEGA_BROWSER_MARK", "0".into())],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert_eq!(
        lines_of(&f, "ab-argv").last().unwrap(),
        &format!("--cdp {} --session me --headed open https://y", f.port)
    );
}

#[test]
fn ab_refuses_unnamed_calls_and_browser_pickers() {
    let f = fix();
    serve(&f, f.port);
    fs::write(f.path("ps"), ours_line(&f)).unwrap();
    let o = run(&f, &["ab", "get", "url"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("name your session"), "{}", out(&o));
    assert!(lines_of(&f, "ab-argv").is_empty());

    for args in [
        vec!["ab", "--session", "me", "connect", "9333"],
        vec!["ab", "--session", "me", "--cdp", "9999", "get", "url"],
        vec!["ab", "--session", "me", "--cdp=9999", "get", "url"],
        vec!["ab", "--session", "me", "--auto-connect", "get", "url"],
        vec!["ab", "--session", "me", "-p", "kernel", "open", "x"],
        vec!["ab", "--session", "me", "--provider=kernel", "open", "x"],
        vec![
            "ab",
            "--session",
            "me",
            "--executable-path",
            "/x",
            "open",
            "y",
        ],
    ] {
        let refs: Vec<&str> = args.to_vec();
        let o = run(&f, &refs, &[]);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert!(out(&o).contains("not allowed"), "{args:?}: {}", out(&o));
    }
    assert!(lines_of(&f, "ab-argv").is_empty());
    assert!(records(&f).as_object().unwrap().is_empty());
}

#[test]
fn ab_never_runs_without_the_global_and_reports_a_lost_one() {
    // Down: refuses before agent-browser runs.
    let f = fix();
    fs::write(f.path("ps"), "").unwrap();
    let o = run(
        &f,
        &["ab", "--session", "me", "open", "https://x"],
        &[("CDP_CHROMIUM", "/nonexistent/chromium".into())],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("no fallback browser"), "{}", out(&o));
    assert!(lines_of(&f, "ab-argv").is_empty());

    // Lost mid-call: agent-browser fails and CDP goes dark.
    let f2 = fix();
    serve(&f2, f2.port);
    fs::write(f2.path("ps"), ours_line(&f2)).unwrap();
    fs::write(f2.path("ab-fail"), "boom").unwrap();
    fs::write(f2.path("ab-hang"), "").unwrap();
    let o = run(&f2, &["ab", "--session", "me", "get", "url"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("lost the global Chromium"), "{}", err(&o));
}

#[test]
fn launch_runs_a_throwaway_and_cleanup_closes_it() {
    let f = fix();
    let port = free_port();
    herdr_panes(&f, r#"{"pane_id":"w1:p2"}"#);
    let o = run(
        &f,
        &[
            "launch",
            "https://example.com/deep/path",
            "--session",
            "fb",
            "--port",
            &port.to_string(),
        ],
        &[("HERDR_PANE_ID", "w1:p2".into())],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    let profile = f.tmp.join("tmp/agent-browser-fb-profile");
    assert_eq!(
        out(&o).trim(),
        format!("session: fb\nport: {port}\nprofile: {}", profile.display())
    );
    let argv = fs::read_to_string(f.path("chromium-argv")).unwrap();
    assert!(
        argv.contains(&format!("--remote-debugging-port={port}")),
        "{argv}"
    );
    assert!(
        argv.contains(&format!("--user-data-dir={}", profile.display())),
        "{argv}"
    );
    assert!(
        argv.contains("--no-first-run https://example.com/deep/path"),
        "{argv}"
    );
    assert!(lines_of(&f, "ab-argv").contains(&format!("--session fb connect {port}")));
    assert!(records(&f)["fb"]["pane"] == "w1:p2");

    let o = run(
        &f,
        &["cleanup", "--session", "fb", "--port", &port.to_string()],
        &[],
    );
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert_eq!(
        out(&o).trim(),
        format!("closed: port {port}, profile deleted")
    );
    assert!(lines_of(&f, "ab-argv").contains(&"--session fb close".to_string()));
    assert!(!profile.exists(), "temp profile deleted");
    assert!(records(&f).get("fb").is_none(), "record removed");
}

#[test]
fn launch_refuses_the_global_port_and_bad_session_names() {
    let f = fix();
    let o = run(
        &f,
        &["launch", "https://x", "--port", &f.port.to_string()],
        &[],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("is the global browser"), "{}", out(&o));

    let o = run(&f, &["launch", "https://x", "--session", "../x"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("bad session name"), "{}", out(&o));
    assert!(!f.path("chromium-argv").exists());
}

#[test]
fn cleanup_refuses_the_global_port_and_profile_first() {
    let f = fix();
    let o = run(&f, &["cleanup", "--port", &f.port.to_string()], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("is the global browser"), "{}", out(&o));

    let global_profile = f.tmp.join("tmp/agent-browser-x-profile");
    fs::create_dir_all(&global_profile).unwrap();
    let o = run(
        &f,
        &["cleanup", "--session", "x"],
        &[("CDP_PROFILE", global_profile.display().to_string())],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("is the global profile"), "{}", out(&o));
    assert!(lines_of(&f, "ab-argv").is_empty(), "nothing was touched");
    assert!(global_profile.is_dir());
}

#[test]
fn ls_merges_sessions_records_and_processes() {
    let f = fix();
    let host = hostname();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    fs::write(f.path("sessions"), "s-live\ns-stale-idle\ns-un\n").unwrap();
    herdr_panes(&f, r#"{"pane_id":"w1:p2"},{"pane_id":"t1"}"#);
    record(&f, "s-live", Some("w1:p2"), &host, now - 60, now - 30);
    record(
        &f,
        "s-stale-pane",
        Some("w1:p9"),
        &host,
        now - 7200,
        now - 60,
    );
    record(
        &f,
        "s-stale-idle",
        Some("w1:p2"),
        &host,
        now - 3 * 86400,
        now - 2 * 86400,
    );
    record(&f, "s-hc-live", Some("t1"), &host, now - 60, now - 60);
    record(&f, "s-gone", None, &host, now - 60, now - 60);
    fs::write(
        f.path("ps"),
        format!(
            "{}\n 4243 2:00 /tmp/cft/Chrome for Testing --headless --remote-debugging-port=0 --user-data-dir=/tmp/ab\n",
            ours_line(&f)
        ),
    )
    .unwrap();

    // `now` inside the binary differs from the test's; only relative output
    // is asserted (age/idle of the fresh record, the states, the browsers).
    let o = run(&f, &["ls"], &[]);
    assert!(o.status.success(), "{}", out(&o));
    let text = out(&o);
    assert!(
        text.contains("sessions[6]{session,state,pane,host,project,age,idle}:"),
        "{text}"
    );
    let live = text
        .lines()
        .find(|l| l.starts_with("  s-live,"))
        .unwrap_or_else(|| panic!("{text}"));
    assert_eq!(
        live.split(',').take(5).collect::<Vec<_>>(),
        ["  s-live", "live", "w1:p2", host.as_str(), "vega-browser"]
    );
    for (session, state) in [
        ("s-stale-pane", "stale"),
        ("s-stale-idle", "stale"),
        ("s-hc-live", "live"),
        ("s-un", "unowned"),
        ("s-gone", "closed"),
    ] {
        assert!(
            text.lines()
                .any(|l| l.starts_with(&format!("  {session},")) && l.contains(state)),
            "{text}"
        );
    }
    assert!(
        text.contains("browsers[2]{pid,kind,headed,age,global}:"),
        "{text}"
    );
    assert!(text.contains("4242,chromium,headed,1m,global"), "{text}");
    assert!(
        text.contains("4243,chrome-for-testing,headless,2m,"),
        "{text}"
    );

    // The stable JSON shape for phase 2 (vega-nic.19).
    let o = run(&f, &["ls", "--json"], &[]);
    assert!(o.status.success(), "{}", out(&o));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["error"], Value::Null);
    let sessions = v["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 6);
    let live = sessions.iter().find(|s| s["session"] == "s-live").unwrap();
    for (k, want) in [
        ("state", "live"),
        ("pane", "w1:p2"),
        ("host", host.as_str()),
        ("project", "vega-browser"),
    ] {
        assert_eq!(live[k], want, "{live}");
    }
    assert!(live["age"].as_i64().unwrap() >= 60, "{}", live["age"]);
    assert!(live["idle"].as_i64().unwrap() >= 30, "{}", live["idle"]);
    assert!(live["headed"].is_null() && live["pid"].is_null());
    let un = sessions.iter().find(|s| s["session"] == "s-un").unwrap();
    assert_eq!(un["state"], "unowned");
    assert!(un["pane"].is_null() && un["age"].is_null() && un["idle"].is_null());
    let gone = sessions.iter().find(|s| s["session"] == "s-gone").unwrap();
    assert_eq!(gone["state"], "closed");
    let browsers = v["browsers"].as_array().unwrap();
    assert_eq!(browsers.len(), 2);
    let leftover = browsers.iter().find(|b| b["pid"] == 4243).unwrap();
    assert_eq!(leftover["kind"], "chrome-for-testing");
    assert_eq!(leftover["headed"], false);
    assert_eq!(leftover["global"], false);
    let global = browsers.iter().find(|b| b["pid"] == 4242).unwrap();
    assert_eq!(global["global"], true);
}

#[test]
fn ls_json_attributes_session_browsers_and_colours() {
    let f = fix();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    fs::write(f.path("sessions"), "s-own\ns-hid\ns-un\n").unwrap();
    record(&f, "s-own", None, &hostname(), now - 60, now - 60);
    record(&f, "s-hid", None, &hostname(), now - 60, now - 60);
    // An owner project colour from the theme (the record helper writes
    // project "vega-browser"), instead of the hashed fallback.
    let config = f.home.join(".config/vega");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("theme.toml"),
        "[symbols.project_colors]\nvega-browser = \"#123456\"\n",
    )
    .unwrap();
    // agent-browser's own headed launches profile under <session>-profile;
    // helper processes share the main browser's arguments.
    fs::write(
        f.path("ps"),
        format!(
            "{}\n 5001 1:00 cft/chrome --remote-debugging-port=0 --user-data-dir=/ab/s-own-profile\n 5002 1:00 cft/chrome --remote-debugging-port=0 --user-data-dir=/ab/s-own-profile\n 5003 1:00 cft/chrome --headless --remote-debugging-port=0 --user-data-dir=/x/other-profile\n",
            ours_line(&f)
        ),
    )
    .unwrap();

    let o = run(&f, &["ls", "--json"], &[]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    let sessions = v["sessions"].as_array().unwrap();
    let own = sessions.iter().find(|s| s["session"] == "s-own").unwrap();
    assert_eq!(own["pid"], 5001, "the group's lowest pid is the browser");
    assert_eq!(own["headed"], true);
    assert_eq!(own["colour"], "#123456");
    let hid = sessions.iter().find(|s| s["session"] == "s-hid").unwrap();
    assert!(hid["pid"].is_null() && hid["headed"].is_null());
    // Both records name project vega-browser: the theme colour applies.
    assert_eq!(hid["colour"], "#123456");
    // An unowned session falls back to the stable hash of its name.
    let un = sessions.iter().find(|s| s["session"] == "s-un").unwrap();
    assert_eq!(un["colour"], mark::session_colour("s-un", None));
    assert_ne!(un["colour"], "#123456");
}

#[test]
fn ls_survives_a_missing_agent_browser() {
    let f = fix();
    fs::remove_file(f.bin.join("agent-browser")).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    record(&f, "s-live", Some("w1:p2"), &hostname(), now - 60, now - 60);
    herdr_panes(&f, r#"{"pane_id":"w1:p2"}"#);
    let o = run(
        &f,
        &["ls", "--json"],
        &[("VEGA_AGENT_BROWSER_BIN", "".into())],
    );
    assert!(o.status.success(), "{}", out(&o));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert!(
        v["error"].as_str().unwrap().contains("not found"),
        "{}",
        v["error"]
    );
    let s = v["sessions"].as_array().unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0]["session"], "s-live");
    assert_eq!(s[0]["state"], "live", "no session list: nothing is closed");
}

#[test]
fn gc_closes_dead_owners_and_prunes_dead_records_only() {
    let f = fix();
    let host = hostname();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    fs::write(
        f.path("sessions"),
        "s-dead\ns-live\ns-hc-dead\ns-far\ns-none\n",
    )
    .unwrap();
    herdr_panes(&f, r#"{"pane_id":"w1:p2"}"#);
    record(&f, "s-dead", Some("w1:p9"), &host, now - 7200, now - 60);
    record(&f, "s-live", Some("w1:p2"), &host, now - 60, now - 60);
    record(&f, "s-hc-dead", Some("t9"), &host, now - 60, now - 60);
    record(&f, "s-far", Some("w1:p2"), "otherhost", now - 60, now - 60);
    record(&f, "s-none", None, &host, now - 60, now - 60);
    record(&f, "s-ghost", Some("w1:p2"), &host, now - 60, now - 60);

    let o = run(&f, &["gc"], &[]);
    assert!(o.status.success(), "{}", out(&o));
    let text = out(&o);
    assert!(
        text.contains("closed s-dead (owner pane w1:p9 is gone)"),
        "{text}"
    );
    assert!(
        text.contains("closed s-hc-dead (owner pane t9 is gone)"),
        "{text}"
    );
    assert!(
        text.contains("pruned s-ghost (no agent-browser session)"),
        "{text}"
    );
    assert!(
        text.contains("gc: 2 closed, kept 3 (1 live owner pane, 1 on another host, 1 unattributed, 0 liveness unknown)"),
        "{text}"
    );
    let closed = lines_of(&f, "ab-argv");
    assert!(
        closed.contains(&"--session s-dead close".to_string()),
        "{closed:?}"
    );
    assert!(
        closed.contains(&"--session s-hc-dead close".to_string()),
        "{closed:?}"
    );
    let left = records(&f);
    for kept in ["s-live", "s-far", "s-none"] {
        assert!(left.get(kept).is_some(), "{left}");
    }
    for gone in ["s-dead", "s-hc-dead", "s-ghost"] {
        assert!(left.get(gone).is_none(), "{left}");
    }
}

#[test]
fn gc_keeps_sessions_when_liveness_is_unknown() {
    let f = fix();
    let host = hostname();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    fs::write(f.path("sessions"), "s-herdr-down\n").unwrap();
    fs::remove_file(f.bin.join("herdr")).unwrap(); // no herdr: liveness unknown
    record(&f, "s-herdr-down", Some("w1:p2"), &host, now - 60, now - 60);
    let o = run(&f, &["gc"], &[]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(
        out(&o).contains("kept s-herdr-down: cannot tell whether pane w1:p2 is alive"),
        "{}",
        out(&o)
    );
    // Only the session listing ran; nothing was closed.
    assert_eq!(lines_of(&f, "ab-argv"), ["session list"]);
    assert!(records(&f).get("s-herdr-down").is_some());
}

#[test]
fn close_closes_one_session_on_request() {
    let f = fix();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    record(&f, "s1", Some("w1:p2"), &hostname(), now, now);
    let o = run(&f, &["close", "s1"], &[]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert_eq!(out(&o).trim(), "closed: s1");
    assert_eq!(lines_of(&f, "ab-argv"), ["--session s1 close"]);
    assert!(records(&f).get("s1").is_none());

    let o = run(&f, &["close", "bad name"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("bad session name"), "{}", out(&o));
}
