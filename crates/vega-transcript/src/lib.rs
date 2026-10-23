//! Reads a coding agent's native session log as a list of transcript entries:
//! what the user prompted, what the agent replied, which tools it called, and
//! provider errors. The log is the receipt; the terminal screen is not.
//!
//! Stable API: [`Kind`], [`locate`], [`read`] / [`parse`], [`Entry`], [`Role`].
//! No herdr dependency: the caller passes the agent kind, herdr's
//! `agent_session` value and cwd.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Agent kinds with a known session log format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Claude,
    Codex,
    Pi,
    Agy,
}

impl Kind {
    /// From a herdr agent kind (`agent list`'s `agent`, or `agent_session.source`
    /// without its `herdr:` prefix).
    pub fn from_herdr(kind: &str) -> Option<Kind> {
        match kind.trim_start_matches("herdr:") {
            "claude" => Some(Kind::Claude),
            "codex" => Some(Kind::Codex),
            "pi" => Some(Kind::Pi),
            "agy" | "antigravity" | "antigravity_cli" => Some(Kind::Agy),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
            Kind::Pi => "pi",
            Kind::Agy => "agy",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A prompt typed or pasted into the agent (injected context is dropped).
    User,
    /// Text the agent replied with (thinking is dropped).
    Assistant,
    /// One tool call, summarised on one line: `Bash: cargo test`.
    Tool,
    /// A provider or agent error, e.g. a 429 or `Not logged in`.
    Error,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub role: Role,
    /// Unix seconds, when the log has a timestamp.
    pub time: Option<i64>,
    pub text: String,
}

/// Why no session file was found.
#[derive(Debug, PartialEq, Eq)]
pub enum LocateError {
    /// No file; `looked` names the paths or dirs searched.
    NotFound { looked: Vec<PathBuf> },
    /// Several files fit and nothing picks one; newest first.
    Ambiguous { candidates: Vec<PathBuf> },
}

/// How many candidates an ambiguous lookup reports, and how many codex
/// rollouts (newest first) it scans for a matching cwd.
const MAX_CANDIDATES: usize = 5;
const MAX_CODEX_SCAN: usize = 300;

/// The session file of an agent. `session` is herdr's `agent_session.value`
/// (an id, or a path for pi) or a user override; `None` falls back to the
/// files of `cwd`, which is ambiguous when there are several. `home` is the
/// agent's `$HOME`.
pub fn locate(
    kind: Kind,
    session: Option<&str>,
    cwd: &str,
    home: &Path,
) -> Result<PathBuf, LocateError> {
    let session = session.map(str::trim).filter(|s| !s.is_empty());
    if let Some(s) = session.filter(|s| s.contains('/')) {
        let p = PathBuf::from(s);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(LocateError::NotFound { looked: vec![p] })
        };
    }
    match kind {
        Kind::Claude => {
            let projects = home.join(".claude/projects");
            let dir = projects.join(claude_slug(cwd));
            match session {
                Some(id) => {
                    let name = format!("{id}.jsonl");
                    let direct = dir.join(&name);
                    if direct.is_file() {
                        return Ok(direct);
                    }
                    // The project dir is the cwd Claude started in; the pane's
                    // cwd may have moved since.
                    let found = files_in(&projects)
                        .into_iter()
                        .filter(|d| d.is_dir())
                        .map(|d| d.join(&name))
                        .find(|p| p.is_file());
                    found.ok_or(LocateError::NotFound {
                        looked: vec![direct, projects.join("*").join(name)],
                    })
                }
                None => pick(jsonl_in(&dir), dir),
            }
        }
        Kind::Codex => {
            let root = home.join(".codex/sessions");
            let mut rollouts = codex_rollouts(&root);
            match session {
                Some(id) => {
                    let suffix = format!("-{id}.jsonl");
                    rollouts
                        .into_iter()
                        .find(|p| file_name(p).ends_with(&suffix))
                        .ok_or(LocateError::NotFound {
                            looked: vec![root.join(format!("YYYY/MM/DD/rollout-*{suffix}"))],
                        })
                }
                None => {
                    rollouts.truncate(MAX_CODEX_SCAN);
                    let mine = rollouts
                        .into_iter()
                        .filter(|p| codex_cwd(p).as_deref() == Some(cwd))
                        .collect();
                    pick(mine, root)
                }
            }
        }
        Kind::Pi => {
            let sessions = home.join(".pi/agent/sessions");
            let dir = sessions.join(format!(
                "--{}--",
                cwd.trim_start_matches('/').replace('/', "-")
            ));
            match session {
                Some(id) => {
                    let suffix = format!("_{id}.jsonl");
                    let mut dirs = vec![dir.clone()];
                    dirs.extend(files_in(&sessions));
                    dirs.iter()
                        .flat_map(|d| jsonl_in(d))
                        .find(|p| file_name(p).ends_with(&suffix))
                        .ok_or(LocateError::NotFound {
                            looked: vec![dir.join(format!("*{suffix}"))],
                        })
                }
                None => pick(jsonl_in(&dir), dir),
            }
        }
        Kind::Agy => {
            let data = std::env::var_os("AGY_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".gemini/antigravity-cli"));
            let brain = data.join("brain");
            match session {
                Some(id) => {
                    let p = brain
                        .join(id)
                        .join(".system_generated/logs/transcript.jsonl");
                    if p.is_file() {
                        Ok(p)
                    } else {
                        Err(LocateError::NotFound { looked: vec![p] })
                    }
                }
                // agy keeps no cwd per conversation: only the id finds it.
                None => Err(LocateError::NotFound {
                    looked: vec![brain.join("<conversation-id>")],
                }),
            }
        }
    }
}

/// Claude names a project dir after its cwd with every character other than
/// ASCII letters and digits turned into `-`: `/a/b.c` -> `-a-b-c`.
pub fn claude_slug(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn pick(mut found: Vec<PathBuf>, looked: PathBuf) -> Result<PathBuf, LocateError> {
    match found.len() {
        0 => Err(LocateError::NotFound {
            looked: vec![looked],
        }),
        1 => Ok(found.remove(0)),
        _ => {
            found.sort_by_key(|p| std::cmp::Reverse(modified(p)));
            found.truncate(MAX_CANDIDATES);
            Err(LocateError::Ambiguous { candidates: found })
        }
    }
}

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn file_name(p: &Path) -> &str {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

fn jsonl_in(dir: &Path) -> Vec<PathBuf> {
    files_in(dir)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file())
        .collect()
}

/// `<root>/YYYY/MM/DD/rollout-*.jsonl`, newest day first.
fn codex_rollouts(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for y in files_in(root).into_iter().rev() {
        for m in files_in(&y).into_iter().rev() {
            for d in files_in(&m).into_iter().rev() {
                let mut day = jsonl_in(&d);
                day.retain(|p| file_name(p).starts_with("rollout-"));
                day.reverse();
                out.extend(day);
            }
        }
    }
    out
}

/// The cwd in a rollout's first line (`session_meta`).
fn codex_cwd(p: &Path) -> Option<String> {
    use io::BufRead;
    let f = fs::File::open(p).ok()?;
    let mut line = String::new();
    io::BufReader::new(f).read_line(&mut line).ok()?;
    let v: Value = serde_json::from_str(&line).ok()?;
    v["payload"]["cwd"].as_str().map(String::from)
}

/// Reads and parses a session file.
pub fn read(kind: Kind, path: &Path) -> io::Result<Vec<Entry>> {
    Ok(parse(kind, &fs::read_to_string(path)?))
}

/// Parses session JSONL. Lines that are not JSON or not understood are
/// skipped: the formats grow new entry types often.
pub fn parse(kind: Kind, jsonl: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match kind {
            Kind::Claude => claude(&v, &mut out),
            Kind::Codex => codex(&v, &mut out),
            Kind::Pi => pi(&v, &mut out),
            Kind::Agy => agy(&v, &mut out),
        }
    }
    out
}

fn push(out: &mut Vec<Entry>, role: Role, time: Option<i64>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        out.push(Entry {
            role,
            time,
            text: text.to_string(),
        });
    }
}

fn claude(v: &Value, out: &mut Vec<Entry>) {
    if v["isSidechain"] == true || v["isMeta"] == true {
        return;
    }
    let time = v["timestamp"].as_str().and_then(parse_rfc3339);
    let content = &v["message"]["content"];
    match v["type"].as_str() {
        Some("user") => match content {
            Value::String(s) => push(out, Role::User, time, s),
            Value::Array(blocks) => {
                for b in blocks.iter().filter(|b| b["type"] == "text") {
                    push(out, Role::User, time, b["text"].as_str().unwrap_or(""));
                }
            }
            _ => {}
        },
        Some("assistant") => {
            let error = v["isApiErrorMessage"] == true;
            for b in content.as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("text") => {
                        let role = if error { Role::Error } else { Role::Assistant };
                        push(out, role, time, b["text"].as_str().unwrap_or(""));
                    }
                    Some("tool_use") => push(
                        out,
                        Role::Tool,
                        time,
                        &tool_line(b["name"].as_str().unwrap_or("?"), &b["input"]),
                    ),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn codex(v: &Value, out: &mut Vec<Entry>) {
    let time = v["timestamp"].as_str().and_then(parse_rfc3339);
    let p = &v["payload"];
    match (v["type"].as_str(), p["type"].as_str()) {
        (Some("response_item"), Some("message")) => {
            let role = match p["role"].as_str() {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => return,
            };
            for b in p["content"].as_array().into_iter().flatten() {
                let text = b["text"].as_str().unwrap_or("");
                // Codex injects AGENTS.md, environment and skill context as
                // user messages; they open with a tag or a fixed heading.
                if role == Role::User
                    && (text.trim_start().starts_with('<') || text.starts_with("# AGENTS.md"))
                {
                    continue;
                }
                push(out, role, time, text);
            }
        }
        (Some("response_item"), Some("function_call")) => {
            let args = p["arguments"]
                .as_str()
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or(Value::Null);
            push(
                out,
                Role::Tool,
                time,
                &tool_line(p["name"].as_str().unwrap_or("?"), &args),
            );
        }
        (Some("response_item"), Some("custom_tool_call")) => {
            push(
                out,
                Role::Tool,
                time,
                &tool_line(p["name"].as_str().unwrap_or("?"), &p["input"]),
            );
        }
        (Some("event_msg"), Some("error" | "stream_error")) => {
            push(
                out,
                Role::Error,
                time,
                p["message"].as_str().unwrap_or("error"),
            );
        }
        _ => {}
    }
}

fn pi(v: &Value, out: &mut Vec<Entry>) {
    if v["type"] != "message" {
        return;
    }
    let m = &v["message"];
    let time = v["timestamp"]
        .as_str()
        .and_then(parse_rfc3339)
        .or_else(|| m["timestamp"].as_i64().map(|ms| ms / 1000));
    let role = match m["role"].as_str() {
        Some("user") => Role::User,
        Some("assistant") => Role::Assistant,
        _ => return,
    };
    match &m["content"] {
        Value::String(s) => push(out, role, time, s),
        Value::Array(blocks) => {
            for b in blocks {
                match b["type"].as_str() {
                    Some("text") => push(out, role, time, b["text"].as_str().unwrap_or("")),
                    Some("toolCall") => push(
                        out,
                        Role::Tool,
                        time,
                        &tool_line(b["name"].as_str().unwrap_or("?"), &b["arguments"]),
                    ),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if m["stopReason"] == "error" {
        push(
            out,
            Role::Error,
            time,
            m["errorMessage"].as_str().unwrap_or("error"),
        );
    }
}

fn agy(v: &Value, out: &mut Vec<Entry>) {
    let time = v["created_at"].as_str().and_then(parse_rfc3339);
    let text = v["content"].as_str().unwrap_or("");
    match (v["source"].as_str(), v["type"].as_str()) {
        (Some("USER_EXPLICIT"), Some("USER_INPUT")) => push(out, Role::User, time, text),
        (Some("MODEL"), Some("PLANNER_RESPONSE")) => {
            push(out, Role::Assistant, time, text);
            for t in v["tool_calls"].as_array().into_iter().flatten() {
                push(
                    out,
                    Role::Tool,
                    time,
                    &tool_line(t["name"].as_str().unwrap_or("?"), &t["args"]),
                );
            }
        }
        _ => {}
    }
}

/// Input keys that say most about a call, in order of preference.
const TOOL_KEYS: &[&str] = &[
    "command",
    "cmd",
    "file_path",
    "path",
    "pattern",
    "url",
    "query",
    "skill",
    "description",
    "prompt",
];

/// `name: <the most telling argument>` on one line, at most 160 chars.
pub fn tool_line(name: &str, input: &Value) -> String {
    let arg = match input {
        Value::Object(o) => TOOL_KEYS
            .iter()
            .find_map(|k| o.get(*k))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Array(a) => a
                    .iter()
                    .map(|x| x.as_str().map_or_else(|| x.to_string(), String::from))
                    .collect::<Vec<_>>()
                    .join(" "),
                other => other.to_string(),
            })
            .unwrap_or_else(|| input.to_string()),
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let arg = one_line(&arg);
    let line = if arg.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {arg}")
    };
    truncate(&line, 160)
}

/// Collapses all whitespace runs, newlines included, to one space.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` chars, with `…` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Unix seconds of `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, sec) = (n(11..13)?, n(14..16)?, n(17..19)?);
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        rest = r.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        r if r.len() == 6 && (r.starts_with('+') || r.starts_with('-')) => {
            let sign = if r.starts_with('-') { -1 } else { 1 };
            sign * (r.get(1..3)?.parse::<i64>().ok()? * 3600
                + r.get(4..6)?.parse::<i64>().ok()? * 60)
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec - offset)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `(year, month, day, hour, minute)` of Unix seconds shifted by `offset`
/// seconds (the local UTC offset), for display without a timezone crate.
pub fn civil(unix: i64, offset: i64) -> (i64, i64, i64, i64, i64) {
    let t = unix + offset;
    let (days, secs) = (t.div_euclid(86400), t.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, secs / 3600, secs % 3600 / 60)
}

/// Seconds east of UTC on this machine, from `date +%z` (e.g. `+0200`).
pub fn utc_offset() -> i64 {
    let out = std::process::Command::new("date").arg("+%z").output();
    let s = out
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    parse_offset(&s).unwrap_or(0)
}

pub fn parse_offset(s: &str) -> Option<i64> {
    let sign = match s.get(..1)? {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let h: i64 = s.get(1..3)?.parse().ok()?;
    let m: i64 = s.get(3..5)?.parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_round_trips_through_civil() {
        let t = parse_rfc3339("2026-10-10T07:54:01.853Z").unwrap();
        assert_eq!(civil(t, 0), (2026, 10, 10, 7, 54));
        assert_eq!(civil(t, 7200), (2026, 10, 10, 9, 54));
        assert_eq!(parse_rfc3339("2026-10-10T09:54:01+02:00"), Some(t));
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1709164800));
        assert_eq!(parse_rfc3339("yesterday"), None);
    }

    #[test]
    fn offsets_parse() {
        assert_eq!(parse_offset("+0200"), Some(7200));
        assert_eq!(parse_offset("-0530"), Some(-19800));
        assert_eq!(parse_offset("CEST"), None);
    }

    #[test]
    fn kind_from_herdr_names() {
        assert_eq!(Kind::from_herdr("claude"), Some(Kind::Claude));
        assert_eq!(Kind::from_herdr("herdr:antigravity_cli"), Some(Kind::Agy));
        assert_eq!(Kind::from_herdr("aider"), None);
    }

    #[test]
    fn claude_slug_dashes_slashes_and_dots() {
        assert_eq!(
            claude_slug("/Users/x/src/vega.transcript"),
            "-Users-x-src-vega-transcript"
        );
    }

    #[test]
    fn tool_line_picks_the_telling_argument() {
        let v: Value = serde_json::json!({"description": "d", "command": "cargo\n  test"});
        assert_eq!(tool_line("Bash", &v), "Bash: cargo test");
        assert_eq!(
            tool_line("exec", &Value::String("ls -la".into())),
            "exec: ls -la"
        );
        assert_eq!(tool_line("x", &serde_json::json!({"a": 1})), "x: {\"a\":1}");
        assert_eq!(
            tool_line("shell", &serde_json::json!({"cmd": ["ls", "-l"]})),
            "shell: ls -l"
        );
        assert!(tool_line("Bash", &serde_json::json!({"command": "a".repeat(500)})).ends_with('…'));
    }
}
