//! Shared by the vega plugins that talk to herdr: the `error{code,message,hint}`
//! contract, running herdr (locally or on a `--machine`), and finding the pane
//! that hosts this process.

pub mod badges;
pub mod capacity;
pub mod catalog;
pub mod cutover;
pub mod dialog;
pub mod gather;
pub mod hosts;
pub mod lanes;
pub mod notes;
pub mod pane;
pub mod project;
pub mod resets;
pub mod service_config;
pub mod state;
pub mod theme;
pub mod waiting;
pub mod waits;

use std::env;
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// The herdr machine that `--machine` targets; `None` is this machine.
static MACHINE: OnceLock<Option<String>> = OnceLock::new();

pub fn machine() -> Option<&'static str> {
    MACHINE.get().and_then(|m| m.as_deref())
}

/// ` --machine <label>` for hints, so suggested commands keep the target.
pub fn mflag() -> String {
    machine()
        .map(|m| format!(" --machine {m}"))
        .unwrap_or_default()
}

/// A pane id as the user should see it: `io:w1:p2` on another machine, bare
/// on this one. `vega open` jumps to either form when it is selected.
pub fn qualify(machine: Option<&str>, pane: &str) -> String {
    match machine {
        Some(m) => format!("{m}:{pane}"),
        None => pane.to_string(),
    }
}

/// A failure rendered as `error{code,message,hint}` on stdout.
#[derive(Debug)]
pub struct Failure {
    pub code: String,
    pub message: String,
    pub hint: String,
}

impl Failure {
    pub fn new(code: &str, message: impl Into<String>, hint: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: hint.into(),
        }
    }
}

pub fn print_failure(f: &Failure) {
    println!("error{{code,message,hint}}:");
    println!("  {},{},{}", f.code, cell(&f.message), cell(&f.hint));
}

pub fn local_host() -> String {
    Command::new("hostname")
        .arg("-s")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// Local wall-clock time for humans, e.g. `2026-10-07 12:05 CEST`: now, or the
/// given Unix time. Uses `date` (BSD `-r`, GNU `-d @`) to avoid a timezone
/// dependency; `None` if neither works.
pub fn local_time(unix: Option<i64>) -> Option<String> {
    const FMT: &str = "+%Y-%m-%d %H:%M %Z";
    let attempts: Vec<Vec<String>> = match unix {
        None => vec![vec![FMT.into()]],
        Some(t) => vec![
            vec!["-r".into(), t.to_string(), FMT.into()],
            vec!["-d".into(), format!("@{t}"), FMT.into()],
        ],
    };
    attempts.into_iter().find_map(|args| {
        let out = Command::new("date").args(&args).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !s.is_empty()).then_some(s)
    })
}

/// Runs `herdr <args>` on the target machine (`--machine`, else this one) and
/// returns its JSON result, or maps its error JSON.
pub fn herdr(args: &[&str]) -> Result<Value, Failure> {
    parse_json(&run_herdr(args, true)?)
}

/// Like `herdr`, but always on this machine: finding our own pane.
pub fn herdr_local(args: &[&str]) -> Result<Value, Failure> {
    parse_json(&run_herdr(args, false)?)
}

/// `herdr <args>` on the target machine, stdout as text (e.g. `agent read`).
pub fn herdr_text(args: &[&str]) -> Result<String, Failure> {
    run_herdr(args, true)
}

/// Like `herdr`, but on an explicit machine (`None`: this one) and bounded by
/// `timeout`: views that ask several machines must not hang on one.
pub fn herdr_on(machine: Option<&str>, args: &[&str], timeout: Duration) -> Result<Value, Failure> {
    parse_json(&run_herdr_on(machine, args, Some(timeout))?)
}

fn parse_json(stdout: &str) -> Result<Value, Failure> {
    serde_json::from_str(stdout.trim()).map_err(|_| {
        Failure::new(
            "herdr_bad_output",
            "herdr returned non-JSON output",
            "check `herdr --version`",
        )
    })
}

/// Runs herdr and maps failures. Successful commands print JSON or plain text;
/// failures are JSON on either stream.
fn run_herdr(args: &[&str], on_target: bool) -> Result<String, Failure> {
    run_herdr_on(if on_target { machine() } else { None }, args, None)
}

