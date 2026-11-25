//! `vega browser` against fakes: a fake agent-browser (`VEGA_AGENT_BROWSER_BIN`),
//! a fake `ps`, `herdr` and `hc` on PATH, a fake Chromium that starts the
//! in-test DevTools responder, and temp homes. Nothing real is started.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

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
        for file in ["responders", "vpn-supervisors"] {
            if let Ok(pids) = fs::read_to_string(self.path(file)) {
                for pid in pids.lines().filter_map(|l| l.trim().parse::<u32>().ok()) {
                    let _ = Command::new("kill").arg(pid.to_string()).status();
                }
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
    fs::write(f.path("ca.pem"), include_str!("fixtures/vpn-ca.pem")).unwrap();
    script(
        &f.bin.join("security"),
        r#"printf '%s\n' "$*" >> "$VEGA_FIX/security-argv"; cat "$VEGA_FIX/ca.pem""#,
    );
    script(
        &f.bin.join("ssh"),
        r#"printf '%s\n' "$*" >> "$VEGA_FIX/ca-ssh-argv"; security find-certificate -a -p -c 'Internal Root CA' /Library/Keychains/System.keychain"#,
    );
    // agent-browser: records argv and env, answers the subcommands vega uses.
    script(
        &f.bin.join("agent-browser"),
        r#"
[ "$1" = "--version" ] && { echo "agent-browser 0.43.0"; exit 0; }
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
    fs::write(f.path("ps"), "").unwrap();
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
        .env("XDG_DATA_HOME", f.path("data"))
        .env("AGENT_BROWSER_SOCKET_DIR", f.path("sock"))
        .env_remove("AGENT_BROWSER_NAMESPACE")
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
        .env_remove("PI_CODING_AGENT")
        .env_remove("CLAUDECODE")
        .env_remove("AI_AGENT");
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

fn vpn_config(f: &Fix, extra: Value) -> (u16, u16) {
    let port = free_port();
    let socks = free_port();
    let mut cfg = serde_json::json!({"cdp_port":port, "socks_port":socks, "ca_sha256":test_ca().0});
    for (k, v) in extra.as_object().unwrap() {
        cfg[k] = v.clone();
    }
    fs::create_dir_all(f.home.join(".config/vega")).unwrap();
    fs::write(
        f.home.join(".config/vega/browser-vpn.json"),
        cfg.to_string(),
    )
    .unwrap();
    (port, socks)
}

#[test]
fn vpn_refuses_collisions_bad_config_and_implicit_start() {
    let f = fix();
    for cfg in [
        serde_json::json!({"cdp_port":9223}),
        serde_json::json!({"socks_port":f.port}),
        serde_json::json!({"host":"-bad"}),
        serde_json::json!({"typo":true}),
        serde_json::json!({"ca_sha256":"gg".repeat(32)}),
        serde_json::json!({"ca_sha256":123}),
        serde_json::json!({"profile":f.home.join(".local/share/vega-chromium")}),
    ] {
        vpn_config(&f, cfg);
        let o = run(&f, &["vpn", "start"], &[]);
        assert!(!o.status.success(), "{}", out(&o));
    }
    let (port, _) = vpn_config(&f, serde_json::json!({}));
    let held = TcpListener::bind(("127.0.0.1", port)).unwrap();
    let o = run(&f, &["vpn", "start"], &[]);
    assert!(out(&o).contains("occupied"), "{}", out(&o));
    drop(held);
    let o = run(
        &f,
        &["vpn", "start"],
        &[(
            "XDG_STATE_HOME",
            f.path("state with spaces").display().to_string(),
        )],
    );
    assert!(!o.status.success());
    assert!(out(&o).contains("whitespace"));
    let o = run(
        &f,
        &[
            "ab",
            "--profile",
            "vpn",
            "--session",
            "vpn-test",
            "get",
            "title",
        ],
        &[],
    );
    assert!(!o.status.success());
    assert!(!f.path("chromium-argv").exists());
    assert!(!f.path("ab-argv").exists());
    for args in [
        vec!["vpn", "start", "--typo"],
        vec!["ab", "--profile=bad", "--session", "test"],
    ] {
        assert_eq!(run(&f, &args, &[]).status.code(), Some(2));
    }
}

#[test]
fn vpn_lifecycle_reuses_launcher_and_isolates_agent_attach_and_stop() {
    let f = fix();
    let (port, socks) = vpn_config(&f, serde_json::json!({}));
    // Real local responder only; no real SSH, autossh, browser or corp request.
    script(
        &f.bin.join("autossh"),
        r#"
printf '%s\n' "$$" >> "$VEGA_FIX/vpn-supervisors"
printf '%s\n' "$*" > "$VEGA_FIX/ssh-argv"
printf '%s 0:01 /fake/autossh %s\n' "$$" "$*" >> "$VEGA_FIX/ps"
port=$(printf '%s\n' "$*" | tr ' ' '\n' | sed -n 's/^127.0.0.1://p')
python3 "$VEGA_FIX/responder.py" "$port" "$VEGA_FIX" &
touch "$VEGA_FIX/tunnel"
trap 'exit 0' TERM
while :; do sleep 1; done
"#,
    );
    script(
        &f.bin.join("ssh"),
        r#"
case "$*" in *"-O check"*) test -e "$VEGA_FIX/tunnel";;
*"-O exit"*) rm -f "$VEGA_FIX/tunnel";; *) security find-certificate -a -p -c 'Internal Root CA' /Library/Keychains/System.keychain;; esac
"#,
    );
    script(
        &f.bin.join("curl"),
        r#"printf '%s\n' "$*" >> "$VEGA_FIX/curl-argv"; printf 302"#,
    );
    let chromium = fs::read_to_string(f.bin.join("chromium"))
        .unwrap()
        .replace("> \"$VEGA_FIX/ps\"", ">> \"$VEGA_FIX/ps\"");
    fs::write(f.bin.join("chromium"), chromium).unwrap();
    // Fake kill only removes exact recorded rows; it never signals fixture PID 4242.
    script(
        &f.bin.join("kill"),
        r#"
printf '%s\n' "$1" >> "$VEGA_FIX/killed"
case "$1" in 4242) for p in $(cat "$VEGA_FIX/responders"); do /bin/kill "$p"; done;;
*) /bin/kill "$1"; rm -f "$VEGA_FIX/tunnel";; esac
awk -v pid="$1" '$1 != pid' "$VEGA_FIX/ps" > "$VEGA_FIX/ps.next"
mv "$VEGA_FIX/ps.next" "$VEGA_FIX/ps"
"#,
    );
    fs::write(
        f.path("ps"),
        format!("{}\n", ours_line(&f).replace("4242", "9999")),
    )
    .unwrap();
    let pinned = vega_browser::chrome::binary(&f.home);
    fs::create_dir_all(pinned.parent().unwrap()).unwrap();
    fs::rename(f.bin.join("chromium"), &pinned).unwrap();
    let started = run(&f, &["vpn", "start"], &[("CDP_CHROMIUM", String::new())]);
    assert!(
        started.status.success(),
        "{}{}",
        out(&started),
        err(&started)
    );
    let again = run(&f, &["vpn", "start"], &[]);
    assert!(
        again.status.success(),
        "{} ps={} status={}",
        out(&again),
        fs::read_to_string(f.path("ps")).unwrap(),
        out(&run(&f, &["vpn", "status", "--json"], &[]))
    );
    let argv = fs::read_to_string(f.path("chromium-argv")).unwrap();
    assert!(argv.contains(&format!("--remote-debugging-port={port}")));
    assert!(argv.contains(&format!("--proxy-server=socks5://127.0.0.1:{socks}")));
    assert!(!argv.contains("--host-resolver-rules"));
    assert!(argv.contains("--proxy-bypass-list=<-loopback>"));
    let ssh = fs::read_to_string(f.path("ssh-argv")).unwrap();
    assert!(ssh.contains(&format!("-D 127.0.0.1:{socks}")));
    assert!(ssh.contains("BatchMode=yes") && ssh.contains("ExitOnForwardFailure=yes"));
    assert!(!ssh.contains("-R"));
    let attached = run(
        &f,
        &[
            "ab",
            "--profile=vpn",
            "--session",
            "test-vpn",
            "get",
            "title",
        ],
        &[],
    );
    assert!(attached.status.success(), "{}", out(&attached));
    let ab = fs::read_to_string(f.path("ab-argv")).unwrap();
    assert!(
        ab.contains("--cdp ws://127.0.0.1:")
            && !ab.contains(&format!(":{port}/"))
            && !ab.contains("--profile")
    );
    let o = run(&f, &["vpn", "status", "--json"], &[]);
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["corp_http"], "302");
    assert_eq!(v["browser"], true);
    // The fake running version can differ from the installed pin.
    assert_eq!(v["browser_version"], "Chrome/146.0.0.0");
    assert!(out(&run(&f, &["vpn", "status"], &[])).contains("browser_version: Chrome/146.0.0.0"));
    assert_eq!(v["ca_trusted"], true);
    let curl = fs::read_to_string(f.path("curl-argv")).unwrap();
    assert!(curl.contains("--socks5-hostname") && curl.contains("--insecure"));
    // Stop follows the saved launch settings, even after a config edit.
    vpn_config(&f, serde_json::json!({"host":"different-host"}));
    let stopped = run(&f, &["vpn", "stop"], &[]);
    assert!(
        stopped.status.success(),
        "{}{}",
        out(&stopped),
        err(&stopped)
    );
    assert!(!fs::read_to_string(f.path("killed"))
        .unwrap()
        .contains("9999"));
    assert!(f.home.join(".local/share/vega-chromium-vpn").is_dir());
    assert!(run(&f, &["vpn", "stop"], &[]).status.success());
}

