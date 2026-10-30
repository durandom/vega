//! `vega review`: live review of a running web app (docs/design/live-review.md).
//!
//! `start` opens the app in an agent-browser session with an injected overlay
//! and starts this session's loopback inbox (`serve`, detached). The owner
//! comments on elements in the page; the inbox batches the notes into one mail
//! to the agent's pane. The agent shows elements with `highlight` and answers
//! with `reply`, both rendered in the page.

mod csp;
mod inbox;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::{json, Value};
use vega_herdr::state::{mode_of, parent_of, redirects_questions, state_file};
use vega_herdr::{agent_name, cell, herdr_local, machine, me, print_failure, Failure};

use inbox::{
    mail_text, read_request, valid_patch_name, write_response, Inbox, MAX_PATCH, STATUSES,
};

const OVERLAY: &str = include_str!("overlay.js");
const CFG_PLACEHOLDER: &str = "__VEGA_REVIEW_CFG__";
/// The `--from-tool` label of the inbox's mail.
const TOOL: &str = "vega-review";
/// The inbox gives up after this long, in case `stop` never runs.
const MAX_LIFETIME: Duration = Duration::from_secs(24 * 3600);

const SKILL: &str = include_str!("../SKILL.md");

#[derive(Parser)]
#[command(
    name = "vega review",
    bin_name = "vega review",
    disable_version_flag = true,
    about = "Live review: highlight elements in the owner's browser, get their in-page notes as mail",
    after_help = "Without a command: this pane's review session (app, browser session, inbox, notes).\n\nExamples:\n  vega review start http://127.0.0.1:5287/\n  vega review highlight '#save' --label \"This one?\"\n  vega review reply 3 \"Done: renamed\" --status done\n  vega review stop"
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the app with the review overlay in a headed browser and start the inbox
    #[command(
        after_help = "URL: a local dev host (localhost, 127.x, private IPs, *.localhost, *.test, *.local,\n*.internal). The overlay runs only on that origin. One review per pane.\n--headless runs the browser headless and skips the headed-pane check: for scripted\ntests, where nobody looks at the page.\n\nExamples:\n  vega review start http://127.0.0.1:5287/\n  vega review start http://app.localhost:3000/settings --session spire-review"
    )]
    Start {
        url: String,
        /// agent-browser session name [default: review-<pane>]
        #[arg(long)]
        session: Option<String>,
        /// Headless browser, no headed-pane check (tests)
        #[arg(long)]
        headless: bool,
    },
    /// Draw a persistent, labelled box around an element in the page
    Highlight {
        /// CSS selector
        selector: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// Remove all highlights
    Clear,
    /// Answer a note on its pin and set its status
    #[command(
        after_help = "Examples:\n  vega review reply 3 \"Looking at it\" --status working\n  vega review reply 3 \"Renamed to Store\""
    )]
    Reply {
        id: u64,
        text: String,
        /// working, done or skipped
        #[arg(long, default_value = "done", value_parser = STATUSES)]
        status: String,
    },
    /// All notes of this session with their status and reply
    Notes,
    /// Inject a design patch (JS) into the page; it re-runs after reload and navigation
    #[command(
        after_help = "The patch runs on every page of the reviewed origin and again after DOM changes\n(SPA navigation, re-renders), so it must be idempotent: mark what it adds or changes\nwith data-patched=\"<name>\" (<name>.js is the file) and skip when the marker is there.\nPatching the same file again replaces it and reloads the page; --clear removes it\nand reloads.\n\nExamples:\n  vega review patch hero.js\n  vega review patch --list\n  vega review patch --clear hero.js\n  vega review patch --clear"
    )]
    Patch {
        /// The patch file (<name>.js); with --clear, the patch to remove
        file: Option<PathBuf>,
        /// The patches of this session and how they ran on the current page
        #[arg(long, conflicts_with_all = ["file", "clear"])]
        list: bool,
        /// Remove the named patch, or all, and reload the page
        #[arg(long)]
        clear: bool,
    },
    /// Screenshot the page, cropped to an element with --selector (overlay hidden)
    #[command(
        after_help = "Examples:\n  vega review shot --selector '.hero'\n  vega review shot --out /tmp/page.png"
    )]
    Shot {
        /// CSS selector of the element to crop to
        #[arg(long)]
        selector: Option<String>,
        /// Output path [default: agent-browser's screenshot dir]
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Close the browser session and the inbox
    Stop,
    /// The inbox process (started by `start`)
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        dir: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp) => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let msg = e.to_string();
            let message = msg
                .lines()
                .take_while(|l| !l.trim().is_empty())
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" ");
            print_failure(&Failure::new(
                "usage",
                message.trim_start_matches("error: "),
                "valid usage below",
            ));
            print!("{}", Cli::command().render_help());
            return ExitCode::from(2);
        }
    };
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    if cli.skill {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    let result = match cli.cmd {
        None => status(),
        Some(Cmd::Start {
            url,
            session,
            headless,
        }) => start(&url, session, headless),
        Some(Cmd::Highlight { selector, label }) => highlight(&selector, label.as_deref()),
        Some(Cmd::Clear) => clear(),
        Some(Cmd::Reply { id, text, status }) => reply(id, &text, &status),
        Some(Cmd::Notes) => notes(),
        Some(Cmd::Patch { file, list, clear }) => match (file, list, clear) {
            (_, true, _) => patch_list(),
            (file, _, true) => patch_clear(file.as_deref()),
            (Some(file), _, _) => patch(&file),
            (None, _, _) => Err(Failure::new(
                "usage",
                "patch needs a file, --list or --clear",
                "vega review patch <file.js>",
            )),
        },
        Some(Cmd::Shot { selector, out }) => shot(selector.as_deref(), out.as_deref()),
        Some(Cmd::Stop) => stop(),
        Some(Cmd::Serve { dir }) => serve(&dir),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

/// `scheme://host[:port]` of an http(s) URL on a local dev host.
fn local_origin(url: &str) -> Result<String, Failure> {
    let bad = |why: &str| {
        Failure::new(
            "usage",
            format!("{url}: {why}"),
            "pass an http(s) URL of a local dev server, e.g. http://127.0.0.1:5287/",
        )
    };
    let (scheme, rest) = url.split_once("://").ok_or_else(|| bad("not a URL"))?;
    if scheme != "http" && scheme != "https" {
        return Err(bad("not http(s)"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(bad("no host, or credentials in the URL"));
    }
    let host = if authority.starts_with('[') {
        authority.split(']').next().unwrap_or("").to_string() + "]"
    } else {
        authority.split(':').next().unwrap_or("").to_string()
    };
    if !is_dev_host(&host.to_ascii_lowercase()) {
        return Err(bad("not a local dev host"));
    }
    Ok(format!("{scheme}://{}", authority.to_ascii_lowercase()))
}

fn is_dev_host(h: &str) -> bool {
    let octets: Vec<u8> = h.split('.').filter_map(|p| p.parse().ok()).collect();
    let ipv4 = octets.len() == 4 && h.split('.').count() == 4;
    h == "localhost"
        || h == "[::1]"
        || (ipv4
            && (octets[0] == 127
                || octets[0] == 10
                || (octets[0] == 192 && octets[1] == 168)
                || (octets[0] == 172 && (16..=31).contains(&octets[1]))))
        || [".localhost", ".test", ".local", ".internal"]
            .iter()
            .any(|s| h.ends_with(s))
}

fn session_dir(pane: &str) -> Result<PathBuf, Failure> {
    state_file("review", pane).ok_or_else(|| {
        Failure::new(
            "no_state_dir",
            "neither XDG_STATE_HOME nor HOME is set",
            "set HOME",
        )
    })
}

fn my_pane() -> Result<String, Failure> {
    me().map(|m| m.pane)
}

/// Whether the owner works in this pane: no parent, or a headed child.
fn owner_works_here(pane: &str) -> bool {
    let Ok(a) = herdr_local(&["agent", "get", pane]) else {
        return false;
    };
    let name = agent_name(&a["result"]["agent"]);
    !redirects_questions(
        parent_of(pane, name).as_ref(),
        mode_of(machine(), pane, name),
    )
}

fn random_hex(bytes: usize) -> Result<String, Failure> {
    let mut buf = vec![0u8; bytes];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| Failure::new("no_random", format!("/dev/urandom: {e}"), ""))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn write_private(path: &Path, text: &str) -> Result<(), Failure> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| Failure::new("write_failed", format!("{}: {e}", path.display()), ""))
}

fn read_config(dir: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(dir.join("config.json")).ok()?).ok()
}

/// This pane's running review: (dir, config with port).
fn current() -> Result<(PathBuf, Value), Failure> {
    let dir = session_dir(&my_pane()?)?;
    let none = || {
        Failure::new(
            "no_review",
            "no review session in this pane",
            "start one: vega review start <url>",
        )
    };
    let cfg = read_config(&dir).ok_or_else(none)?;
    if !alive(cfg["pid"].as_u64()) {
        return Err(Failure::new(
            "inbox_gone",
            "the review inbox is not running",
            "vega review stop, then vega review start <url>",
        ));
    }
    Ok((dir, cfg))
}

fn alive(pid: Option<u64>) -> bool {
    pid.is_some_and(|p| {
        Command::new("kill")
            .args(["-0", &p.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

fn agent_browser(session: &str, args: &[&str]) -> Result<String, Failure> {
    let out = Command::new("agent-browser")
        .arg("--session")
        .arg(session)
        .args(args)
        .output()
        .map_err(|e| {
            Failure::new(
                "no_agent_browser",
                format!("agent-browser: {e}"),
                "install agent-browser (npm i -g agent-browser)",
            )
        })?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Failure::new(
            "agent_browser_failed",
            format!("agent-browser {}: {} {}", args.join(" "), text, err.trim()),
            "check the session with agent-browser session list",
        ));
    }
    Ok(text)
}

fn start(url: &str, session: Option<String>, headless: bool) -> Result<(), Failure> {
    let origin = local_origin(url)?;
    let pane = my_pane()?;
    if !headless && !owner_works_here(&pane) {
        return Err(Failure::new(
            "headless_pane",
            format!("{pane} is a headless child: nobody would see the overlay"),
            "the owner must work in this pane (vega mux mode <pane> headed), or use --headless for tests",
        ));
    }
    let dir = session_dir(&pane)?;
    if let Some(cfg) = read_config(&dir) {
        if alive(cfg["pid"].as_u64()) {
            return Err(Failure::new(
                "already_running",
                format!(
                    "a review of {} runs in this pane",
                    cfg["url"].as_str().unwrap_or("?")
                ),
                "vega review stop first",
            ));
        }
    }
    let _ = fs::remove_dir_all(&dir);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| Failure::new("write_failed", format!("{}: {e}", dir.display()), ""))?;
    let session = session.unwrap_or_else(|| format!("review-{}", pane.replace(':', "-")));
    let secret = random_hex(32)?;
    let cfg = json!({
        "url": url, "origin": origin, "pane": pane, "session": session, "secret": secret,
        "headed": !headless,
        "blank": format!("about:blank#vega-review-{}", random_hex(8)?),
    });
    write_private(&dir.join("config.json"), &cfg.to_string())?;

    let log = fs::File::create(dir.join("inbox.log"))
        .map_err(|e| Failure::new("write_failed", format!("inbox.log: {e}"), ""))?;
    let exe = std::env::current_exe()
        .map_err(|e| Failure::new("no_exe", format!("current_exe: {e}"), ""))?;
    Command::new(exe)
        .arg("serve")
        .arg("--dir")
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(
            log.try_clone()
                .map_err(|e| Failure::new("write_failed", e.to_string(), ""))?,
        )
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|e| Failure::new("spawn_failed", format!("inbox: {e}"), ""))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let cfg = loop {
        match read_config(&dir) {
            Some(c) if c["port"].is_u64() => break c,
            _ if Instant::now() > deadline => {
                return Err(Failure::new(
                    "inbox_failed",
                    "the inbox did not start",
                    &format!("see {}", dir.join("inbox.log").display()),
                ))
            }
            _ => thread::sleep(Duration::from_millis(50)),
        }
    };
    let port = cfg["port"].as_u64().unwrap_or(0);

    let overlay_cfg = json!({
        "origin": origin, "secret": secret, "endpoint": format!("http://127.0.0.1:{port}"),
    });
    let script = OVERLAY.replace(CFG_PLACEHOLDER, &overlay_cfg.to_string());
    let script_path = dir.join("overlay.js");
    write_private(&script_path, &script)?;
    let script_arg = script_path.display().to_string();
    if let Err(f) = open_review(&session, &script_arg, url, headless, &dir, &cfg) {
        let _ = agent_browser(&session, &["close"]);
        stop_inbox(&dir, &cfg);
        return Err(f);
    }
    // A reused session can carry an earlier `set viewport`; the owner's page
    // must follow their window from the start.
    let reset = if headless {
        None
    } else {
        follow_window(&session)
    };
    println!("review: {url}");
    println!(
        "browser: agent-browser session {session} ({})",
        if headless { "headless" } else { "headed" }
    );
    print_reset(reset);
    println!("inbox: 127.0.0.1:{port} (notes need the session secret and Origin {origin})");
    println!("help: the owner comments in the page; notes arrive as mail (vega post inbox). Show elements: vega review highlight <selector> --label <text>");
    Ok(())
}

/// Launch on a unique blank tab so the inbox can attach CSP bypass before the
/// app's CSP is initialized. The init script still runs on every navigation.
fn open_review(
    session: &str,
    script: &str,
    url: &str,
    headless: bool,
    dir: &Path,
    cfg: &Value,
) -> Result<(), Failure> {
    agent_browser(
        session,
        &[
            "--init-script",
            script,
            "--headed",
            if headless { "false" } else { "true" },
            "open",
            cfg["blank"].as_str().unwrap_or("about:blank"),
        ],
    )?;
    csp::prepare(dir, session)?;
    agent_browser(session, &["open", url])?;
    Ok(())
}

fn stop_inbox(dir: &Path, cfg: &Value) {
    if let Some(pid) = cfg["pid"].as_u64() {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
    let _ = fs::remove_dir_all(dir);
}

fn stop() -> Result<(), Failure> {
    let dir = session_dir(&my_pane()?)?;
    let cfg = read_config(&dir).ok_or_else(|| {
        Failure::new(
            "no_review",
            "no review session in this pane",
            "nothing to stop",
        )
    })?;
    let notes = cli_get(&cfg, "/notes").ok();
    let session = cfg["session"].as_str().unwrap_or("").to_string();
    let closed = agent_browser(&session, &["close"]).is_ok();
    stop_inbox(&dir, &cfg);
    println!("stopped: {}", cfg["url"].as_str().unwrap_or("?"));
    println!(
        "browser: session {session} {}",
        if closed { "closed" } else { "was not open" }
    );
    if let Some(n) = notes {
        let open = n["notes"].as_array().map_or(0, |a| {
            a.iter()
                .filter(|x| !matches!(x["status"].as_str(), Some("done" | "skipped")))
                .count()
        });
        println!(
            "notes: {} total, {open} not done or skipped",
            n["notes"].as_array().map_or(0, Vec::len)
        );
    }
    Ok(())
}

/// A request from the CLI to the inbox: secret header, no Origin.
fn cli_request(cfg: &Value, method: &str, path: &str, body: &str) -> Result<Value, Failure> {
    let port = cfg["port"].as_u64().unwrap_or(0);
    let unreachable = |e: std::io::Error| {
        Failure::new(
            "inbox_unreachable",
            format!("127.0.0.1:{port}: {e}"),
            "vega review stop, then start again",
        )
    };
    let mut s = TcpStream::connect(("127.0.0.1", port as u16)).map_err(unreachable)?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Vega-Secret: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        cfg["secret"].as_str().unwrap_or(""),
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(unreachable)?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(unreachable)?;
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    if !head.starts_with("HTTP/1.1 200") {
        return Err(Failure::new(
            "inbox_refused",
            format!(
                "{} {}",
                head.lines().next().unwrap_or(""),
                v["error"].as_str().unwrap_or("")
            ),
            "check the note id with vega review notes",
        ));
    }
    Ok(v)
}

fn cli_get(cfg: &Value, path: &str) -> Result<Value, Failure> {
    cli_request(cfg, "GET", path, "")
}

/// Runs a call of the overlay's `window.__vegaReview` in the review session.
/// `eval` prints the result as JSON; a page without the overlay gives the
/// string `no_overlay`.
fn overlay_call(cfg: &Value, call: &str) -> Result<Value, Failure> {
    let session = cfg["session"].as_str().unwrap_or("");
    let js = format!("window.__vegaReview ? (window.__vegaReview.{call} ?? null) : 'no_overlay'");
    let out = agent_browser(session, &["eval", &js])?;
    Ok(serde_json::from_str(&out).unwrap_or(Value::String(out)))
}

/// Viewport sizes from the page: inner and outer width and height.
const SIZES_JS: &str = "JSON.stringify([innerWidth, innerHeight, outerWidth, outerHeight])";

/// The emulated viewport `[w, h]` when a device metrics override pins the page
/// (`agent-browser set viewport` or `set device`), None when it follows the
/// window. Without one, the inner width equals the window's outer width (bar a
/// border) and the inner height is below the outer one.
fn pinned(sizes: &str) -> Option<(u64, u64)> {
    let v: Vec<u64> = serde_json::from_str::<String>(sizes)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())?;
    let [iw, ih, ow, oh] = v[..] else { return None };
    (iw.abs_diff(ow) > 16 || ih > oh).then_some((iw, ih))
}

/// Clears a device metrics override so the page follows the window again:
/// `set viewport 0 0 0` (CDP: 0 disables the override). Returns the size the
/// page was pinned at, if any. Best effort: a failure only keeps the override.
fn follow_window(session: &str) -> Option<(u64, u64)> {
    let size = pinned(&agent_browser(session, &["eval", SIZES_JS]).ok()?)?;
    agent_browser(session, &["set", "viewport", "0", "0", "0"]).ok()?;
    Some(size)
}

fn print_reset(reset: Option<(u64, u64)>) {
    if let Some((w, h)) = reset {
        println!("viewport: was fixed at {w}x{h}; reset, the page follows the window again");
    }
}

/// What `highlight` printed or failed with, from the overlay's answer.
fn highlight_result(out: &Value, selector: &str, label: Option<&str>) -> Result<String, Failure> {
    if out == "no_overlay" {
        return Err(Failure::new(
            "no_overlay",
            "the current page has no review overlay",
            "the overlay runs only on the reviewed origin; open a page of it",
        ));
    }
    match out["found"].as_bool() {
        Some(true) => Ok(format!(
            "highlight: {selector}{}, shown ({} on the page)",
            label.map(|l| format!(" ({l})")).unwrap_or_default(),
            out["highlights"].as_u64().unwrap_or(1)
        )),
        Some(false) => Err(Failure::new(
            "not_found",
            format!("no element matches {selector} on the current page"),
            "agent-browser --session <session> snapshot to find it",
        )),
        None => Err(Failure::new(
            "overlay_failed",
            format!("highlight {selector}: unexpected answer {out}"),
            "reload the page (agent-browser --session <session> reload) and try again",
        )),
    }
}

fn highlight(selector: &str, label: Option<&str>) -> Result<(), Failure> {
    let (_, cfg) = current()?;
    let reset = if cfg["headed"].as_bool().unwrap_or(true) {
        follow_window(cfg["session"].as_str().unwrap_or(""))
    } else {
        None
    };
    let call = format!(
        "highlight({}, {})",
        Value::from(selector),
        label.map_or(Value::Null, Value::from)
    );
    let line = highlight_result(&overlay_call(&cfg, &call)?, selector, label)?;
    println!("{line}");
    print_reset(reset);
    println!("help: vega review clear removes highlights; they also go when the page navigates");
    Ok(())
}

fn clear() -> Result<(), Failure> {
    let (_, cfg) = current()?;
    overlay_call(&cfg, "clear()")?;
    println!("cleared: highlights");
    Ok(())
}

fn reply(id: u64, text: &str, status: &str) -> Result<(), Failure> {
    let (_, cfg) = current()?;
    let body = json!({ "id": id, "text": text, "status": status }).to_string();
    cli_request(&cfg, "POST", "/reply", &body)?;
    // The overlay polls the inbox every 3 s; this only makes it immediate.
    let shown = overlay_call(&cfg, "refresh()").is_ok_and(|v| v != "no_overlay");
    println!("reply: note {id} {status}");
    if !shown {
        println!("page: refresh failed; the overlay picks it up within 3 s");
    }
    Ok(())
}

fn notes() -> Result<(), Failure> {
    let (_, cfg) = current()?;
    let v = cli_get(&cfg, "/notes")?;
    let list = v["notes"].as_array().cloned().unwrap_or_default();
    println!(
        "notes[{}]{{id,status,url,selector,quote,note,replies}}:",
        list.len()
    );
    for n in &list {
        let f = |k: &str| cell(n[k].as_str().unwrap_or(""));
        println!(
            "  {},{},{},{},{},{},{}",
            n["id"],
            f("status"),
            f("url"),
            f("selector"),
            f("quote"),
            f("text"),
            n["replies"].as_array().map_or(0, Vec::len)
        );
    }
    if list.is_empty() {
        println!("help: no notes yet; the owner annotates in the page (Annotate or Alt+click), then Send");
    } else if list.iter().any(|n| n["status"] == "draft") {
        println!("help: drafts are the owner's, not sent yet; wait for their mail");
    }
    Ok(())
}

/// The patch name of `<dir>/<name>.js` (or a bare name, for `--clear`).
fn patch_name(file: &Path) -> Result<String, Failure> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.strip_suffix(".js").unwrap_or(n))
        .unwrap_or("");
    if !valid_patch_name(name) {
        return Err(Failure::new(
            "usage",
            format!(
                "{}: the name must be letters, digits, - or _",
                file.display()
            ),
            "name the file like hero-card.js; the name is also the data-patched marker",
        ));
    }
    Ok(name.to_string())
}