fn run_herdr_on(
    target: Option<&str>,
    args: &[&str],
    timeout: Option<Duration>,
) -> Result<String, Failure> {
    let mut full: Vec<&str> = Vec::new();
    if let Some(m) = target {
        full.extend(["--machine", m]);
    }
    full.extend(args);
    let mut cmd = Command::new("herdr");
    cmd.args(&full);
    let not_found = |e: std::io::Error| {
        Failure::new(
            "herdr_not_found",
            e.to_string(),
            "install herdr or put it on PATH",
        )
    };
    let out = match timeout {
        None => cmd.output().map_err(not_found)?,
        Some(t) => output_within(&mut cmd, t)
            .map_err(not_found)?
            .ok_or_else(|| {
                Failure::new(
                    "herdr_timeout",
                    format!("herdr did not answer within {}s", t.as_secs()),
                    "check that the machine is up and reachable over ssh",
                )
            })?,
    };

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let error: Option<Value> = [stdout.trim(), stderr.trim()]
        .iter()
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .find_map(|v| v.get("error").cloned());

    if let Some(err) = error {
        let code = err["code"].as_str().unwrap_or("herdr_error");
        return Err(Failure::new(
            code,
            err["message"].as_str().unwrap_or(""),
            hint_for(code),
        ));
    }
    if !out.status.success() {
        // An agent sandbox (e.g. Codex) denies connecting to the herdr socket.
        if stderr.contains("PermissionDenied") || stderr.contains("Operation not permitted") {
            let socket = env::var("HERDR_SOCKET_PATH").unwrap_or_else(|_| "herdr socket".into());
            return Err(Failure::new(
                "socket_denied",
                format!(
                    "a sandbox blocks the connection to {}",
                    collapse_home(&socket)
                ),
                "re-run this command with escalated permissions, outside the agent sandbox",
            ));
        }
        let first = stderr.lines().next().unwrap_or("").trim();
        return Err(Failure::new(
            "herdr_failed",
            format!("herdr failed: {first}"),
            "run `vega mux` to check the current state",
        ));
    }
    Ok(stdout)
}

/// Runs `cmd` and returns its output, or `None` if it outlives `timeout`
/// (then it is killed). Pipes are drained on threads so a full pipe cannot
/// block the child.
pub fn output_within(cmd: &mut Command, timeout: Duration) -> std::io::Result<Option<Output>> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let drain = |r: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(20));
    };
    Ok(Some(Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    }))
}

fn hint_for(code: &str) -> &'static str {
    match code {
        "agent_not_found" => {
            "run `vega mux` to list agents; the target pane must host a running agent"
        }
        "agent_blocked" => "the target waits at an approval dialog; ask the human or retry later",
        _ => "run `vega mux` to check the current state",
    }
}

/// The herdr pane that hosts this process.
pub struct Me {
    pub pane: String,
    pub workspace: String,
    pub agent: String,
    /// The agent kind if herdr detects one, else the program we run under:
    /// e.g. `zsh` for a plain shell, `opencode` for an undetected harness.
    pub program: String,
    pub cwd: String,
}

