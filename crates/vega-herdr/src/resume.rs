//! In-place resume of agents whose kept hc terminal came back as a shell
//! after a reboot (docs/design/hc-migration-inventory.md, "Resume after a
//! reboot"). hc keeps the terminal (address, name, labels, parent) but never
//! reruns its command; vega types the harness resume command into the shell.
use crate::state::{spawn_records, SpawnRecord};
use crate::Failure;
use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};

/// Shells whose syntax `command_line` uses (`K=V cmd`, `&&`).
const SHELLS: &[&str] = &["zsh", "bash", "sh", "dash", "ksh", "fish"];

/// What vegad does with lost agents after a boot (`resume.toml` `mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Offer,
    Auto,
}

/// `$XDG_CONFIG_HOME/vega/resume.toml`, `mode = "offer" | "auto" | "off"`;
/// missing or unreadable means offer.
pub fn mode() -> Mode {
    config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("resume.toml")).ok())
        .map(|t| parse_mode(&t))
        .unwrap_or(Mode::Offer)
}

pub fn parse_mode(text: &str) -> Mode {
    let doc: toml_edit::DocumentMut = match text.parse() {
        Ok(d) => d,
        Err(_) => return Mode::Offer,
    };
    match doc.get("mode").and_then(|v| v.as_str()) {
        Some("auto") => Mode::Auto,
        Some("off") => Mode::Off,
        _ => Mode::Offer,
    }
}

fn config_dir() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
        .map(|d| d.join("vega"))
}

/// The harness arguments that resume `session`.
pub fn resume_args(kind: &str, session: Option<&str>) -> Result<Vec<String>, Failure> {
    let flag = match kind {
        "codex" => "resume",
        "claude" => "--resume",
        "pi" => "--session",
        _ => return Err(Failure::new(
            "unsupported_kind",
            format!("reopen does not support {kind}"),
            "start a new agent with `vega mux spawn <name> --kind claude|codex|pi --tier balanced`",
        )),
    };
    let session = session.filter(|s| !s.is_empty()).ok_or_else(|| {
        Failure::new(
            "no_session",
            "no agent session was recorded for this agent",
            "start a new agent with `vega mux spawn <name> --kind claude|codex|pi --tier balanced`",
        )
    })?;
    Ok(vec![flag.into(), session.into()])
}

/// A live terminal whose foreground is a plain shell with nothing running in it.
pub fn shell_idle(t: &Value) -> bool {
    let fg = &t["foreground"];
    let program = fg["argv"][0]
        .as_str()
        .or(fg["name"].as_str())
        .unwrap_or("")
        .trim_start_matches('-');
    let base = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let pid = fg["pid"].as_u64();
    t["state"] == "live"
        && SHELLS.contains(&base)
        && pid.is_some()
        && pid == t["pid"].as_u64()
        && t["status"]["app"].as_str().is_none_or(str::is_empty)
        && !matches!(t["status"]["state"].as_str(), Some("working" | "blocked"))
}

fn pi_dir(cwd: &str) -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    let folder = format!("--{}--", cwd.trim_start_matches('/').replace('/', "-"));
    Some(Path::new(&home).join(".pi/agent/sessions").join(folder))
}

/// pi sessions of `cwd`: (id, start in Unix s, modified), from each file's header.
fn pi_sessions(cwd: &str) -> Vec<(String, i64, std::time::SystemTime)> {
    let Some(dir) = pi_dir(cwd) else {
        return vec![];
    };
    let mut out = vec![];
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(head) = text
            .lines()
            .next()
            .and_then(|l| serde_json::from_str::<Value>(l).ok())
        else {
            continue;
        };
        if head["type"] != "session" || head["cwd"] != cwd {
            continue;
        }
        let (Some(id), Some(at)) = (
            head["id"].as_str(),
            head["timestamp"]
                .as_str()
                .and_then(crate::capacity::parse_rfc3339),
        ) else {
            continue;
        };
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        out.push((id.to_string(), at, modified));
    }
    out
}