/// What `patch` printed or failed with, from the overlay's `patches()`.
fn patch_result(page: &Value, name: &str) -> Result<String, Failure> {
    if page == "no_overlay" {
        return Ok(format!(
            "patch: {name} stored; the current page has no overlay, it runs on the reviewed origin's pages"
        ));
    }
    let p = &page[name];
    if let Some(e) = p["error"].as_str() {
        return Err(Failure::new(
            "patch_error",
            format!("{name}: {e}"),
            &format!(
                "fix the file and run vega review patch again; vega review patch --clear {name} removes it"
            ),
        ));
    }
    match p["runs"].as_u64() {
        Some(n) if n > 0 => Ok(format!("patch: {name} applied ({n} run(s) on this page)")),
        _ => Err(Failure::new(
            "patch_not_run",
            format!("{name}: the overlay has not run it on this page"),
            "vega review patch --list shows it once the page has loaded; check the page with vega review shot",
        )),
    }
}

/// The overlay's patch status, waiting until `name` has run or failed (after
/// a reload the overlay fetches its patches first).
fn page_patches(cfg: &Value, name: Option<&str>) -> Value {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let v = overlay_call(cfg, "patches()").unwrap_or(Value::Null);
        let ready = name.is_none_or(|n| {
            v == "no_overlay"
                || v[n]["runs"].as_u64().unwrap_or(0) != 0
                || v[n]["error"].is_string()
        });
        if ready || Instant::now() > deadline {
            return v;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn reload(cfg: &Value) -> Result<(), Failure> {
    agent_browser(cfg["session"].as_str().unwrap_or(""), &["reload"]).map(drop)
}

fn patch(file: &Path) -> Result<(), Failure> {
    let name = patch_name(file)?;
    let code = fs::read_to_string(file).map_err(|e| {
        Failure::new(
            "usage",
            format!("{}: {e}", file.display()),
            "pass a readable .js file",
        )
    })?;
    if code.len() > MAX_PATCH {
        return Err(Failure::new(
            "usage",
            format!(
                "{}: {} bytes, the limit is {MAX_PATCH}",
                file.display(),
                code.len()
            ),
            "split it into several patches",
        ));
    }
    if !code.contains("data-patched") {
        return Err(Failure::new(
            "usage",
            format!("{}: no data-patched marker", file.display()),
            &format!(
                "patches re-run after every DOM change: mark what they add with data-patched=\"{name}\" and skip when it is there"
            ),
        ));
    }
    let (_, cfg) = current()?;
    let body = json!({ "name": name, "code": code }).to_string();
    let replaced = cli_request(&cfg, "POST", "/patch", &body)?["replaced"] == true;
    // A changed patch would skip on its old marker: start from the app's page.
    if replaced {
        reload(&cfg)?;
    } else {
        overlay_call(&cfg, "refresh()")?;
    }
    let line = patch_result(&page_patches(&cfg, Some(&name)), &name)?;
    println!("{line}");
    if replaced {
        println!("page: reloaded, the previous version replaced");
    }
    println!("help: it re-runs after reload and navigation; check it with vega review shot --selector <css>");
    Ok(())
}

/// One `patch --list` row per stored patch, with how it ran on the page.
fn patch_rows(stored: &Value, page: &Value) -> Vec<String> {
    let list = stored["patches"].as_array().cloned().unwrap_or_default();
    list.iter()
        .map(|p| {
            let name = p["name"].as_str().unwrap_or("");
            let on_page = &page[name];
            let runs = on_page["runs"]
                .as_u64()
                .map_or("-".to_string(), |n| n.to_string());
            format!(
                "  {},{},{runs},{}",
                cell(name),
                p["bytes"],
                cell(on_page["error"].as_str().unwrap_or(""))
            )
        })
        .collect()
}

fn patch_list() -> Result<(), Failure> {
    let (_, cfg) = current()?;
    let stored = cli_get(&cfg, "/patches")?;
    let rows = patch_rows(&stored, &page_patches(&cfg, None));
    println!("patches[{}]{{name,bytes,runs,error}}:", rows.len());
    for r in &rows {
        println!("{r}");
    }
    if rows.is_empty() {
        println!("help: vega review patch <file.js> injects one");
    } else {
        println!("help: runs counts this page view (- when it has no overlay); vega review patch --clear [<file>] removes");
    }
    Ok(())
}

fn patch_clear(file: Option<&Path>) -> Result<(), Failure> {
    let name = file.map(patch_name).transpose()?;
    let (_, cfg) = current()?;
    let body = json!({ "name": name }).to_string();
    let v = cli_request(&cfg, "POST", "/unpatch", &body).map_err(|f| match &name {
        Some(n) if f.message.contains("404") => Failure::new(
            "not_found",
            format!("no patch named {n}"),
            "vega review patch --list shows the patches",
        ),
        _ => f,
    })?;
    let removed: Vec<&str> = v["removed"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if removed.is_empty() {
        println!("patches: none to clear");
        return Ok(());
    }
    // Their nodes stay in the page until it reloads.
    reload(&cfg)?;
    println!("cleared: {}", removed.join(", "));
    println!("page: reloaded, it shows the app without them");
    Ok(())
}

/// The file agent-browser wrote, from its `✓ Screenshot saved to <path>`.
fn shot_path(out: &str) -> Option<&str> {
    out.lines()
        .find_map(|l| l.split_once("Screenshot saved to ").map(|(_, p)| p.trim()))
}

fn shot(selector: Option<&str>, out: Option<&Path>) -> Result<(), Failure> {
    let (_, cfg) = current()?;
    let session = cfg["session"].as_str().unwrap_or("");
    // The agent-browser daemon has its own working directory.
    let out = out
        .map(|o| std::path::absolute(o).map(|p| p.display().to_string()))
        .transpose()
        .map_err(|e| Failure::new("usage", format!("--out: {e}"), ""))?;
    let mut args = vec!["screenshot"];
    args.extend(selector);
    args.extend(out.as_deref());
    // Highlights and the panel would cover the design under review.
    let hidden = overlay_call(&cfg, "hide(true)").is_ok_and(|v| v != "no_overlay");
    let res = agent_browser(session, &args);
    if hidden {
        let _ = overlay_call(&cfg, "hide(false)");
    }
    let text = res.map_err(|f| match selector {
        Some(sel) if f.message.contains("Element not found") => Failure::new(
            "not_found",
            format!("no element matches {sel} on the current page"),
            &format!("agent-browser --session {session} snapshot to find it"),
        ),
        _ => f,
    })?;
    let path = shot_path(&text).ok_or_else(|| {
        Failure::new(
            "agent_browser_failed",
            format!("screenshot: unexpected answer {text}"),
            "",
        )
    })?;
    println!(
        "shot: {path}{}",
        selector.map(|s| format!(" ({s})")).unwrap_or_default()
    );
    println!("help: read the image to check the design; vega review patch <file.js> to iterate");
    Ok(())
}

fn status() -> Result<(), Failure> {
    let pane = my_pane()?;
    let dir = session_dir(&pane)?;
    let Some(cfg) = read_config(&dir) else {
        println!("review: none in {pane}");
        println!("help: vega review start <url> opens a local app with the review overlay");
        return Ok(());
    };
    let up = alive(cfg["pid"].as_u64());
    println!("review: {}", cfg["url"].as_str().unwrap_or("?"));
    println!(
        "browser: agent-browser session {}",
        cfg["session"].as_str().unwrap_or("?")
    );
    println!(
        "inbox: {}",
        if up {
            format!("127.0.0.1:{} running", cfg["port"])
        } else {
            "not running (vega review stop, then start)".into()
        }
    );
    if up {
        if let Ok(v) = cli_get(&cfg, "/notes") {
            let list = v["notes"].as_array().cloned().unwrap_or_default();
            let count = |s: &str| list.iter().filter(|n| n["status"] == s).count();
            println!(
                "notes: {} (draft {}, queued {}, sent {}, working {}, done {}, skipped {})",
                list.len(),
                count("draft"),
                count("queued"),
                count("sent"),
                count("working"),
                count("done"),
                count("skipped")
            );
        }
    }
    println!("help: vega review notes lists them; vega review stop ends the review");
    Ok(())
}

/// The inbox: binds 127.0.0.1 on a free port, records it in config.json,
/// serves the overlay and the CLI, and mails batches to the pane. Exits when
/// config.json goes away (`stop`) or after `MAX_LIFETIME`.
fn serve(dir: &Path) -> Result<(), Failure> {
    let mut cfg = read_config(dir).ok_or_else(|| {
        Failure::new(
            "no_review",
            "no config.json",
            "started by vega review start",
        )
    })?;
    let s = |k: &str| cfg[k].as_str().unwrap_or("").to_string();
    let (secret, origin, pane, session) = (s("secret"), s("origin"), s("pane"), s("session"));
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| Failure::new("bind_failed", e.to_string(), ""))?;
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    cfg["port"] = port.into();
    cfg["pid"] = std::process::id().into();
    write_private(&dir.join("config.json"), &cfg.to_string())?;
    eprintln!("inbox: 127.0.0.1:{port} for {origin}, mail to {pane}");

    let inbox = Arc::new(Mutex::new(Inbox::new(&secret, &origin)));
    let born = Instant::now();
    {
        let inbox = Arc::clone(&inbox);
        let config = dir.join("config.json");
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(250));
            if !config.exists() || born.elapsed() > MAX_LIFETIME {
                eprintln!("inbox: exiting");
                std::process::exit(0);
            }
            let batch = inbox
                .lock()
                .ok()
                .and_then(|mut i| i.take_batch(Instant::now()));
            let Some(batch) = batch else { continue };
            let text = mail_text(&session, &origin, &batch);
            match mail(&pane, &text) {
                Ok(()) => eprintln!("mailed: {} note(s) to {pane}", batch.len()),
                Err(e) => {
                    eprintln!("mail failed, retrying: {e}");
                    if let Ok(mut i) = inbox.lock() {
                        i.requeue(&batch, Instant::now());
                    }
                }
            }
        });
    }
    // Keep this attachment alive alongside the inbox, including across renderer
    // changes. Only the unique tab launched by start receives CSP bypass.
    let _csp = csp::listen(dir, cfg["blank"].as_str().unwrap_or(""))?;
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let inbox = Arc::clone(&inbox);
        thread::spawn(move || {
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let Some(req) = read_request(&mut stream) else {
                eprintln!("rejected: malformed request");
                return;
            };
            let resp = match inbox.lock() {
                Ok(mut i) => i.handle(&req, Instant::now()),
                Err(_) => return,
            };
            if let Some(l) = &resp.log {
                eprintln!("{l}");
            }
            write_response(&mut stream, &resp);
        });
    }
    Ok(())
}

