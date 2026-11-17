//! `vega browser`: named agent-browser sessions (docs/design/browser.md).
//!
//! Owns the rule's tooling side: the global Chromium on 127.0.0.1:9223
//! (`ab`, `chromium`), the throwaway fallback for bot blockers (`launch`,
//! `cleanup`), and the session inventory (`ls`, `gc`, `close`) built from
//! agent-browser's session list, the owner records and the process table.

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

use vega_browser::{cdp, cdp::Probe, mark, owner, pane, procs, procs::Browser, real, session_name};
use vega_herdr::{capacity::duration, cell, print_failure, print_failure_json, Failure};

const HELP: &str = include_str!("../help/browser.md");
const SKILL: &str = include_str!("../SKILL.md");

/// A session with no call for this long is stale (design, ls).
const DAY: i64 = 86400;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let json = args.iter().any(|a| a == "--json");
    let result: Result<ExitCode, Failure> = match args.as_slice() {
        [] => ls(false).map(|_| ExitCode::SUCCESS),
        [a] if a == "-V" || a == "--version" => {
            println!("{}", vega_build::version_line!());
            Ok(ExitCode::SUCCESS)
        }
        [a] if a == "--skill" => {
            print!("{SKILL}");
            Ok(ExitCode::SUCCESS)
        }
        [a] if a == "ls" => ls(false).map(|_| ExitCode::SUCCESS),
        [a] if a == "--json" => ls(true).map(|_| ExitCode::SUCCESS),
        [a, b] if a == "ls" && b == "--json" => ls(true).map(|_| ExitCode::SUCCESS),
        [a, rest @ ..] if a == "ab" => ab(rest),
        [a, rest @ ..] if a == "chromium" => chromium(rest),
        [a, rest @ ..] if a == "launch" => launch(rest),
        [a, rest @ ..] if a == "cleanup" => cleanup(rest),
        [a] if a == "gc" => gc().map(|_| ExitCode::SUCCESS),
        [a, s] if a == "close" => close(s).map(|_| ExitCode::SUCCESS),
        _ => Err(Failure::new(
            "usage",
            "unknown command or arguments",
            "vega browser --help",
        )),
    };
    match result {
        Ok(code) => code,
        Err(f) => {
            if json {
                print_failure_json(&f);
            } else {
                print_failure(&f);
            }
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

/// The global Chromium: CDP port, persistent profile, binary. Environment
/// overrides (`CDP_PORT`, `CDP_PROFILE`, `CDP_CHROMIUM`) are the old
/// scripts' and stay for tests and non-default installs.
struct Global {
    port: u16,
    profile: PathBuf,
    chromium: PathBuf,
}

fn global() -> Global {
    let home = || PathBuf::from(env::var_os("HOME").unwrap_or_default());
    Global {
        port: env::var("CDP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(9223),
        profile: env::var_os("CDP_PROFILE")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local/share/vega-chromium")),
        chromium: env::var_os("CDP_CHROMIUM")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium")),
    }
}

fn no_agent_browser() -> Failure {
    Failure::new(
        "no_agent_browser",
        "the real agent-browser binary was not found",
        "install agent-browser (npm i -g agent-browser), or set VEGA_AGENT_BROWSER_BIN",
    )
}

/// `vega browser ab --session <name> <agent-browser args>`: agent-browser on
/// the global Chromium only, the port of the old `scripts/ab`.
fn ab(args: &[String]) -> Result<ExitCode, Failure> {
    let g = global();
    let Some(session) = session_name(args) else {
        return Err(vega_browser::name_failure());
    };
    let attach = format!("ab always attaches to 127.0.0.1:{}", g.port);
    let refuse =
        |what: &str| Failure::new("not_allowed", format!("{what} is not allowed"), &attach);
    if first_positional(args) == Some("connect") {
        return Err(refuse("connect"));
    }
    for a in args {
        let browser_picker = matches!(
            a.as_str(),
            "--cdp" | "--auto-connect" | "-p" | "--provider" | "--executable-path"
        ) || a.starts_with("--cdp=")
            || a.starts_with("--auto-connect=")
            || a.starts_with("--provider=")
            || a.starts_with("--executable-path=");
        if browser_picker {
            return Err(refuse(a));
        }
    }
    ensure_global(&g, None).map_err(|f| {
        Failure::new(
            &f.code,
            format!(
                "the global Chromium is not available; no fallback browser ({})",
                f.message
            ),
            &f.hint,
        )
    })?;
    let real = real::real_binary().ok_or_else(no_agent_browser)?;
    let record = owner::record_from_env(vega_shim::unix_now());
    owner::upsert(&session, record.clone());
    let mut cmd = Command::new(&real);
    cmd.arg("--cdp")
        .arg(g.port.to_string())
        .args(mark::with_mark(args, &session, record.project.as_deref()));
    for var in [
        "AGENT_BROWSER_AUTO_CONNECT",
        "AGENT_BROWSER_PROVIDER",
        "AGENT_BROWSER_EXECUTABLE_PATH",
    ] {
        cmd.env_remove(var);
    }
    let status = cmd.status().map_err(|e| {
        Failure::new(
            "agent_browser_failed",
            format!("{}: {e}", real.display()),
            "install agent-browser (npm i -g agent-browser)",
        )
    })?;
    if !status.success() && cdp::probe(g.port) != Probe::Cdp {
        eprintln!(
            "ab: lost the global Chromium on 127.0.0.1:{}; no fallback browser",
            g.port
        );
    }
    Ok(ExitCode::from(
        status.code().unwrap_or(1).clamp(0, 255) as u8
    ))
}

/// `vega browser chromium [url]`: attach to or start the global Chromium
/// (the old `chromium.sh`); idempotent.
fn chromium(args: &[String]) -> Result<ExitCode, Failure> {
    let usage = || {
        Failure::new(
            "usage",
            "vega browser chromium [url]",
            "the url opens only when Chromium starts",
        )
    };
    let url = match args {
        [] => None,
        [u] if !u.starts_with('-') => Some(u.clone()),
        _ => return Err(usage()),
    };
    let g = global();
    let label = ensure_global(&g, url.as_deref())?;
    println!(
        "{}: 127.0.0.1:{}, profile {}",
        label,
        g.port,
        g.profile.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// The old `chromium.sh` state machine.
fn ensure_global(g: &Global, url: Option<&str>) -> Result<&'static str, Failure> {
    let profile = g.profile.display().to_string();
    match cdp::probe(g.port) {
        Probe::Cdp => {
            if procs::ours(&procs::scan(), g.port, &profile) {
                Ok("attached")
            } else {
                Err(own_failure(g))
            }
        }
        Probe::Nothing => {
            if procs::ours(&procs::scan(), g.port, &profile) {
                wait_up(g, "attached", |p| {
                    format!("our Chromium runs but CDP on 127.0.0.1:{p} does not answer")
                })
            } else {
                start(g, url)?;
                wait_up(g, "started", |p| {
                    format!(
                        "Chromium did not open CDP on 127.0.0.1:{p} (is it already open with \
                         this profile but without CDP? Quit it and retry)"
                    )
                })
            }
        }
        Probe::Held => Err(Failure::new(
            "port_held",
            format!(
                "port {} is held by something that does not speak CDP",
                g.port
            ),
            "vega browser ls shows the browser processes with their pids",
        )),
    }
}

fn own_failure(g: &Global) -> Failure {
    Failure::new(
        "foreign_listener",
        format!(
            "port {} answers, but not from our Chromium (profile {})",
            g.port,
            g.profile.display()
        ),
        "stop the other listener or fix CDP_PROFILE; the global browser is never replaced",
    )
}

/// Up to 50 probes, 0.2 s apart, as the old `wait_up`.
fn wait_up(
    g: &Global,
    label: &'static str,
    timeout: impl Fn(u16) -> String,
) -> Result<&'static str, Failure> {
    for _ in 0..50 {
        if cdp::probe(g.port) == Probe::Cdp {
            return if procs::ours(&procs::scan(), g.port, &g.profile.display().to_string()) {
                Ok(label)
            } else {
                Err(own_failure(g))
            };
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(Failure::new("no_cdp", timeout(g.port), ""))
}

fn start(g: &Global, url: Option<&str>) -> Result<(), Failure> {
    if !g.chromium.is_file() {
        return Err(Failure::new(
            "missing_chromium",
            format!(
                "missing {} (brew install --cask ungoogled-chromium)",
                g.chromium.display()
            ),
            "install it, or point CDP_CHROMIUM at the binary",
        ));
    }
    fs::create_dir_all(&g.profile).map_err(|e| {
        Failure::new(
            "profile_unwritable",
            format!("{}: {e}", g.profile.display()),
            "fix the permissions or set CDP_PROFILE",
        )
    })?;
    Command::new("nohup")
        .arg(&g.chromium)
        .arg(format!("--remote-debugging-port={}", g.port))
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--user-data-dir={}", g.profile.display()))
        .args(["--no-first-run", "--no-default-browser-check"])
        .arg(url.unwrap_or("about:blank"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            Failure::new(
                "chromium_spawn",
                format!("{}: {e}", g.chromium.display()),
                "is the binary executable?",
            )
        })?;
    Ok(())
}

/// `vega browser launch <url> [--session s] [--port p]`: a throwaway Chromium
/// for bot blockers (the old `launch.sh`); never the global browser.
fn launch(args: &[String]) -> Result<ExitCode, Failure> {
    let g = global();
    let usage = || {
        Failure::new(
            "usage",
            "vega browser launch <url> [--session s] [--port p]",
            "a local-only throwaway Chromium for bot blockers; never the global port",
        )
    };
    let (url, session, port) = options(args, usage())?;
    let url = url.ok_or_else(usage)?;
    valid_session(&session)?;
    if port == g.port {
        return Err(Failure::new(
            "global_port",
            format!("port {port} is the global browser; use another port"),
            "vega browser ab drives the global one",
        ));
    }
    if !g.chromium.is_file() {
        return Err(Failure::new(
            "missing_chromium",
            format!(
                "missing {} (brew install --cask ungoogled-chromium)",
                g.chromium.display()
            ),
            "install it, or point CDP_CHROMIUM at the binary",
        ));
    }
    if cdp::connected(port) {
        return Err(Failure::new(
            "port_in_use",
            format!("port {port} is already in use"),
            "run vega browser cleanup, or pick another port",
        ));
    }
    let profile = env::temp_dir().join(format!("agent-browser-{session}-profile"));
    Command::new("nohup")
        .arg(&g.chromium)
        .arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(["--no-first-run", &url])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            Failure::new(
                "chromium_spawn",
                format!("{}: {e}", g.chromium.display()),
                "is the binary executable?",
            )
        })?;
    let mut up = false;
    for _ in 0..30 {
        if cdp::probe(port) == Probe::Cdp {
            up = true;
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    if !up {
        return Err(Failure::new(
            "no_cdp",
            format!("Chromium did not open CDP on 127.0.0.1:{port}"),
            "run vega browser cleanup and retry, or pick another port",
        ));
    }
    let real = real::real_binary().ok_or_else(no_agent_browser)?;
    let record = owner::record_from_env(vega_shim::unix_now());
    owner::upsert(&session, record);
    let attach = Command::new(&real)
        .args(["--session", &session, "connect", &port.to_string()])
        .output()
        .map_err(|e| {
            Failure::new(
                "agent_browser_failed",
                format!("{}: {e}", real.display()),
                "install agent-browser (npm i -g agent-browser)",
            )
        })?;
    if !attach.status.success() {
        return Err(Failure::new(
            "connect_failed",
            format!(
                "agent-browser connect {port}: {}{}",
                String::from_utf8_lossy(&attach.stdout).trim(),
                String::from_utf8_lossy(&attach.stderr).trim()
            ),
            "run vega browser cleanup and retry",
        ));
    }
    let current = Command::new(&real)
        .args(["--session", &session, "get", "url"])
        .output()
        .map_err(|e| Failure::new("agent_browser_failed", e.to_string(), ""))?;
    let current = String::from_utf8_lossy(&current.stdout).trim().to_owned();
    let host = host_of(&url);
    if !current.contains(&host) {
        return Err(Failure::new(
            "wrong_browser",
            format!("attached to the wrong browser (url: {current})"),
            "run vega browser cleanup and retry with another port",
        ));
    }
    owner::upsert(&session, owner::record_from_env(vega_shim::unix_now()));
    println!("session: {session}");
    println!("port: {port}");
    println!("profile: {}", profile.display());
    Ok(ExitCode::SUCCESS)
}

/// `vega browser cleanup [--session s] [--port p]`: close the fallback
/// browser, verify the port is closed, delete its profile (the old
/// `cleanup.sh`); refuses the global browser's port and profile first.
fn cleanup(args: &[String]) -> Result<ExitCode, Failure> {
    let g = global();
    let usage = || {
        Failure::new(
            "usage",
            "vega browser cleanup [--session s] [--port p]",
            "closes one throwaway fallback browser and deletes its profile",
        )
    };
    let (_, session, port) = options(args, usage())?;
    valid_session(&session)?;
    if port == g.port {
        return Err(Failure::new(
            "global_port",
            format!("port {port} is the global browser"),
            "the global Chromium stays; vega browser ab close only disconnects",
        ));
    }
    let profile = env::temp_dir().join(format!("agent-browser-{session}-profile"));
    let same = |a: &PathBuf, b: &PathBuf| {
        a.canonicalize().unwrap_or_else(|_| a.clone())
            == b.canonicalize().unwrap_or_else(|_| b.clone())
    };
    if same(&profile, &g.profile) {
        return Err(Failure::new(
            "global_profile",
            format!("{} is the global profile", profile.display()),
            "the global profile is never deleted",
        ));
    }
    if let Some(real) = real::real_binary() {
        let _ = Command::new(&real)
            .args(["--session", &session, "close"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    for pid in procs::with_profile(&procs::scan(), &profile.display().to_string()) {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
    for _ in 0..10 {
        if !cdp::connected(port) {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    if cdp::connected(port) {
        return Err(Failure::new(
            "port_open",
            format!("port {port} is still open"),
            "the fallback browser did not stop; check vega browser ls",
        ));
    }
    let _ = fs::remove_dir_all(&profile);
    owner::remove(&session);
    println!("closed: port {port}, profile deleted");
    Ok(ExitCode::SUCCESS)
}

/// Shared `--session/--port` (and a free first word) parsing for launch and
/// cleanup. Defaults: session `chromium`, port 9222 (the old scripts').
fn options(args: &[String], usage: Failure) -> Result<(Option<String>, String, u16), Failure> {
    let mut word = None;
    let mut session = String::from("chromium");
    let mut port: u16 = 9222;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let missing = || Failure {
            code: usage.code.clone(),
            message: usage.message.clone(),
            hint: usage.hint.clone(),
        };
        match a.as_str() {
            "--session" => session = it.next().ok_or_else(missing)?.clone(),
            s if s.starts_with("--session=") => session = s[10..].to_owned(),
            "--port" => port = it.next().and_then(|v| v.parse().ok()).ok_or_else(missing)?,
            s if s.starts_with("--port=") => port = s[7..].parse().map_err(|_| missing())?,
            w if word.is_none() && !w.starts_with('-') => word = Some(w.to_owned()),
            _ => return Err(missing()),
        }
    }
    Ok((word, session, port))
}

/// The first argument that is a command, not a flag or a flag's value.
fn first_positional(args: &[String]) -> Option<&str> {
    let mut skip_value = false;
    for a in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if a == "--session" {
            skip_value = true;
            continue;
        }
        if !a.starts_with('-') {
            return Some(a);
        }
    }
    None
}

fn valid_session(s: &str) -> Result<(), Failure> {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    plain.then_some(()).ok_or_else(|| {
        Failure::new(
            "bad_session",
            format!("bad session name: {s}"),
            "letters, digits, dot, underscore and hyphen only",
        )
    })
}

/// `scheme://host/rest` -> `host` (the old `sed` in launch.sh).
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest).to_owned()
}

/// agent-browser's active sessions; Err carries why (shown, never fatal).
fn session_names(real: Option<&PathBuf>) -> Result<Vec<String>, String> {
    let real = real.ok_or_else(|| "the real agent-browser binary was not found".to_owned())?;
    let out = Command::new(real)
        .args(["session", "list"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(vega_browser::parse_session_list(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// `vega browser ls [--json]`: sessions with owner and state, plus the
/// browser processes (`unowned` leftovers included).
fn ls(json: bool) -> Result<(), Failure> {
    let g = global();
    let records = owner::load();
    let real = real::real_binary();
    let listed = session_names(real.as_ref());
    let now = vega_shim::unix_now();
    let local = vega_herdr::local_host();
    let names: BTreeSet<String> = listed
        .iter()
        .flatten()
        .cloned()
        .chain(records.keys().cloned())
        .collect();
    let mut alive: HashMap<String, Option<bool>> = HashMap::new();
    let rows: Vec<(String, &str, Option<owner::Record>)> = names
        .iter()
        .map(|name| {
            let Some(rec) = records.get(name) else {
                return (name.clone(), "unowned", None);
            };
            let state = if listed.as_ref().is_ok_and(|l| !l.contains(name)) {
                "closed"
            } else if now - rec.last_seen > DAY {
                "stale"
            } else if let Some(pane) = &rec.pane {
                if rec.host != local
                    || *alive
                        .entry(pane.clone())
                        .or_insert_with(|| pane::alive(pane))
                        != Some(false)
                {
                    "live"
                } else {
                    "stale"
                }
            } else {
                "live"
            };
            (name.clone(), state, Some(rec.clone()))
        })
        .collect();
    let scanned = procs::scan();
    let global_profile = g.profile.display().to_string();
    let browsers: Vec<(Browser, bool)> = procs::browsers(&scanned)
        .into_iter()
        .map(|b| {
            let is_global = b.user_data_dir.as_deref() == Some(global_profile.as_str());
            (b, is_global)
        })
        .collect();
    let error = listed.as_ref().err().cloned();
    if json {
        println!("{}", json_view(&rows, &browsers, &error, now));
        return Ok(());
    }
    if let Some(why) = &error {
        println!("sessions: agent-browser unknown ({why}); records only");
    }
    println!(
        "sessions[{}]{{session,state,pane,host,project,age,idle}}:",
        rows.len()
    );
    for (name, state, rec) in &rows {
        let empty = (String::new(), String::new(), String::new());
        let (pane, host, project) = rec
            .as_ref()
            .map(|r| {
                (
                    r.pane.clone().unwrap_or_default(),
                    r.host.clone(),
                    r.project.clone().unwrap_or_default(),
                )
            })
            .unwrap_or(empty);
        let (age, idle) = rec
            .as_ref()
            .map(|r| (duration(now - r.first_seen), duration(now - r.last_seen)))
            .unwrap_or_default();
        println!(
            "  {},{},{},{},{},{},{}",
            cell(name),
            state,
            cell(&pane),
            cell(&host),
            cell(&project),
            age,
            idle
        );
    }
    println!(
        "browsers[{}]{{pid,kind,headed,age,global}}:",
        browsers.len()
    );
    for (b, is_global) in &browsers {
        println!(
            "  {},{},{},{},{}",
            b.pid,
            b.kind.as_str(),
            if b.headed { "headed" } else { "headless" },
            b.age.map(duration).unwrap_or_default(),
            if *is_global { "global" } else { "" }
        );
    }
    Ok(())
}

/// The stable JSON shape (design, phase 2): `headed` and `pid` are null
/// when this host cannot know them. A browser whose
/// `--user-data-dir` ends in `/<session>-profile` (agent-browser's own
/// headed launches) attributes that session; helper processes share the
/// main browser's arguments, so the group's lowest pid is the browser and
/// `headed` is true when any of them shows a window.
fn json_view(
    rows: &[(String, &str, Option<owner::Record>)],
    browsers: &[(Browser, bool)],
    error: &Option<String>,
    now: i64,
) -> serde_json::Value {
    let owned = |session: &str| {
        let suffix = format!("{session}-profile");
        let group = browsers
            .iter()
            .filter(|(b, _)| {
                b.user_data_dir
                    .as_deref()
                    .is_some_and(|dir| dir.rsplit_once('/').map_or(dir, |(_, last)| last) == suffix)
            })
            .collect::<Vec<_>>();
        (!group.is_empty()).then(|| {
            (
                group.iter().map(|(b, _)| b.pid).min(),
                group.iter().any(|(b, _)| b.headed),
            )
        })
    };
    serde_json::json!({
        "schema": 1,
        "sessions": rows.iter().map(|(name, state, rec)| {
            let browser = owned(name);
            serde_json::json!({
                "session": name,
                "state": state,
                "colour": mark::session_colour(name, rec.as_ref().and_then(|r| r.project.as_deref())),
                "pane": rec.as_ref().and_then(|r| r.pane.clone()),
                "host": rec.as_ref().map(|r| r.host.clone()),
                "project": rec.as_ref().and_then(|r| r.project.clone()),
                "cwd": rec.as_ref().map(|r| r.cwd.clone()),
                "age": rec.as_ref().map(|r| now - r.first_seen),
                "idle": rec.as_ref().map(|r| now - r.last_seen),
                "headed": browser.as_ref().map(|(_, headed)| *headed),
                "pid": browser.and_then(|(pid, _)| pid),
            })
        }).collect::<Vec<_>>(),
        "browsers": browsers.iter().map(|(b, is_global)| {
            serde_json::json!({
                "pid": b.pid,
                "kind": b.kind.as_str(),
                "headed": b.headed,
                "age": b.age,
                "port": b.port,
                "user_data_dir": b.user_data_dir,
                "global": is_global,
            })
        }).collect::<Vec<_>>(),
        "error": error,
    })
}

/// `vega browser gc`: close the sessions of dead owner panes, prune records
/// of sessions that no longer exist (design, decision 4: never a live
/// owner's session, never an unrecorded one).
fn gc() -> Result<(), Failure> {
    let records = owner::load();
    let real = real::real_binary().ok_or_else(no_agent_browser)?;
    let listed = session_names(Some(&real));
    let local = vega_herdr::local_host();
    let mut alive: HashMap<String, Option<bool>> = HashMap::new();
    let mut closed = 0;
    let mut live = 0;
    let mut remote = 0;
    let mut unattributed = 0;
    let mut unknown = 0;
    // Records of sessions that no longer exist go first: closing them is
    // pointless, and they must not count as kept.
    let mut pruned = BTreeSet::new();
    if let Ok(list) = &listed {
        for session in records.keys() {
            if !list.contains(session) {
                owner::remove(session);
                println!("pruned {session} (no agent-browser session)");
                pruned.insert(session.clone());
            }
        }
    }
    for (session, rec) in records.iter().filter(|(s, _)| !pruned.contains(*s)) {
        let Some(pane) = &rec.pane else {
            unattributed += 1;
            continue;
        };
        if rec.host != local {
            // Its owner's host runs its own gc with the same record.
            remote += 1;
            continue;
        }
        match *alive
            .entry(pane.clone())
            .or_insert_with(|| pane::alive(pane))
        {
            Some(true) => live += 1,
            None => {
                unknown += 1;
                println!("kept {session}: cannot tell whether pane {pane} is alive");
            }
            Some(false) => {
                let _ = Command::new(&real)
                    .args(["--session", session, "close"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                owner::remove(session);
                println!("closed {session} (owner pane {pane} is gone)");
                closed += 1;
            }
        }
    }
    println!(
        "gc: {closed} closed, kept {} ({} live owner pane, {} on another host, {} unattributed, {} liveness unknown)",
        live + remote + unattributed + unknown,
        live,
        remote,
        unattributed,
        unknown
    );
    Ok(())
}

/// `vega browser close <session>`: close one session on explicit request.
fn close(session: &str) -> Result<(), Failure> {
    valid_session(session)?;
    let real = real::real_binary().ok_or_else(no_agent_browser)?;
    let out = Command::new(real)
        .args(["--session", session, "close"])
        .output()
        .map_err(|e| Failure::new("agent_browser_failed", e.to_string(), ""))?;
    owner::remove(session);
    if !out.status.success() {
        return Err(Failure::new(
            "close_failed",
            format!(
                "agent-browser close {}: {}{}",
                session,
                String::from_utf8_lossy(&out.stdout).trim(),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            "check the session with agent-browser session list",
        ));
    }
    println!("closed: {session}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "browser");
    }

    #[test]
    fn help_lists_every_command() {
        for cmd in ["ab", "chromium", "launch", "cleanup", "ls", "gc", "close"] {
            assert!(HELP.contains(cmd), "help misses {cmd}");
        }
    }

    #[test]
    fn host_of_strips_scheme_and_path() {
        assert_eq!(host_of("https://example.com/a/b"), "example.com");
        assert_eq!(host_of("http://localhost:3000"), "localhost:3000");
        assert_eq!(host_of("example.com/"), "example.com");
    }

    #[test]
    fn session_names_validate() {
        assert!(valid_session("review-t7m").is_ok());
        assert!(valid_session("wt.journey_2").is_ok());
        assert!(valid_session("").is_err());
        assert!(valid_session("../x").is_err());
        assert!(valid_session("a b").is_err());
    }
}