/// Finds the pane whose shell or foreground process is an ancestor of this
/// process. `HERDR_PANE_ID` is only tried first: it can be stale, e.g. in
/// commands run by a long-lived daemon that started in another pane.
pub fn me() -> Result<Me, Failure> {
    let list = herdr_local(&["pane", "list"])?;
    let panes = list["result"]["panes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let hint = env::var("HERDR_PANE_ID").unwrap_or_default();
    // `lineage` is computed only for a pane without an agent: it costs a
    // full process table.
    let me_of = |p: &Value, info: &Value, lineage: Option<&[u32]>| -> Result<Me, Failure> {
        let program = match p["agent"].as_str() {
            Some(a) => a.to_string(),
            None => match lineage {
                Some(l) => pane_program(info, l),
                None => pane_program(info, &ancestors(&process_table()?, std::process::id())),
            },
        };
        Ok(Me {
            pane: str_of(p, "pane_id").to_string(),
            workspace: str_of(p, "workspace_id").to_string(),
            agent: p["agent"].as_str().unwrap_or("unknown").to_string(),
            program,
            cwd: collapse_home(p["cwd"].as_str().unwrap_or("?")),
        })
    };

    // Fast path: verify the hinted pane by walking up our parents one cheap
    // `ps -p` at a time. A full process table costs ~70 ms on macOS.
    if let Some(p) = panes.iter().find(|p| p["pane_id"] == hint.as_str()) {
        if let Ok(v) = herdr_local(&["pane", "process-info", "--pane", &hint]) {
            let pids = pane_pids(&v["result"]["process_info"]);
            let first = std::os::unix::process::parent_id();
            if reaches(std::process::id(), &pids, |pid| {
                if pid == std::process::id() {
                    Some(first)
                } else {
                    parent_of(pid)
                }
            }) {
                return me_of(p, &v["result"]["process_info"], None);
            }
        }
    }

    let lineage = ancestors(&process_table()?, std::process::id());
    for p in &panes {
        let id = str_of(p, "pane_id");
        let Ok(v) = herdr_local(&["pane", "process-info", "--pane", id]) else {
            continue; // the pane closed meanwhile
        };
        if hosts(&v["result"]["process_info"], &lineage) {
            return me_of(p, &v["result"]["process_info"], Some(&lineage));
        }
    }
    if let Some((p, info)) = container_pane(&panes, &lineage) {
        return me_of(&p, &info, Some(&lineage));
    }
    let message = if hint.is_empty() {
        "no herdr pane hosts this process".to_string()
    } else {
        format!("no herdr pane hosts this process (HERDR_PANE_ID={hint} is stale)")
    };
    Err(Failure::new(
        "not_in_herdr",
        message,
        "run this from an agent inside a herdr pane; codex must run with --no-daemon (`vega mux spawn` does this); in a toolbox/podman container the pane is matched by the `podman exec` that started it",
    ))
}

/// Clock ticks (10 ms each on Linux) within which `podman exec` forks its
/// conmon.
const EXEC_START_WINDOW: u64 = 100;

/// The pane that hosts us when a container boundary splits the process tree:
/// `podman exec` (e.g. via `toolbox enter`) runs in the pane, but the exec'd
/// process descends from `conmon`, a child of init. The two are linked by the
/// container name and by their start times. Linux only; `None` when not in a
/// container, or when the match is not unique.
fn container_pane(panes: &[Value], lineage: &[u32]) -> Option<(Value, Value)> {
    let names = comm_names(lineage.iter());
    let conmon = names.iter().find(|(_, n)| n == "conmon")?.0;
    let container = conmon_container(&proc_cmdline(conmon)?)?;
    let conmon_start = proc_start(conmon)?;
    let mut found: Vec<(u64, Value, Value)> = Vec::new();
    for p in panes {
        let Ok(v) = herdr_local(&["pane", "process-info", "--pane", str_of(p, "pane_id")]) else {
            continue; // the pane closed meanwhile
        };
        let info = v["result"]["process_info"].clone();
        for fp in info["foreground_processes"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let argv: Vec<&str> = fp["argv"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if !is_exec_of(&argv, &container) {
                continue;
            }
            if let Some(start) = fp["pid"].as_u64().and_then(|p| proc_start(p as u32)) {
                found.push((start.abs_diff(conmon_start), p.clone(), info.clone()));
                break;
            }
        }
    }
    let near: Vec<_> = found
        .into_iter()
        .filter(|(d, ..)| *d <= EXEC_START_WINDOW)
        .collect();
    match <[_; 1]>::try_from(near) {
        Ok([(_, p, info)]) => Some((p, info)),
        _ => None,
    }
}

/// The container name of a conmon command line (`-n <name>`).
fn conmon_container(argv: &[String]) -> Option<String> {
    let i = argv.iter().position(|a| a == "-n")?;
    argv.get(i + 1).cloned()
}

/// True if `argv` is a `podman ... exec ... <container> ...` client.
fn is_exec_of(argv: &[&str], container: &str) -> bool {
    argv.first().is_some_and(|a| a.ends_with("podman"))
        && argv.contains(&"exec")
        && argv.contains(&container)
}

fn proc_cmdline(pid: u32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|b| *b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect(),
    )
}

/// Start time in clock ticks since boot, from `/proc/<pid>/stat`.
fn proc_start(pid: u32) -> Option<u64> {
    start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// Field 22 of a `/proc/<pid>/stat` line; the comm field may hold spaces.
fn start_ticks(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// The program this process runs under in a pane without a detected agent.
/// `lineage` is our pid and its ancestors, nearest first.
fn pane_program(info: &Value, lineage: &[u32]) -> String {
    let shell = info["shell_pid"].as_u64().map(|p| p as u32);
    let below = match shell.and_then(|s| lineage.iter().position(|&p| p == s)) {
        Some(i) => &lineage[..i],
        None => &[][..],
    };
    let names = comm_names(below.iter().chain(shell.as_ref()));
    let name_of = |pid: &u32| {
        names
            .iter()
            .find(|(p, _)| p == pid)
            .map(|(_, n)| n.as_str())
    };
    let chain: Vec<&str> = below.iter().rev().filter_map(name_of).collect();
    let shell_name = shell.as_ref().and_then(name_of).unwrap_or("unknown");
    program_name(&chain, shell_name)
}

/// The first program below the shell that is not vega itself, else the
/// shell. `chain` is outermost first, as from `ps -o comm=`.
fn program_name(chain: &[&str], shell: &str) -> String {
    let base = |c: &str| {
        let b = c.rsplit('/').next().unwrap_or(c);
        b.trim_start_matches('-').to_string()
    };
    chain
        .iter()
        .map(|c| base(c))
        .find(|n| !n.is_empty() && !n.starts_with("vega"))
        .unwrap_or_else(|| base(shell))
}

/// `pid -> command name` for the given pids, from one `ps` call.
fn comm_names<'a>(pids: impl Iterator<Item = &'a u32>) -> Vec<(u32, String)> {
    let list = pids.map(u32::to_string).collect::<Vec<_>>().join(",");
    if list.is_empty() {
        return Vec::new();
    }
    let Ok(out) = Command::new("ps")
        .args(["-o", "pid=,comm=", "-p", &list])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (pid, comm) = l.trim().split_once(char::is_whitespace)?;
            Some((pid.parse().ok()?, comm.trim().to_string()))
        })
        .collect()
}