#[test]
fn vpn_without_a_pinned_browser_reports_the_install_command() {
    let f = fix();
    vpn_config(&f, serde_json::json!({}));
    let status = run(&f, &["vpn", "status", "--json"], &[]);
    let value: Value = serde_json::from_str(&out(&status)).unwrap();
    assert_eq!(value["browser"], false);
    assert!(value["browser_version"].is_null());
    let start = run(&f, &["vpn", "start"], &[("CDP_CHROMIUM", String::new())]);
    assert!(!start.status.success());
    assert!(
        out(&start).contains("vega browser install"),
        "{}",
        out(&start)
    );
    assert!(!f.path("chromium-argv").exists());
    assert!(!f.path("ca-ssh-argv").exists());
}

#[test]
fn vpn_start_failure_and_stale_pid_do_not_kill_foreign_processes() {
    let f = fix();
    vpn_config(&f, serde_json::json!({}));
    script(&f.bin.join("autossh"), "exit 1");
    script(&f.bin.join("ssh"), "exit 1");
    script(&f.bin.join("kill"), r#"echo "$*" >> "$VEGA_FIX/killed""#);
    let o = run(&f, &["vpn", "start"], &[]);
    assert!(!o.status.success());
    assert!(!f.path("chromium-argv").exists());
    assert!(!f.state.join("vega/browser/vpn/active.json").exists());
    fs::write(f.state.join("vega/browser/vpn/autossh.pid"), "8888").unwrap();
    fs::write(f.path("ps"), "8888 0:01 /some/other/program\n").unwrap();
    assert!(run(&f, &["vpn", "stop"], &[]).status.success());
    assert!(!f.path("killed").exists());
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
fn agent_tab_new_creates_in_background_until_explicitly_activated() {
    let f = fix();
    let run_tab = |args: &[&str]| {
        let mut argv = vec!["ab", "--session", "me", "tab"];
        argv.extend_from_slice(args);
        let o = run(&f, &argv, &[("CODEX_THREAD_ID", "agent-thread".into())]);
        assert!(o.status.success(), "{}", out(&o));
    };
    run_tab(&["new", "https://example.com/"]);
    run_tab(&["new"]);
    run_tab(&["t2"]);
    let requests = lines_of(&f, "target-requests")
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        requests,
        [
            serde_json::json!(["Target.createTarget", {"url":"about:blank", "background":true}]),
            serde_json::json!(["Target.createTarget", {"url":"about:blank", "background":true}]),
            serde_json::json!(["Target.activateTarget", {"targetId":"owned2"}]),
        ]
    );
    let chromium_args = fs::read_to_string(f.path("chromium-argv")).unwrap();
    assert!(chromium_args
        .split_whitespace()
        .any(|arg| arg == "--no-startup-window"));
}

#[test]
fn shared_session_cannot_fall_back_to_the_foreign_first_page() {
    use serde_json::json;
    use tungstenite::Message;
    let f = fix();
    let named = |args: &[&str]| {
        let mut argv = vec!["ab", "--profile", "global", "--session", "me"];
        argv.extend_from_slice(args);
        run(&f, &argv, &[])
    };
    let ok = |args: &[&str]| {
        let o = named(args);
        assert!(o.status.success(), "{}", out(&o));
        out(&o)
    };
    assert!(!ok(&["tab", "list"]).contains("foreign"));
    ok(&["tab", "new"]);
    assert!(ok(&["tab", "list"]).contains("[t2]"));
    for args in [vec!["tab", "foreign"], vec!["tab", "close", "t3"]] {
        assert!(!named(&args).status.success());
    }
    ok(&["tab", "close"]);
    ok(&["tab", "close"]);
    let endpoint = ok(&["get", "cdp-url"]);
    let (mut ws, _) = tungstenite::connect(endpoint.trim()).unwrap();
    let mut rpc = |method: &str, params: Value, session: Option<&str>| {
        let mut req = json!({"id":10,"method":method,"params":params});
        if let Some(session) = session {
            req["sessionId"] = json!(session);
        }
        ws.send(Message::text(req.to_string())).unwrap();
        serde_json::from_str::<Value>(ws.read().unwrap().to_text().unwrap()).unwrap()
    };
    let infos = rpc("Target.getTargets", json!({}), None);
    assert_eq!(infos["result"]["targetInfos"].as_array().unwrap().len(), 1);
    assert_eq!(infos["result"]["targetInfos"][0]["targetId"], "owned3");
    for method in [
        "Target.activateTarget",
        "Target.closeTarget",
        "Target.attachToTarget",
    ] {
        assert!(
            rpc(method, json!({"targetId":"foreign","flatten":true}), None)["error"].is_object()
        );
    }
    assert!(rpc("Runtime.evaluate", json!({}), Some("foreign"))["error"].is_object());
    assert_eq!(
        rpc("Runtime.evaluate", json!({}), Some("owned3"))["result"]["target"],
        "owned3"
    );
    drop(ws);
    assert!(ok(&["get", "url"]).contains("https://example.com/x"));
    assert_eq!(records(&f)["me"]["project"], "proj");
    ok(&["close"]);
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
fn ab_never_runs_without_a_verified_browser_endpoint() {
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

    // An HTTP 200 without a browser generation cannot authorize a daemon.
    let f2 = fix();
    serve(&f2, f2.port);
    fs::write(f2.path("ps"), ours_line(&f2)).unwrap();
    let o = run(&f2, &["ab", "--session", "me", "get", "url"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(lines_of(&f2, "ab-argv").is_empty());
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

fn daemon_socket(f: &Fix, session: &str) -> UnixListener {
    fs::create_dir_all(f.path("sock")).unwrap();
    UnixListener::bind(f.path(&format!("sock/{session}.sock"))).unwrap()
}

fn expect_disconnect(socket: UnixListener) {
    socket.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match socket.accept() {
            Ok((stream, _)) => break stream,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("expected disconnect: {e}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut command = String::new();
    BufReader::new(&stream).read_line(&mut command).unwrap();
    let request: Value = serde_json::from_str(&command).unwrap();
    assert_eq!(request["action"], "close");
    writeln!(
        stream,
        "{}",
        serde_json::json!({
            "id": request["id"], "success": true, "data": {"closed": true}
        })
    )
    .unwrap();
}

#[test]
fn gc_closes_only_daemons_of_dead_local_owners() {
    let f = fix();
    let host = hostname();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    fs::write(
        f.path("sessions"),
        "s-dead\ns-live\ns-hc-dead\ns-far\ns-none\ns-unrecorded\n",
    )
    .unwrap();
    herdr_panes(&f, r#"{"pane_id":"w1:p2"}"#);
    record(&f, "s-dead", Some("w1:p9"), &host, now - 7200, now - 60);
    record(&f, "s-live", Some("w1:p2"), &host, now - 60, now - 60);
    record(&f, "s-hc-dead", Some("t9"), &host, now - 60, now - 60);
    record(&f, "s-far", Some("w1:p2"), "otherhost", now - 60, now - 60);
    record(&f, "s-none", None, &host, now - 60, now - 60);
    record(&f, "s-ghost", Some("w1:p2"), &host, now - 60, now - 60);

    let workers: Vec<_> = ["s-dead", "s-hc-dead"]
        .into_iter()
        .map(|name| {
            let socket = daemon_socket(&f, name);
            thread::spawn(move || expect_disconnect(socket))
        })
        .collect();
    // Ownerless sessions remain untouched even when their sockets exist.
    let untouched: Vec<_> = ["s-unrecorded", "s-live", "s-far", "s-none"]
        .into_iter()
        .map(|name| daemon_socket(&f, name))
        .collect();
    let o = run(&f, &["gc"], &[]);
    for worker in workers {
        worker.join().unwrap();
    }
    for socket in untouched {
        socket.set_nonblocking(true).unwrap();
        assert_eq!(
            socket.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
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
    assert!(text.contains("gc: 2 closed, 4 kept"), "{text}");
    assert!(lines_of(&f, "ab-argv").is_empty());
    let left = records(&f);
    for kept in ["s-live", "s-far", "s-none", "s-ghost"] {
        assert!(left.get(kept).is_some(), "{left}");
    }
    for gone in ["s-dead", "s-hc-dead"] {
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
    let _unknown = daemon_socket(&f, "s-herdr-down");
    fs::remove_file(f.bin.join("herdr")).unwrap(); // no herdr: liveness unknown
    record(&f, "s-herdr-down", Some("w1:p2"), &host, now - 60, now - 60);
    let o = run(&f, &["gc"], &[]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(
        out(&o).contains("kept s-herdr-down: unknown"),
        "{}",
        out(&o)
    );
    assert!(lines_of(&f, "ab-argv").is_empty());
    assert!(records(&f).get("s-herdr-down").is_some());
}

#[test]
fn gc_only_releases_dev_bindings_with_a_matching_dead_local_owner() {
    let f = fix();
    let dir = f.path("data/vega/browser/dev-cft");
    fs::create_dir_all(&dir).unwrap();
    let registry = dir.join("vega-dev.json");
    let session = serde_json::json!({
        "project": "fixture",
        "generation": "/devtools/browser/expired",
        "pane": "w1:p9",
        "active": 1,
        "next": 2,
        "tabs": {"1": {"target": "fixture", "port": 0}}
    });
    fs::write(
        &registry,
        serde_json::json!({
            "browser": null, "projects": {},
            "sessions": {"dev-dead": session, "dev-transferred": session}
        })
        .to_string(),
    )
    .unwrap();
    herdr_panes(&f, r#"{"pane_id":"w1:p2"}"#);
    let host = hostname();
    record(&f, "dev-dead", Some("w1:p9"), &host, 1, 2);
    record(&f, "dev-transferred", Some("w1:p2"), &host, 1, 2);
    let output = run(&f, &["gc"], &[]);
    assert!(output.status.success(), "{output:?}");
    let current: Value = serde_json::from_slice(&fs::read(registry).unwrap()).unwrap();
    assert!(current["sessions"]["dev-dead"].is_null());
    assert!(current["sessions"]["dev-transferred"].is_object());
}

#[test]
fn legacy_disconnect_requires_current_ownerless_socket_identity() {
    let f = fix();
    fs::write(f.path("sessions"), "ownerless\nprotected\n").unwrap();
    let old = daemon_socket(&f, "ownerless");
    let protected = daemon_socket(&f, "protected");
    record(&f, "protected", Some("w1:p2"), "remote", 1, 2);
    let args = ["gc", "--dry-run"];
    let dry = run(&f, &args, &[]);
    assert!(dry.status.success(), "{dry:?}");
    let original = out(&dry);
    let identity = |report: &str| {
        report
            .lines()
            .find_map(|line| line.strip_prefix("  ownerless,ownerless,"))
            .filter(|id| id.len() == 64)
            .unwrap()
            .to_owned()
    };
    let reviewed = identity(&original);
    assert!(original.contains("protected,remote,"));
    assert!(lines_of(&f, "ab-argv").is_empty());

    fs::remove_file(f.path("sock/ownerless.sock")).unwrap();
    let replacement = daemon_socket(&f, "ownerless");
    let rejected = run(
        &f,
        &["disconnect", "ownerless", "--identity", &reviewed],
        &[],
    );
    assert!(!rejected.status.success(), "{rejected:?}");
    assert!(out(&rejected).contains("no longer active with that identity"));
    let current = identity(&out(&run(&f, &args, &[])));
    assert_ne!(current, reviewed);

    record(&f, "ownerless", Some("w1:p2"), &hostname(), 1, 2);
    let rejected = run(
        &f,
        &["disconnect", "ownerless", "--identity", &current],
        &[],
    );
    assert!(!rejected.status.success(), "{rejected:?}");
    assert!(out(&rejected).contains("no longer a legacy ownerless entry"));
    // The old socket and newly owned socket were never connected to.
    for socket in [&old, &replacement, &protected] {
        socket.set_nonblocking(true).unwrap();
        assert_eq!(
            socket.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    record(&f, "ownerless", None, &hostname(), 1, 2);
    let worker = thread::spawn(move || expect_disconnect(replacement));
    let closed = run(
        &f,
        &["disconnect", "ownerless", "--identity", &current],
        &[],
    );
    worker.join().unwrap();
    assert!(closed.status.success(), "{closed:?}");
    assert!(records(&f).get("ownerless").is_none());
    assert!(records(&f).get("protected").is_some());
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

fn test_ca() -> (String, Vec<u8>) {
    let pem = include_str!("fixtures/vpn-ca.pem");
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .unwrap();
    (format!("{:x}", Sha256::digest(&der)), der)
}

#[test]
fn vpn_trust_pins_ca_and_preserves_existing_certificates() {
    let f = fix();
    vpn_config(&f, serde_json::json!({}));
    let db_path = f
        .home
        .join(".local/share/vega-chromium-vpn/Default/ServerCertificate");
    let initial = run(&f, &["vpn", "status", "--json"], &[]);
    assert_eq!(
        serde_json::from_str::<Value>(&out(&initial)).unwrap()["ca_trusted"],
        false
    );
    assert!(!db_path.exists());
    let imported = run(&f, &["vpn", "trust", "--json"], &[]);
    assert!(
        imported.status.success(),
        "{}{}",
        out(&imported),
        err(&imported)
    );
    let db = rusqlite::Connection::open(&db_path).unwrap();
    let (pin, der) = test_ca();
    let stored: (Vec<u8>, Vec<u8>) = db
        .query_row(
            "SELECT der_cert,trust_settings FROM certificates WHERE sha256hash_hex=?",
            [&pin],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored, (der, vec![10, 2, 8, 3]));
    db.execute(
        "INSERT INTO certificates VALUES('unrelated',x'12',x'34')",
        [],
    )
    .unwrap();
    let before = fs::read(&db_path).unwrap();
    fs::remove_file(f.path("ca.pem")).unwrap();
    assert!(run(&f, &["vpn", "trust"], &[]).status.success());
    assert_eq!(before, fs::read(&db_path).unwrap());
    assert!(!f.home.join(".local/share/vega-chromium").exists());
    let ssh = fs::read_to_string(f.path("ca-ssh-argv")).unwrap();
    assert!(ssh.contains("BatchMode=yes") && ssh.contains("ConnectTimeout=10"));
    assert_eq!(ssh.lines().count(), 1);
    assert!(ssh.contains("puck security find-certificate -a -p -c 'Internal Root CA' /Library/Keychains/System.keychain"));
    assert!(fs::read_to_string(f.path("security-argv"))
        .unwrap()
        .contains("find-certificate -a -p -c Internal Root CA /Library/Keychains/System.keychain"));
    db.execute(
        "UPDATE certificates SET trust_settings=x'0a020802' WHERE sha256hash_hex=?",
        [&pin],
    )
    .unwrap();
    let before = fs::read(&db_path).unwrap();
    let o = run(&f, &["vpn", "trust"], &[]);
    assert!(
        !o.status.success() && out(&o).contains("no overwrite"),
        "{}",
        out(&o)
    );
    assert_eq!(before, fs::read(&db_path).unwrap());
}

#[test]
fn vpn_trust_refuses_mismatch_running_profile_symlink_and_unknown_schema() {
    let f = fix();
    vpn_config(&f, serde_json::json!({"ca_sha256":"00".repeat(32)}));
    let profile = f.home.join(".local/share/vega-chromium-vpn");
    let o = run(&f, &["vpn", "start"], &[]);
    assert!(
        !o.status.success() && out(&o).contains("fingerprint mismatch"),
        "{}",
        out(&o)
    );
    assert!(!profile.exists());
    assert!(!f.path("chromium-argv").exists());
    vpn_config(&f, serde_json::json!({}));
    fs::write(
        f.path("ps"),
        format!("4242 1:00 Chromium --user-data-dir={}\n", profile.display()),
    )
    .unwrap();
    let o = run(&f, &["vpn", "trust"], &[]);
    assert!(
        !o.status.success() && out(&o).contains("stop the VPN"),
        "{}",
        out(&o)
    );
    fs::write(f.path("ps"), "").unwrap();
    fs::create_dir_all(&profile).unwrap();
    std::os::unix::fs::symlink(&f.tmp, profile.join("Default")).unwrap();
    assert!(out(&run(&f, &["vpn", "trust"], &[])).contains("symlinks"));
    fs::remove_file(profile.join("Default")).unwrap();
    assert!(run(&f, &["vpn", "trust"], &[]).status.success());
    let path = profile.join("Default/ServerCertificate");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE meta SET value='2' WHERE key='version'", [])
        .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(out(&run(&f, &["vpn", "trust"], &[])).contains("unsupported"));
    assert_eq!(before, fs::read(&path).unwrap());
    db.execute("UPDATE meta SET value='1'", []).unwrap();
    db.execute("ALTER TABLE certificates ADD COLUMN future TEXT", [])
        .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(out(&run(&f, &["vpn", "trust"], &[])).contains("unsupported"));
    assert_eq!(before, fs::read(&path).unwrap());
}
