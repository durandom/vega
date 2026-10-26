//! The Uber's brief (docs/design/uber-uac.md §3 Brief, §5): Decisions are
//! the Uber's own open asks; FYI and Blocked come from each live UAC's reply
//! to `uber.brief`. The brief renders in at most `MAX_LINES` lines.
//!
//! `vega uber brief` fans `uber.brief` out on one `vega post ask` thread and
//! records the request; vegad on the Uber host stores each target's reply
//! (`store_reply`), so the brief renders without the model relaying it:
//! - `<uber_dir>/briefs/<thread>.json`: the request (`Request`);
//! - `<uber_dir>/briefs/<thread>/<host>_<pane>.txt`: one reply per target.
//!
//! Also the 4 h digest wake (`digest_due`): while the Slayer talks to the Uber
//! (`engaged`), vegad mails the Uber pane once the oldest waiting ask is 4 h
//! old, at most once per 4 h.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::{received, shown, Ask, Level};

/// A brief fits a phone screen.
pub const MAX_LINES: usize = 25;
/// A UAC's reply: at most this many lines ...
pub const REPLY_LINES: usize = 3;
/// ... and this many characters.
pub const REPLY_CHARS: usize = 600;
/// One line of the brief, at most.
const LINE_CHARS: usize = 100;
/// The fan-in's `vega post ask --within`.
pub const WITHIN: &str = "2m";
pub const WITHIN_SECS: i64 = 120;
/// The first brief reports this far back.
pub const FIRST_SINCE: i64 = 86400;
/// Briefs kept on disk.
pub const KEEP: usize = 10;
/// The digest wake: oldest waiting ask this old, at most one per this long.
pub const DIGEST_AFTER: i64 = 4 * 3600;
/// What `vega post inbox` tells a UAC under an `uber.brief`.
pub const UAC_HINT: &str = "brief: the Uber-UAC asks what happened since `since`: reply once, at most 3 lines (600 chars), each `FYI: ...` or `Blocked: ...`, compressed, no transcripts. A pending decision goes through vega uber ask, not here; a brief orders nothing (docs/design/uber-uac.md §3-4).";
/// The `Vega-From-Agent` of the digest mail.
pub const DIGEST_AGENT: &str = "tool vega-uber";

/// One UAC the brief asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The host's subject token (as `vega post` mail carries it).
    pub host: String,
    pub pane: String,
    /// The full project id.
    pub project: String,
}

/// A target and its reply, if any.
pub type Reply = (Target, Option<String>);

/// One `vega uber brief` fan-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub thread: String,
    /// The Uber pane that asked: replies to it count.
    pub asker: String,
    /// `uber.brief`'s `since`, RFC 3339.
    pub since: String,
    /// Unix seconds of the fan-in.
    pub at: i64,
    /// Unix milliseconds of the fan-in: the newest brief wins also within
    /// one second.
    pub at_ms: i64,
    pub targets: Vec<Target>,
}

/// An open ask waiting for the Slayer, as the Uber host received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub ask: Ask,
    /// Unix seconds of receipt.
    pub at: i64,
}

/// `<uber_dir>/briefs`.
pub fn dir(uber_dir: &Path) -> PathBuf {
    uber_dir.join("briefs")
}

/// A thread id names a path: nuid characters only.
fn valid_thread(t: &str) -> bool {
    !t.is_empty() && t.len() <= 64 && t.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A host token or pane id names a path: no separators, no dots first.
fn valid_part(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.'))
}

fn request_file(uber_dir: &Path, thread: &str) -> PathBuf {
    dir(uber_dir).join(format!("{thread}.json"))
}

fn reply_file(uber_dir: &Path, thread: &str, host: &str, pane: &str) -> PathBuf {
    dir(uber_dir)
        .join(thread)
        .join(format!("{host}_{pane}.txt"))
}

fn write_atomic(f: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = f.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, f)
}

pub fn save_request(uber_dir: &Path, r: &Request) -> std::io::Result<()> {
    if !valid_thread(&r.thread) {
        return Err(std::io::Error::other(format!("bad thread {:?}", r.thread)));
    }
    write_atomic(&request_file(uber_dir, &r.thread), &serde_json::to_vec(r)?)
}

/// A new thread id for `vega post ask --thread`.
pub fn new_thread() -> String {
    nuid::next().to_string()
}