/// The pi session a spawn started: the first one in its cwd at or after its
/// launch. Later sessions there are other pi runs (e.g. game sims).
pub fn pi_session_since(cwd: &str, started_ms: u64) -> Option<String> {
    let since = (started_ms / 1000) as i64 - 5;
    pi_sessions(cwd)
        .into_iter()
        .filter(|(_, at, _)| *at >= since)
        .min_by_key(|(_, at, _)| *at)
        .map(|(id, _, _)| id)
}

/// The pi session being written now in `cwd`: the calling pi's own one.
pub fn pi_session_current(cwd: &str) -> Option<String> {
    pi_sessions(cwd)
        .into_iter()
        .max_by_key(|(_, _, modified)| *modified)
        .map(|(id, _, _)| id)
}

/// The recorded session, or for pi the session file its launch started.
pub fn session_of(r: &SpawnRecord) -> Option<String> {
    r.session.clone().filter(|s| !s.is_empty()).or_else(|| {
        (r.kind == "pi")
            .then_some(r.started_ms)
            .flatten()
            .and_then(|at| pi_session_since(&r.cwd, at))
    })
}

/// Model and provider choices of a hand-started harness argv (no task, no
/// resume flags): an in-place resume repeats them after the default args.
pub fn passthrough(argv: &[String]) -> Vec<String> {
    const VALUED: &[&str] = &[
        "--model",
        "-m",
        "--provider",
        "--thinking",
        "--permission-mode",
        "--effort",
    ];
    let mut out = vec![];
    let mut i = 1;
    while i < argv.len() {
        let a = argv[i].as_str();
        if a.split_once('=').is_some_and(|(f, _)| VALUED.contains(&f)) {
            out.push(a.to_string());
        } else if VALUED.contains(&a) {
            if let Some(v) = argv.get(i + 1) {
                out.extend([a.to_string(), v.clone()]);
                i += 1;
            }
        }
        i += 1;
    }
    out
}

fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
    {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// `cd <cwd> && K=V ... <harness> <args> <resume>`, typed into the shell. The recorded argv is
/// the harness argv; on a host with the toolbox launcher the line is wrapped here, as spawn does
/// (`<launcher> env K=V ... <harness> ...`), so the resumed agent runs in the toolbox too.
pub fn command_line(r: &SpawnRecord, session: &str) -> Result<String, Failure> {
    command_line_with(r, session, crate::launch::toolbox_launcher().as_deref())
}

fn command_line_with(
    r: &SpawnRecord,
    session: &str,
    launcher: Option<&str>,
) -> Result<String, Failure> {
    let mut argv = if r.argv.is_empty() {
        let mut a = vec![r.kind.clone()];
        a.extend(
            crate::launch::default_args(&r.kind)
                .iter()
                .map(|s| s.to_string()),
        );
        a.extend(r.extra.iter().cloned());
        a
    } else {
        r.argv.clone()
    };
    argv.extend(resume_args(&r.kind, Some(session))?);
    let mut words = vec![];
    if let Some(l) = launcher {
        words.extend([quote(l), "env".into()]);
    }
    for var in &r.env {
        if let Some((k, v)) = var.split_once('=') {
            if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                words.push(format!("{k}={}", quote(v)));
            }
        }
    }
    words.extend(argv.iter().map(|a| quote(a)));
    Ok(format!("cd {} && {}", quote(&r.cwd), words.join(" ")))
}

/// The record of a terminal's current occupant (same address and name).
fn record_of<'a>(t: &Value, records: &'a [SpawnRecord]) -> Option<&'a SpawnRecord> {
    let address = t["address"].as_str()?;
    let name = t["name"].as_str().unwrap_or("");
    records
        .iter()
        .find(|r| r.backend == "hc" && r.pane == address && r.name == name)
}