/// One mail to the reviewing pane, sent as a tool (no sender pane).
/// `VEGA_REVIEW_POST` overrides the vega-post binary (development builds).
fn mail(pane: &str, text: &str) -> Result<(), String> {
    let post = std::env::var("VEGA_REVIEW_POST").unwrap_or_else(|_| "vega-post".into());
    let out = Command::new(&post)
        .args(["send", "--from-tool", TOOL, pane, text])
        .output()
        .map_err(|e| format!("{post}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[cfg(test)]
mod browser_test;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_meets_the_skill_contract() {
        vega_plugin::assert_guide(SKILL, "review");
    }

    #[test]
    fn origins_of_local_dev_hosts_only() {
        assert_eq!(
            local_origin("http://127.0.0.1:5287/a?b").unwrap(),
            "http://127.0.0.1:5287"
        );
        assert_eq!(
            local_origin("https://App.localhost").unwrap(),
            "https://app.localhost"
        );
        assert_eq!(
            local_origin("http://[::1]:8080/").unwrap(),
            "http://[::1]:8080"
        );
        assert_eq!(
            local_origin("http://192.168.1.4:3000").unwrap(),
            "http://192.168.1.4:3000"
        );
        for bad in [
            "https://example.com/",
            "http://127.0.0.1.evil.com/",
            "http://user@127.0.0.1/",
            "file:///tmp/x.html",
            "http://172.32.0.1/",
            "127.0.0.1:5287",
        ] {
            assert!(local_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn every_highlight_confirms_or_fails() {
        let ok = highlight_result(
            &json!({"found": true, "highlights": 2}),
            "#a",
            Some("This?"),
        );
        assert_eq!(ok.unwrap(), "highlight: #a (This?), shown (2 on the page)");
        let code = |v: Value| highlight_result(&v, "#a", None).unwrap_err().code;
        assert_eq!(code(json!({"found": false})), "not_found");
        assert_eq!(code(json!("no_overlay")), "no_overlay");
        assert_eq!(code(Value::Null), "overlay_failed");
        assert_eq!(code(json!("some text")), "overlay_failed");
    }

    #[test]
    fn pinned_viewport_from_page_sizes() {
        let p = |a: [u64; 4]| pinned(&Value::from(json!(a).to_string()).to_string());
        // Measured: the review owner's 1728x1080 override in a 1312x734 window.
        assert_eq!(p([1728, 1080, 1312, 734]), Some((1728, 1080)));
        // Measured: `set viewport 1000 700` after resizing the window to 900x600.
        assert_eq!(p([1000, 700, 900, 600]), Some((1000, 700)));
        // Measured: no override, the page follows a 900x600 window.
        assert_eq!(p([900, 457, 900, 600]), None);
        assert_eq!(pinned("garbage"), None);
    }

    #[test]
    fn overlay_has_one_config_placeholder() {
        assert_eq!(OVERLAY.matches(CFG_PLACEHOLDER).count(), 1);
    }

    #[test]
    fn patch_names_come_from_the_file_stem() {
        assert_eq!(
            patch_name(Path::new("/tmp/x/hero-card.js")).unwrap(),
            "hero-card"
        );
        assert_eq!(patch_name(Path::new("nav_2")).unwrap(), "nav_2");
        for bad in ["/tmp/x/my patch.js", "/tmp/x/a\"b.js", ".js", "/"] {
            assert_eq!(
                patch_name(Path::new(bad)).unwrap_err().code,
                "usage",
                "{bad}"
            );
        }
    }

    #[test]
    fn every_patch_confirms_or_fails() {
        let ok = patch_result(&json!({"hero": {"runs": 2, "error": null}}), "hero");
        assert_eq!(ok.unwrap(), "patch: hero applied (2 run(s) on this page)");
        let off = patch_result(&json!("no_overlay"), "hero").unwrap();
        assert!(off.starts_with("patch: hero stored;"), "{off}");
        let code = |v: Value| patch_result(&v, "hero").unwrap_err().code;
        assert_eq!(
            code(json!({"hero": {"runs": 0, "error": "x is not defined"}})),
            "patch_error"
        );
        assert_eq!(
            code(json!({"hero": {"runs": 0, "error": null}})),
            "patch_not_run"
        );
        assert_eq!(code(json!({})), "patch_not_run");
        assert_eq!(code(Value::Null), "patch_not_run");
    }

    #[test]
    fn patch_list_rows_join_store_and_page() {
        let stored =
            json!({"patches": [{"name": "hero", "bytes": 120}, {"name": "nav", "bytes": 80}]});
        let page =
            json!({"hero": {"runs": 3, "error": null}, "nav": {"runs": 0, "error": "boom, bad"}});
        assert_eq!(
            patch_rows(&stored, &page),
            ["  hero,120,3,", "  nav,80,0,\"boom, bad\""]
        );
        assert_eq!(
            patch_rows(&stored, &json!("no_overlay")),
            ["  hero,120,-,", "  nav,80,-,"]
        );
        assert!(patch_rows(&json!({"patches": []}), &Value::Null).is_empty());
    }

    #[test]
    fn shot_path_from_agent_browser_output() {
        // Measured: agent-browser 0.31.1 `screenshot '#box' <path>`.
        assert_eq!(
            shot_path("✓ Screenshot saved to /tmp/x/box.png"),
            Some("/tmp/x/box.png")
        );
        assert_eq!(shot_path("✓ Done"), None);
    }

    #[test]
    fn patch_and_shot_parse_their_flags() {
        let parse = |a: &[&str]| Cli::try_parse_from([&["vega review"], a].concat());
        assert!(parse(&["patch", "hero.js"]).is_ok());
        assert!(parse(&["patch", "--list"]).is_ok());
        assert!(parse(&["patch", "--clear"]).is_ok());
        assert!(parse(&["patch", "--clear", "hero.js"]).is_ok());
        assert!(parse(&["patch", "--list", "hero.js"]).is_err());
        assert!(parse(&["patch", "--list", "--clear"]).is_err());
        assert!(parse(&["shot", "--selector", ".hero", "--out", "/tmp/a.png"]).is_ok());
    }
}