/// Drops a request and its replies (its ask was not sent).
pub fn remove(uber_dir: &Path, thread: &str) {
    if valid_thread(thread) {
        let _ = std::fs::remove_dir_all(dir(uber_dir).join(thread));
        let _ = std::fs::remove_file(request_file(uber_dir, thread));
    }
}

pub fn load(uber_dir: &Path, thread: &str) -> Option<Request> {
    if !valid_thread(thread) {
        return None;
    }
    let text = std::fs::read_to_string(request_file(uber_dir, thread)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Every request, newest first.
fn all(uber_dir: &Path) -> Vec<Request> {
    let mut v: Vec<Request> = std::fs::read_dir(dir(uber_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .collect();
    v.sort_by(|a: &Request, b| b.at_ms.cmp(&a.at_ms).then(b.thread.cmp(&a.thread)));
    v
}

/// The newest brief.
pub fn last(uber_dir: &Path) -> Option<Request> {
    all(uber_dir).into_iter().next()
}

/// Keeps the newest `keep` briefs and their replies.
pub fn prune(uber_dir: &Path, keep: usize) {
    for r in all(uber_dir).into_iter().skip(keep) {
        let _ = std::fs::remove_dir_all(dir(uber_dir).join(&r.thread));
        let _ = std::fs::remove_file(request_file(uber_dir, &r.thread));
    }
}

/// vegad on the Uber host: stores mail from `host`/`pane` to `to` on
/// `thread` when it answers a recorded brief of `to` from one of its
/// targets. `Ok(false)`: not a brief reply, nothing stored. A later reply
/// of the same UAC replaces its earlier one.
pub fn store_reply(
    uber_dir: &Path,
    thread: &str,
    to: &str,
    host: &str,
    pane: &str,
    body: &[u8],
) -> std::io::Result<bool> {
    let Some(r) = load(uber_dir, thread) else {
        return Ok(false);
    };
    let ours = r.asker == to && r.targets.iter().any(|t| t.host == host && t.pane == pane);
    if !ours || !valid_part(host) || !valid_part(pane) {
        return Ok(false);
    }
    let text = String::from_utf8_lossy(body);
    let capped: String = text.chars().take(REPLY_CHARS).collect();
    write_atomic(&reply_file(uber_dir, thread, host, pane), capped.as_bytes())?;
    Ok(true)
}

/// Each target with its reply, if any.
pub fn replies(uber_dir: &Path, r: &Request) -> Vec<Reply> {
    r.targets
        .iter()
        .map(|t| {
            let text = (valid_part(&t.host) && valid_part(&t.pane))
                .then(|| std::fs::read_to_string(reply_file(uber_dir, &r.thread, &t.host, &t.pane)))
                .and_then(Result::ok);
            (t.clone(), text)
        })
        .collect()
}

/// A reply's FYI and Blocked items: its first `REPLY_LINES` non-empty lines
/// within `REPLY_CHARS`, each `FYI: ...` or `Blocked: ...`. An unmarked line
/// counts as FYI; `none`, `nothing` or `-` after the mark is dropped.
pub fn parse_reply(text: &str) -> (Vec<String>, Vec<String>) {
    let capped: String = text.chars().take(REPLY_CHARS).collect();
    let (mut fyi, mut blocked) = (Vec::new(), Vec::new());
    for line in capped
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(REPLY_LINES)
    {
        let line = line.trim_start_matches(['-', '*', '•']).trim_start();
        let (is_blocked, rest) = match line.split_once(':') {
            Some((mark, rest)) if mark.trim().eq_ignore_ascii_case("blocked") => (true, rest),
            Some((mark, rest)) if mark.trim().eq_ignore_ascii_case("fyi") => (false, rest),
            _ => (false, line),
        };
        let rest = rest.trim();
        let empty = rest.is_empty()
            || ["none", "nothing", "-", "n/a", "—"]
                .iter()
                .any(|n| rest.trim_end_matches('.').eq_ignore_ascii_case(n));
        if empty {
            continue;
        }
        if is_blocked {
            blocked.push(rest.to_string());
        } else {
            fyi.push(rest.to_string());
        }
    }
    (fyi, blocked)
}

/// RFC 3339 of unix seconds, UTC.
pub fn rfc3339(unix: i64) -> String {
    OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

/// ` · due 14:00` (today in the deadline's offset), ` · due 10-27 14:00`,
/// ` · overdue`; empty for `no-rush`.
fn due(deadline: &str, now: i64) -> String {
    let Ok(t) = OffsetDateTime::parse(deadline, &Rfc3339) else {
        return String::new();
    };
    if t.unix_timestamp() <= now {
        return " · overdue".into();
    }
    let today = OffsetDateTime::from_unix_timestamp(now)
        .map(|n| n.to_offset(t.offset()).date() == t.date())
        .unwrap_or(false);
    if today {
        format!(" · due {:02}:{:02}", t.hour(), t.minute())
    } else {
        format!(
            " · due {:02}-{:02} {:02}:{:02}",
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute()
        )
    }
}

/// One line, at most `LINE_CHARS` characters.
fn clip(s: &str) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= LINE_CHARS {
        return one;
    }
    let mut c: String = one.chars().take(LINE_CHARS - 1).collect();
    c.push('…');
    c
}

/// `45s`, `25m`, `4h05m`, `3d`.
pub fn age(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..172800 => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d", secs / 86400),
    }
}

/// Urgent asks first, then the oldest (as `vega uber show` orders them).
fn ordered(decisions: &[Decision]) -> Vec<&Decision> {
    let mut d: Vec<&Decision> = decisions.iter().collect();
    d.sort_by(|a, b| {
        b.ask
            .urgent
            .is_some()
            .cmp(&a.ask.urgent.is_some())
            .then(a.at.cmp(&b.at))
            .then(a.ask.id.cmp(&b.ask.id))
    });
    d
}

/// `1 vega · L4 · urgent · push main? · rec: yes · due 14:00`.
fn decision_line(n: usize, d: &Decision, now: i64) -> String {
    let a = &d.ask;
    let level = match a.level {
        Level::L3 => "L3",
        Level::L4 => "L4",
    };
    let urgent = if a.urgent.is_some() { " · urgent" } else { "" };
    let rec = a
        .options
        .get(a.recommend)
        .map(|o| format!(" · rec: {}", o.label))
        .unwrap_or_default();
    clip(&format!(
        "{n} {} · {level}{urgent} · {}{rec}{}",
        shown::header(&a.project),
        a.question,
        due(&a.deadline, now)
    ))
}

/// How many lines each section gets within `budget`: one at a time, in
/// `order`, so a long section cannot crowd out the others.
fn fit(lens: [usize; 3], budget: usize, order: [usize; 3]) -> [usize; 3] {
    let mut got = [0; 3];
    let mut used = 0;
    loop {
        let mut more = false;
        for &s in &order {
            if got[s] < lens[s] && used < budget {
                got[s] += 1;
                used += 1;
                more = true;
            }
        }
        if !more {
            return got;
        }
    }
}

/// The brief: `Decisions (n)`, `FYI (n)`, `Blocked (n)`, one line per item,
/// then who has not replied; at most `max` lines (`MAX_LINES` for the
/// phone). A section cut short ends in `+n more`. `brief` is the last fan-in and its replies.
pub fn render(
    decisions: &[Decision],
    brief: Option<(&Request, &[Reply])>,
    now: i64,
    max: usize,
) -> Vec<String> {
    let decided: Vec<String> = ordered(decisions)
        .into_iter()
        .enumerate()
        .map(|(i, d)| decision_line(i + 1, d, now))
        .collect();
    let (mut fyi, mut blocked, mut silent) = (Vec::new(), Vec::new(), Vec::new());
    for (t, reply) in brief.map(|b| b.1).unwrap_or_default() {
        let name = shown::header(&t.project);
        match reply {
            Some(text) => {
                let (f, b) = parse_reply(text);
                fyi.extend(f.into_iter().map(|x| clip(&format!("{name}: {x}"))));
                blocked.extend(b.into_iter().map(|x| clip(&format!("{name}: {x}"))));
            }
            None => silent.push(name),
        }
    }
    let footer = match brief {
        None => Some("FYI/Blocked: no brief asked yet (vega uber brief)".to_string()),
        Some((r, _)) if r.targets.is_empty() => {
            Some(format!("asked: no live UAC ({} ago)", age(now - r.at)))
        }
        Some(_) if silent.is_empty() => None,
        Some((r, _)) => {
            let what = if now - r.at < WITHIN_SECS {
                "waiting"
            } else {
                "no reply"
            };
            Some(clip(&format!(
                "{what} ({}): {} (asked {} ago)",
                silent.len(),
                silent.join(", "),
                age(now - r.at)
            )))
        }
    };
    let sections = [("Decisions", decided), ("FYI", fyi), ("Blocked", blocked)];
    let budget = max.saturating_sub(sections.len() + usize::from(footer.is_some()));
    let lens = [
        sections[0].1.len(),
        sections[1].1.len(),
        sections[2].1.len(),
    ];
    // Decisions first, then Blocked, then FYI.
    let got = fit(lens, budget, [0, 2, 1]);
    let mut out = Vec::new();
    for ((title, items), n) in sections.into_iter().zip(got) {
        out.push(format!("{title} ({})", items.len()));
        if n < items.len() {
            out.extend(items.iter().take(n.saturating_sub(1)).cloned());
            out.push(format!("+{} more", items.len() - n.saturating_sub(1)));
        } else {
            out.extend(items);
        }
    }
    out.extend(footer);
    out
}

/// The Uber's asks that wait for the Slayer (`received`, state open),
/// oldest first.
pub fn open_decisions(uber_dir: &Path) -> Vec<Decision> {
    received::all(uber_dir)
        .into_iter()
        .filter(|r| r.state == received::State::Open)
        .map(|r| Decision {
            at: OffsetDateTime::parse(&r.at, &Rfc3339)
                .map(|t| t.unix_timestamp())
                .unwrap_or(0),
            ask: r.ask,
        })
        .collect()
}

/// Whether the Slayer is talking to the Uber now (its claim's engaged
/// window). The Uber must not wake him otherwise (the Slayer's rule, uber-uac.md
/// §3 Batching). Deliberately off: vega-lhc.209.12 replaces this call with
/// `View::uber_engaged(&claim, now).on()` in vegad's digest; until then no
/// digest is sent, which the docs say.
pub fn engaged(_uber_dir: &Path, _now: i64) -> bool {
    false
}

/// The digest mail, while the Uber is `engaged`, when the oldest waiting ask
/// (unix seconds of receipt in `waiting`) is `DIGEST_AFTER` old and the last
/// digest (`last`) is as old.
pub fn digest_due(waiting: &[i64], last: Option<i64>, engaged: bool, now: i64) -> Option<String> {
    if !engaged {
        return None;
    }
    let oldest = *waiting.iter().min()?;
    let ripe = now - oldest >= DIGEST_AFTER;
    let quiet = last.is_none_or(|l| now - l >= DIGEST_AFTER);
    (ripe && quiet).then(|| {
        format!(
            "digest: {} asks wait for the Slayer, the oldest {}. Run vega uber brief; open the dialog 60 s after this pointer (docs/design/uber-uac.md §5).",
            waiting.len(),
            age(now - oldest)
        )
    })
}

/// `<uber_dir>/digest.json`: unix seconds of the last digest mail.
fn digest_file(uber_dir: &Path) -> PathBuf {
    uber_dir.join("digest.json")
}

pub fn last_digest(uber_dir: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(digest_file(uber_dir)).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()?["at"].as_i64()
}

pub fn save_digest(uber_dir: &Path, at: i64) -> std::io::Result<()> {
    write_atomic(
        &digest_file(uber_dir),
        serde_json::json!({ "at": at }).to_string().as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Opt;

    const NOW: i64 = 1_793_023_200; // 2026-10-26T14:00:00Z

    fn ask(project: &str, bead: &str, level: Level, urgent: bool, deadline: &str) -> Ask {
        Ask {
            id: format!("{bead}~X"),
            project: project.into(),
            bead: bead.into(),
            level,
            urgent: urgent.then(|| "prod down".into()),
            question: "push main?".into(),
            context: String::new(),
            options: vec![
                Opt {
                    label: "yes".into(),
                    detail: String::new(),
                },
                Opt {
                    label: "no".into(),
                    detail: String::new(),
                },
            ],
            recommend: 0,
            why: String::new(),
            deadline: deadline.into(),
        }
    }

    fn target(host: &str, pane: &str, project: &str) -> Target {
        Target {
            host: host.into(),
            pane: pane.into(),
            project: project.into(),
        }
    }

    fn request(targets: Vec<Target>, at: i64) -> Request {
        Request {
            thread: "T1".into(),
            asker: "w1:p1".into(),
            since: rfc3339(at - FIRST_SINCE),
            at,
            at_ms: at * 1000,
            targets,
        }
    }

    #[test]
    fn replies_split_into_fyi_and_blocked_within_three_lines() {
        let (f, b) = parse_reply(
            "FYI: landed 209.4\nBlocked: waiting on CI\n- fyi: gate pass\nBlocked: fourth line",
        );
        assert_eq!(f, ["landed 209.4", "gate pass"]);
        assert_eq!(b, ["waiting on CI"]);
        let (f, b) = parse_reply("Blocked: none\nFYI: -\nquiet day");
        assert_eq!((f, b), (vec!["quiet day".to_string()], vec![]));
        let long = format!("FYI: {}", "x".repeat(700));
        assert_eq!(parse_reply(&long).0[0].chars().count(), REPLY_CHARS - 5);
    }

    #[test]
    fn decisions_show_level_rec_and_due_urgent_first() {
        let d = [
            Decision {
                ask: ask(
                    "github.com/durandom/vega",
                    "vega-1",
                    Level::L4,
                    false,
                    "2026-10-26T15:30:00+01:00",
                ),
                at: NOW - 600,
            },
            Decision {
                ask: ask(
                    "github.com/durandom/dotfiles",
                    "dot-2",
                    Level::L3,
                    true,
                    "no-rush",
                ),
                at: NOW - 60,
            },
            Decision {
                ask: ask("arena", "ar-3", Level::L3, false, "2026-10-26T13:00:00Z"),
                at: NOW - 3600,
            },
        ];
        let out = render(&d, None, NOW, MAX_LINES);
        assert_eq!(out[0], "Decisions (3)");
        assert_eq!(out[1], "1 dotfiles · L3 · urgent · push main? · rec: yes");
        assert_eq!(out[2], "2 arena · L3 · push main? · rec: yes · overdue");
        assert_eq!(out[3], "3 vega · L4 · push main? · rec: yes · due 15:30");
        assert_eq!(
            out.last().unwrap(),
            "FYI/Blocked: no brief asked yet (vega uber brief)"
        );
    }

    #[test]
    fn a_brief_over_three_uacs_renders_within_25_lines() {
        let d: Vec<Decision> = (0..20)
            .map(|i| Decision {
                ask: ask(
                    "github.com/durandom/vega",
                    &format!("vega-{i}"),
                    Level::L3,
                    false,
                    "no-rush",
                ),
                at: NOW - i,
            })
            .collect();
        let r = request(
            vec![
                target("proteus", "w3V:p1", "github.com/durandom/vega"),
                target("ganymede", "w1:p1", "github.com/durandom/dotfiles"),
                target("titan", "w2:p1", "github.com/b4arena/arena"),
            ],
            NOW - 30,
        );
        let replies = vec![
            (
                r.targets[0].clone(),
                Some("FYI: landed uber-ask\nFYI: gate pass\nBlocked: CI red on main".to_string()),
            ),
            (
                r.targets[1].clone(),
                Some("FYI: zsh theme\nFYI: herdr 0.9\nFYI: nothing else".to_string()),
            ),
            (r.targets[2].clone(), None),
        ];
        let out = render(&d, Some((&r, &replies)), NOW, MAX_LINES);
        assert!(
            out.len() <= MAX_LINES,
            "{} lines:\n{}",
            out.len(),
            out.join("\n")
        );
        assert!(out.iter().all(|l| l.chars().count() <= LINE_CHARS));
        assert_eq!(out[0], "Decisions (20)");
        // `vega uber brief` keeps one line for its help.
        let fewer = render(&d, Some((&r, &replies)), NOW, MAX_LINES - 1);
        assert_eq!(fewer.len(), MAX_LINES - 1);
        assert!(out.contains(&"FYI (5)".to_string()));
        assert!(out.contains(&"vega: landed uber-ask".to_string()));
        assert!(out.contains(&"Blocked (1)".to_string()));
        assert!(out.contains(&"vega: CI red on main".to_string()));
        assert!(out
            .iter()
            .any(|l| l.starts_with("+") && l.ends_with(" more")));
        assert_eq!(out.last().unwrap(), "waiting (1): arena (asked 30s ago)");
        // Without the overflow every item shows and nothing is cut.
        let out = render(&d[..2], Some((&r, &replies)), NOW + 300, MAX_LINES);
        assert_eq!(out.len(), 3 + 2 + 5 + 1 + 1);
        assert_eq!(out.last().unwrap(), "no reply (1): arena (asked 5m ago)");
    }

    #[test]
    fn replies_are_stored_only_from_targets_to_the_asker() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = request(vec![target("proteus", "w3V:p1", "vega")], NOW);
        save_request(d, &r).unwrap();
        assert!(!store_reply(d, "T1", "w9:p9", "proteus", "w3V:p1", b"x").unwrap());
        assert!(!store_reply(d, "T1", "w1:p1", "titan", "w3V:p1", b"x").unwrap());
        assert!(!store_reply(d, "T2", "w1:p1", "proteus", "w3V:p1", b"x").unwrap());
        assert!(!store_reply(d, "../x", "w1:p1", "proteus", "w3V:p1", b"x").unwrap());
        assert_eq!(replies(d, &r)[0].1, None);
        assert!(store_reply(d, "T1", "w1:p1", "proteus", "w3V:p1", b"FYI: a").unwrap());
        assert!(store_reply(d, "T1", "w1:p1", "proteus", "w3V:p1", b"FYI: b").unwrap());
        assert_eq!(replies(d, &r)[0].1.as_deref(), Some("FYI: b"));
        assert_eq!(last(d), Some(r.clone()));
    }

    #[test]
    fn last_is_the_newest_and_prune_keeps_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for (i, t) in ["A", "B", "C"].iter().enumerate() {
            let mut r = request(vec![target("h", "w1:p2", "p")], NOW + i as i64);
            r.thread = t.to_string();
            save_request(d, &r).unwrap();
            store_reply(d, t, "w1:p1", "h", "w1:p2", b"FYI: x").unwrap();
        }
        assert_eq!(last(d).unwrap().thread, "C");
        prune(d, 2);
        assert!(load(d, "A").is_none() && !dir.path().join("briefs/A").exists());
        assert!(load(d, "B").is_some());
    }

    #[test]
    fn of_two_briefs_in_one_second_the_later_is_last() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for (t, ms) in [("Zolder", 0), ("Anewer", 500)] {
            let mut r = request(vec![], NOW);
            r.thread = t.into();
            r.at_ms = NOW * 1000 + ms;
            save_request(d, &r).unwrap();
        }
        assert_eq!(last(d).unwrap().thread, "Anewer");
    }

    #[test]
    fn decisions_are_the_received_asks_that_wait_for_the_slayer() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let record = |bead: &str, state, at: i64| received::Received {
            ask: ask("vega", bead, Level::L3, false, "no-rush"),
            tag: format!("#{bead}"),
            from_host: "proteus".into(),
            from_pane: "w3V:p1".into(),
            at: rfc3339(at),
            floor: 0,
            state,
            used: vec![],
            result: None,
        };
        received::save(d, &record("b", received::State::Open, NOW - 60)).unwrap();
        received::save(d, &record("a", received::State::Open, NOW - 600)).unwrap();
        received::save(d, &record("c", received::State::Sent, NOW)).unwrap();
        received::save(d, &record("e", received::State::Withdrawn, NOW)).unwrap();
        let open = open_decisions(d);
        let got: Vec<_> = open.iter().map(|x| (x.ask.bead.as_str(), x.at)).collect();
        assert_eq!(got, [("a", NOW - 600), ("b", NOW - 60)]);
        assert_eq!(render(&open, None, NOW, MAX_LINES)[0], "Decisions (2)");
    }

    #[test]
    fn the_digest_waits_for_4h_and_comes_once_per_4h() {
        assert_eq!(digest_due(&[], None, true, NOW), None);
        assert_eq!(digest_due(&[NOW - DIGEST_AFTER + 1], None, true, NOW), None);
        let old = [NOW - 60, NOW - DIGEST_AFTER - 300];
        let m = digest_due(&old, None, true, NOW).unwrap();
        assert!(m.starts_with("digest: 2 asks wait for the Slayer, the oldest 4h05m."));
        // Not while the Slayer is away: the Uber does not spam.
        assert_eq!(digest_due(&old, None, false, NOW), None);
        assert_eq!(digest_due(&old, Some(NOW - 3600), true, NOW), None);
        assert!(digest_due(&old, Some(NOW - DIGEST_AFTER), true, NOW).is_some());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(last_digest(dir.path()), None);
        save_digest(dir.path(), NOW).unwrap();
        assert_eq!(last_digest(dir.path()), Some(NOW));
    }
}