/// Lost agents in a `terminal.list` result: local kept terminals at an idle
/// shell whose record has a session. Each with its session.
pub fn candidates(list: &Value, records: &[SpawnRecord]) -> Vec<(SpawnRecord, String)> {
    list["terminals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| {
            t["address"].as_str().is_some_and(crate::pane::is_hc_id)
                && t["keep"] == true
                && shell_idle(t)
        })
        .filter_map(|t| record_of(t, records))
        .filter_map(|r| session_of(r).map(|s| (r.clone(), s)))
        .collect()
}

/// Type the resume command into `r`'s terminal after re-checking that it
/// still is the same terminal at an idle shell.
pub fn resume_in_place(r: &SpawnRecord, session: &str) -> Result<String, Failure> {
    let line = command_line(r, session)?;
    if !Path::new(&r.cwd).is_dir() {
        return Err(Failure::new(
            "cwd_gone",
            format!("recorded cwd {} no longer exists", r.cwd),
            "restore the directory or worktree before reopening",
        ));
    }
    let t = crate::hc::hc_local(&["get", &r.pane])?;
    if t["name"].as_str().unwrap_or("") != r.name || !shell_idle(&t) {
        return Err(Failure::new(
            "not_idle_shell",
            format!("{} is not at an idle shell", r.pane),
            &format!("inspect `vega mux show {}`", r.pane),
        ));
    }
    // No caller: a shell gets the bare line, never a tagged message.
    crate::hc::rpc(
        "terminal.send",
        serde_json::json!({"terminal": r.pane, "text": line}),
        crate::DAEMON_CALL_TIMEOUT,
    )?;
    Ok(line)
}

/// The record and session for a kept terminal of this machine at an idle
/// shell, for `vega mux reopen <t>`; `None` when it is something else.
pub fn lost(target: &str) -> Option<(SpawnRecord, String)> {
    let t = crate::hc::hc_local(&["get", target]).ok()?;
    if !shell_idle(&t) {
        return None;
    }
    let records = spawn_records();
    let r = record_of(&t, &records)?;
    session_of(r).map(|s| (r.clone(), s))
}

/// `vega uac claim`: record the caller's own hc terminal (a hand-started UAC
/// has no spawn record) so a reboot can resume it. Returns a status line.
pub fn record_own(pane: &str) -> Option<String> {
    let crate::Terminal::Hc(id) = crate::own_terminal().ok()? else {
        return None;
    };
    if id != pane {
        return None;
    }
    let t = crate::hc::hc_local(&["get", &id]).ok()?;
    let argv: Vec<String> = t["foreground"]["argv"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let program = argv.first().map(|a| {
        Path::new(a)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string()
    });
    let kind = match t["status"]["app"].as_str().filter(|s| !s.is_empty()) {
        Some("claude-code") => "claude".to_string(),
        Some(k) => k.to_string(),
        None => program.unwrap_or_default(),
    };
    if !["claude", "codex", "pi"].contains(&kind.as_str()) {
        return None;
    }
    let cwd = t["foreground"]["cwd"]
        .as_str()
        .or(t["cwd"].as_str())
        .unwrap_or("")
        .to_string();
    let session = match kind.as_str() {
        "claude" => env::var("CLAUDE_CODE_SESSION_ID").ok(),
        "codex" => env::var("CODEX_THREAD_ID").ok(),
        _ => pi_session_current(&cwd),
    }
    .filter(|s| !s.is_empty());
    let name = t["name"].as_str().unwrap_or("").to_string();
    let mut r = crate::state::spawn_of_on(None, &id, &name, &kind).unwrap_or_else(|| SpawnRecord {
        backend: "hc".into(),
        pane: id.clone(),
        name,
        kind: kind.clone(),
        cwd,
        mode: "headed".into(),
        extra: passthrough(&argv),
        env: env::var("BEADS_ACTOR")
            .ok()
            .filter(|a| !a.is_empty())
            .map(|a| format!("BEADS_ACTOR={a}"))
            .into_iter()
            .collect(),
        ..Default::default()
    });
    if session.is_some() {
        r.session = session;
    }
    crate::state::save_spawn(&r).ok()?;
    Some(match &r.session {
        Some(s) => format!("resume: recorded ({kind}, session {s})"),
        None => format!("resume: recorded ({kind}, no session yet)"),
    })
}

/// This boot, as a string; `VEGA_BOOT_EPOCH` overrides it (tests).
pub fn boot_epoch() -> Option<String> {
    if let Some(e) = env::var("VEGA_BOOT_EPOCH").ok().filter(|s| !s.is_empty()) {
        return Some(e);
    }
    if let Ok(stat) = std::fs::read_to_string("/proc/stat") {
        return stat
            .lines()
            .find_map(|l| l.strip_prefix("btime "))
            .map(|s| s.trim().to_string());
    }
    let out = std::process::Command::new("sysctl")
        .args(["-n", "kern.boottime"])
        .output()
        .ok()?;
    // `{ sec = 1794216504, usec = 290222 } Mon Nov  9 10:28:24 2026`
    let text = String::from_utf8_lossy(&out.stdout);
    let sec = text.split("sec = ").nth(1)?.split(',').next()?.trim();
    (!sec.is_empty()).then(|| sec.to_string())
}

fn epoch_file() -> Option<PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))
        .map(|d| d.join("vega/resume/epoch"))
}