/// The pane's shell and foreground pids.
fn pane_pids(info: &Value) -> Vec<u32> {
    let mut pids: Vec<u32> = info["foreground_processes"]
        .as_array()
        .map(|ps| {
            ps.iter()
                .filter_map(|p| p["pid"].as_u64())
                .map(|p| p as u32)
                .collect()
        })
        .unwrap_or_default();
    if let Some(shell) = info["shell_pid"].as_u64() {
        pids.push(shell as u32);
    }
    pids
}

/// True if `pid` or one of its ancestors is in `targets`. Stops at init, on a
/// cycle, or after 64 steps.
fn reaches(pid: u32, targets: &[u32], parent: impl Fn(u32) -> Option<u32>) -> bool {
    let mut cur = pid;
    for _ in 0..64 {
        if targets.contains(&cur) {
            return true;
        }
        match parent(cur) {
            Some(p) if p > 1 && p != cur => cur = p,
            _ => return false,
        }
    }
    false
}

/// Parent pid of one process, from `ps -o ppid= -p`.
fn parent_of(pid: u32) -> Option<u32> {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// `pid -> ppid` for all processes, from `ps`.
fn process_table() -> Result<Vec<(u32, u32)>, Failure> {
    let out = Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .map_err(|e| Failure::new("ps_failed", e.to_string(), "check that `ps` is on PATH"))?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace().map(str::parse::<u32>);
            Some((it.next()?.ok()?, it.next()?.ok()?))
        })
        .collect())
}

/// `pid` and all its ancestors, nearest first.
fn ancestors(table: &[(u32, u32)], pid: u32) -> Vec<u32> {
    let mut lineage = vec![pid];
    let mut cur = pid;
    while let Some(&(_, ppid)) = table.iter().find(|(p, _)| *p == cur) {
        if ppid <= 1 || lineage.contains(&ppid) {
            break;
        }
        lineage.push(ppid);
        cur = ppid;
    }
    lineage
}

/// True if the pane's shell or one of its foreground processes is in `lineage`.
fn hosts(info: &Value, lineage: &[u32]) -> bool {
    let in_lineage = |v: &Value| v.as_u64().is_some_and(|p| lineage.contains(&(p as u32)));
    in_lineage(&info["shell_pid"])
        || info["foreground_processes"]
            .as_array()
            .is_some_and(|ps| ps.iter().any(|p| in_lineage(&p["pid"])))
}

pub fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("-")
}