/// What one boot pass did.
#[derive(Debug, Default, PartialEq)]
pub struct Pass {
    /// Terminals typed into (auto).
    pub resumed: Vec<String>,
    /// Terminals offered in a note (offer).
    pub offered: Vec<String>,
    /// `<terminal>: <reason>` for those that failed.
    pub failed: Vec<String>,
}

/// One resume pass per boot. `Ok(None)`: this boot was already handled.
/// An error (hc unreachable) leaves the boot unhandled for a retry.
pub fn boot_pass(mode: Mode) -> Result<Option<Pass>, Failure> {
    let Some(epoch) = boot_epoch() else {
        return Ok(None);
    };
    let file = epoch_file();
    if file
        .as_ref()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .is_some_and(|e| e.trim() == epoch)
    {
        return Ok(None);
    }
    let pass = if mode == Mode::Off {
        Pass::default()
    } else {
        let list = crate::hc::terminal_list()?;
        run_pass(mode, &list, &spawn_records())
    };
    if let Some(f) = file {
        let _ = f.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(f, &epoch);
    }
    Ok(Some(pass))
}

fn run_pass(mode: Mode, list: &Value, records: &[SpawnRecord]) -> Pass {
    let mut pass = Pass::default();
    let found = candidates(list, records);
    match mode {
        Mode::Off => {}
        Mode::Auto => {
            for (r, session) in found {
                match resume_in_place(&r, &session) {
                    Ok(_) => pass.resumed.push(r.pane),
                    Err(f) => pass.failed.push(format!("{}: {}", r.pane, f.message)),
                }
            }
        }
        Mode::Offer => {
            pass.offered = found.into_iter().map(|(r, _)| r.pane).collect();
            if !pass.offered.is_empty() {
                offer(&pass.offered);
            }
        }
    }
    pass
}

/// One note for the Slayer naming the command that resumes them all.
fn offer(panes: &[String]) {
    let Some(store) = crate::notes::Store::local() else {
        return;
    };
    let cmd = format!("vega mux reopen {}", panes.join(" "));
    let note = crate::notes::Note {
        host: crate::state::post_host(),
        kind: "resume".into(),
        level: Some("action".into()),
        title: format!("{} agent(s) lost in the reboot", panes.len()),
        body: format!("resume them in place: {cmd}"),
        cmd: Some(cmd),
        ..Default::default()
    };
    let _ = store.add(note, crate::notes::now_ms());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shell(address: &str, name: Option<&str>) -> Value {
        json!({"address":address,"name":name,"state":"live","keep":true,"pid":10,
            "argv":["claude","--dangerously-skip-permissions"],
            "foreground":{"argv":["-zsh"],"name":"zsh","pid":10,"cwd":"/home"},"status":null})
    }

    fn record(kind: &str, pane: &str, session: Option<&str>) -> SpawnRecord {
        SpawnRecord {
            backend: "hc".into(),
            pane: pane.into(),
            name: "fix".into(),
            kind: kind.into(),
            cwd: "/work tree".into(),
            mode: "headed".into(),
            session: session.map(String::from),
            extra: vec!["--model".into(), "m1".into()],
            ..Default::default()
        }
    }

    #[test]
    fn resume_cli_syntax_per_kind() {
        assert_eq!(resume_args("claude", Some("u")).unwrap(), ["--resume", "u"]);
        assert_eq!(resume_args("codex", Some("u")).unwrap(), ["resume", "u"]);
        assert_eq!(resume_args("pi", Some("u")).unwrap(), ["--session", "u"]);
        assert_eq!(
            resume_args("agy", Some("u")).unwrap_err().code,
            "unsupported_kind"
        );
        assert_eq!(
            resume_args("claude", Some("")).unwrap_err().code,
            "no_session"
        );
    }

    #[test]
    fn command_lines_repeat_defaults_args_env_and_resume() {
        let mut r = record("claude", "t1", Some("s1"));
        r.env = vec!["BEADS_ACTOR=proteus/fix".into(), "BAD-KEY=x".into()];
        assert_eq!(
            command_line_with(&r, "s1", None).unwrap(),
            "cd '/work tree' && BEADS_ACTOR=proteus/fix claude --dangerously-skip-permissions --model m1 --resume s1"
        );
        let r = record("codex", "t1", None);
        assert_eq!(
            command_line_with(&r, "c1", None).unwrap(),
            "cd '/work tree' && codex --no-daemon --dangerously-bypass-approvals-and-sandbox -c check_for_update_on_startup=false --model m1 resume c1"
        );
        let mut r = record("pi", "t1", None);
        r.extra = vec![
            "--provider".into(),
            "zai".into(),
            "--model".into(),
            "glm".into(),
        ];
        assert_eq!(
            command_line_with(&r, "p1", None).unwrap(),
            "cd '/work tree' && pi --approve --provider zai --model glm --session p1"
        );
        // A recorded launch argv wins over defaults + extra.
        let mut r = record("codex", "t1", None);
        r.argv = vec![
            "codex".into(),
            "-c".into(),
            "shell_environment_policy.inherit=\"all\"".into(),
        ];
        assert_eq!(
            command_line_with(&r, "c1", None).unwrap(),
            r#"cd '/work tree' && codex -c 'shell_environment_policy.inherit="all"' resume c1"#
        );
    }

    #[test]
    fn launcher_wraps_the_resume_line_and_leaves_the_record_alone() {
        let mut r = record("claude", "t1", Some("s1"));
        r.env = vec!["BEADS_ACTOR=proteus/fix".into()];
        r.argv = vec!["claude".into(), "--x".into()];
        assert_eq!(
            command_line_with(&r, "s1", Some("/h/.local/bin/hc-toolbox-shell")).unwrap(),
            "cd '/work tree' && /h/.local/bin/hc-toolbox-shell env BEADS_ACTOR=proteus/fix claude --x --resume s1"
        );
        assert_eq!(
            command_line_with(&r, "s1", None).unwrap(),
            "cd '/work tree' && BEADS_ACTOR=proteus/fix claude --x --resume s1"
        );
        // The record keeps the harness argv: no launcher in it.
        assert_eq!(r.argv, ["claude", "--x"]);
    }

    #[test]
    fn idle_shell_only() {
        assert!(shell_idle(&shell("t1", None)));
        let mut child = shell("t1", None);
        child["foreground"]["pid"] = 11.into();
        assert!(!shell_idle(&child), "a program runs in the shell");
        let mut agent = shell("t1", None);
        agent["foreground"]["argv"] = json!(["claude"]);
        assert!(!shell_idle(&agent));
        let mut nu = shell("t1", None);
        nu["foreground"]["argv"] = json!(["/opt/bin/nu"]);
        assert!(!shell_idle(&nu), "no POSIX syntax");
        let mut app = shell("t1", None);
        app["status"] = json!({"app":"claude-code","state":"idle"});
        assert!(!shell_idle(&app));
        let mut bash = shell("t1", None);
        bash["foreground"]["argv"] = json!(["/bin/bash", "-l"]);
        assert!(shell_idle(&bash));
    }

    #[test]
    fn candidates_need_keep_record_name_and_session() {
        let list = json!({"terminals":[
            shell("t1", Some("fix")),
            shell("t2", Some("fix")),
            shell("t3", Some("other")),
            shell("vps/t4", Some("fix")),
            {"address":"t5","name":"fix","state":"live","keep":false,"pid":1,
             "foreground":{"argv":["zsh"],"pid":1}},
        ]});
        let records = vec![
            record("claude", "t1", Some("s1")),
            record("codex", "t2", None),
            record("claude", "t3", Some("s3")),
            record("claude", "vps/t4", Some("s4")),
            record("claude", "t5", Some("s5")),
        ];
        let found = candidates(&list, &records);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].0.pane.as_str(), found[0].1.as_str()),
            ("t1", "s1")
        );
        let mut herdr = record("claude", "t1", Some("s1"));
        herdr.backend = "herdr".into();
        assert!(candidates(&list, &[herdr]).is_empty());
    }

    #[test]
    fn passthrough_keeps_model_choices_only() {
        let argv: Vec<String> = [
            "claude",
            "--dangerously-skip-permissions",
            "--model",
            "opus",
            "--resume",
            "old",
            "--permission-mode=bypassPermissions",
            "a task",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            passthrough(&argv),
            ["--model", "opus", "--permission-mode=bypassPermissions"]
        );
        let pi: Vec<String> = [
            "pi",
            "--provider",
            "zai",
            "--model",
            "glm-5.3",
            "--session",
            "x",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            passthrough(&pi),
            ["--provider", "zai", "--model", "glm-5.3"]
        );
    }

    #[test]
    fn mode_defaults_to_offer() {
        assert_eq!(parse_mode(""), Mode::Offer);
        assert_eq!(parse_mode("mode = \"auto\""), Mode::Auto);
        assert_eq!(parse_mode("mode = \"off\""), Mode::Off);
        assert_eq!(parse_mode("mode = \"bogus\""), Mode::Offer);
        assert_eq!(parse_mode("not toml ="), Mode::Offer);
    }

    #[test]
    fn pi_sessions_pick_the_spawns_own_file() {
        if env::var_os("VEGA_RESUME_PI_TEST").is_none() {
            let home = tempfile::tempdir().unwrap();
            let out = std::process::Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "resume::tests::pi_sessions_pick_the_spawns_own_file",
                ])
                .env("VEGA_RESUME_PI_TEST", "1")
                .env("HOME", home.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            return;
        }
        let cwd = "/Users/x/src/game";
        let dir = pi_dir(cwd).unwrap();
        assert!(dir.ends_with(".pi/agent/sessions/--Users-x-src-game--"));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, id: &str, ts: &str, cwd: &str| {
            std::fs::write(
                dir.join(name),
                format!(
                    "{}\n",
                    json!({"type":"session","version":3,"id":id,"timestamp":ts,"cwd":cwd})
                ),
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        write("a.jsonl", "old", "2026-11-12T07:00:00.000Z", cwd);
        write("b.jsonl", "own", "2026-11-12T08:00:01.000Z", cwd);
        write("c.jsonl", "sim", "2026-11-12T08:05:00.000Z", cwd);
        write("d.jsonl", "elsewhere", "2026-11-12T08:00:00.500Z", "/other");
        let start = crate::capacity::parse_rfc3339("2026-11-12T08:00:00Z").unwrap() as u64 * 1000;
        assert_eq!(pi_session_since(cwd, start).as_deref(), Some("own"));
        assert_eq!(pi_session_current(cwd).as_deref(), Some("sim"));
        let mut r = record("pi", "t1", None);
        r.cwd = cwd.into();
        r.started_ms = Some(start);
        assert_eq!(session_of(&r).as_deref(), Some("own"));
    }
}