/// Quotes a TOON cell when it contains a delimiter or quote.
pub fn cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\\\"").replace('\n', " "))
    } else {
        s.to_string()
    }
}

pub fn collapse_home(path: &str) -> String {
    match env::var("HOME") {
        Ok(home) if path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// `/Users/<u>/x` or `/home/<u>/x` -> `~/x`, so paths compare across machines
/// whose home dirs differ.
pub fn home_relative(path: &str) -> String {
    let local = collapse_home(path);
    if local.starts_with('~') {
        return local;
    }
    for prefix in ["/Users/", "/home/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return match rest.split_once('/') {
                Some((_, tail)) => format!("~/{tail}"),
                None => "~".to_string(),
            };
        }
    }
    path.to_string()
}

/// `path` with symlinks resolved, or as given if it does not exist.
pub fn canonical(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// Project membership compares v2 IDs only. Callers resolve local metadata
/// first; registry consumers use the owner's published project and host.
pub fn in_project(project: &str, record: &Value) -> bool {
    record["schema"] == 2 && record["project"].as_str() == Some(project)
}

/// Status order for agent lists: what needs attention first.
pub const STATUS_ORDER: [&str; 5] = ["blocked", "working", "done", "idle", "unknown"];

pub fn status_rank(status: &str) -> usize {
    STATUS_ORDER
        .iter()
        .position(|s| *s == status)
        .unwrap_or(STATUS_ORDER.len())
}

/// Aggregate like `1 blocked, 2 idle`, in attention order.
pub fn status_counts(agents: &[&Value]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for a in agents {
        let s = str_of(a, "agent_status");
        match counts.iter_mut().find(|(k, _)| *k == s) {
            Some((_, n)) => *n += 1,
            None => counts.push((s, 1)),
        }
    }
    counts.sort_by_key(|(s, _)| status_rank(s));
    let line = counts
        .iter()
        .map(|(s, n)| format!("{n} {s}"))
        .collect::<Vec<_>>()
        .join(", ");
    // herdr: both mean ready for input; they differ only in whether a human
    // looked at the pane since the agent finished.
    if counts.iter().any(|(s, _)| *s == "done" || *s == "idle") {
        format!("{line} (done: finished, not yet viewed; idle: viewed)")
    } else {
        line
    }
}

/// The herdr agent name set by `vega mux spawn`, or `-`.
pub fn agent_name(a: &Value) -> &str {
    match a["name"].as_str() {
        Some(n) if !n.is_empty() => n,
        _ => "-",
    }
}

/// `w3V:p1 (claude, name rev, ~/src/x)`. Without a detected agent, `program`
/// is what runs in the pane, e.g. `w3V:p1B (zsh, ~/src/x)`.
pub fn me_line(pane: &str, program: &str, name: &str, cwd: &str) -> String {
    let name = match name {
        "-" => String::new(),
        n => format!(", name {}", cell(n)),
    };
    format!("{pane} ({program}{name}, {})", cell(cwd))
}

/// Sets the herdr machine for all later herdr calls; call once at startup.
/// This machine's own host name means local: `vega uac` prints it as the
/// machine of local agents, and herdr knows no machine by that label.
pub fn set_machine(m: Option<String>) {
    let m = m.filter(|l| !l.eq_ignore_ascii_case(&local_host()));
    let _ = MACHINE.set(m);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn in_project_compares_ids_and_never_interprets_locations() {
        let a = json!({"schema": 2, "project": "github.com/team/repo", "repo_key": "/one/.git"});
        let b = json!({"schema": 2, "project": "github.com/team/repo", "cwd": "/different/clone"});
        assert!(in_project("github.com/team/repo", &a));
        assert!(in_project("github.com/team/repo", &b));
        assert!(!in_project("github.com/team/fork", &a));
        assert!(!in_project(
            "github.com/team/repo",
            &json!({"repo_key": "github.com/team/repo"})
        ));
    }

    #[test]
    fn qualify_prefixes_other_machines_only() {
        assert_eq!(qualify(Some("io"), "w1:p2"), "io:w1:p2");
        assert_eq!(qualify(None, "w1:p2"), "w1:p2");
    }

    #[test]
    fn status_counts_in_attention_order() {
        let a = [
            json!({"agent_status": "idle"}),
            json!({"agent_status": "blocked"}),
            json!({"agent_status": "idle"}),
        ];
        let refs: Vec<&Value> = a.iter().collect();
        assert_eq!(
            status_counts(&refs),
            "1 blocked, 2 idle (done: finished, not yet viewed; idle: viewed)"
        );
        let busy = [json!({"agent_status": "working"})];
        let refs: Vec<&Value> = busy.iter().collect();
        assert_eq!(status_counts(&refs), "1 working");
    }

    #[test]
    fn me_line_names_the_agent_or_the_program() {
        assert_eq!(
            me_line("w3V:p1C", "claude", "mux-list", "~/src/v"),
            "w3V:p1C (claude, name mux-list, ~/src/v)"
        );
        assert_eq!(
            me_line("w3V:p1", "codex", "-", "~/src/v"),
            "w3V:p1 (codex, ~/src/v)"
        );
        assert_eq!(
            me_line("w3V:p1B", "zsh", "-", "~/src/v"),
            "w3V:p1B (zsh, ~/src/v)"
        );
    }

    #[test]
    fn ancestors_walk_up_to_init_and_stop_on_cycles() {
        let table = [(50, 40), (40, 30), (30, 1), (7, 8), (8, 7)];
        assert_eq!(ancestors(&table, 50), vec![50, 40, 30]);
        assert_eq!(ancestors(&table, 7), vec![7, 8]);
        assert_eq!(ancestors(&table, 99), vec![99]);
    }

    #[test]
    fn pane_hosts_process_via_shell_or_foreground_agent() {
        let info = json!({"shell_pid": 30, "foreground_processes": [{"pid": 40}]});
        assert!(hosts(&info, &[50, 40]));
        assert!(hosts(&info, &[50, 30]));
        // A daemon outside the pane: no pid of the pane is an ancestor.
        assert!(!hosts(&info, &[50, 60, 70]));
    }
    #[test]
    fn reaches_walks_parents_until_a_pane_pid() {
        let table = [(50, 40), (40, 30), (30, 1)];
        let parent = |pid| table.iter().find(|(p, _)| *p == pid).map(|(_, pp)| *pp);
        assert!(reaches(50, &[30], parent));
        assert!(reaches(50, &[50], parent));
        assert!(!reaches(50, &[99], parent));
        assert!(!reaches(7, &[99], |_| Some(7))); // cycle
    }

    #[test]
    fn pane_pids_has_shell_and_foreground() {
        let info = json!({"shell_pid": 30, "foreground_processes": [{"pid": 40}]});
        assert_eq!(pane_pids(&info), vec![40, 30]);
    }

    #[test]
    fn program_name_skips_vega_and_falls_back_to_the_shell() {
        // `vega mux` typed into a shell.
        assert_eq!(program_name(&["vega", "/x/vega-mux"], "-zsh"), "zsh");
        // An agent herdr does not detect, running vega via its shell tool.
        assert_eq!(
            program_name(&["opencode", "/bin/zsh", "vega"], "-zsh"),
            "opencode"
        );
        assert_eq!(program_name(&[], "/bin/bash"), "bash");
    }

    #[test]
    fn start_ticks_survives_spaces_in_comm() {
        let stat =
            "42 (my (odd) prog) S 1 42 42 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1 2";
        assert_eq!(start_ticks(stat), Some(987654));
        assert_eq!(start_ticks("garbage"), None);
    }

    #[test]
    fn conmon_names_its_container() {
        let argv: Vec<String> = ["/usr/bin/conmon", "-c", "abc", "-n", "code", "-t"]
            .map(String::from)
            .to_vec();
        assert_eq!(conmon_container(&argv).as_deref(), Some("code"));
        assert_eq!(conmon_container(&argv[..3]), None);
    }

    #[test]
    fn podman_exec_client_matches_by_container_name() {
        let exec = [
            "podman",
            "--log-level",
            "error",
            "exec",
            "--tty",
            "code",
            "zsh",
        ];
        assert!(is_exec_of(&exec, "code"));
        assert!(!is_exec_of(&exec, "other"));
        assert!(!is_exec_of(&["toolbox", "enter", "code"], "code"));
        assert!(!is_exec_of(&["podman", "ps", "code"], "code"));
    }

    #[test]
    fn cell_quotes_delimiters() {
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(cell("plain"), "plain");
    }
}
