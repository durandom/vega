//! The Uber host's side of an answer (docs/design/uber-uac.md §4-5,
//! vega-lhc.209.6): `vega uber answer`
//! builds the `uber.answer` from one capture and sends it, and
//! `vega uber audit` prints `audit.jsonl`. The records and the binding
//! rules live in `vega_uber_msg::{capture, received, audit}`.

use std::path::{Path, PathBuf};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use vega_herdr::{state as st, Failure};
use vega_registry::uber_state;
use vega_uber_msg::received::{self, Received};
use vega_uber_msg::{audit, capture, Answer};

/// `$XDG_STATE_HOME/vega/uber`, next to the claim file.
pub fn uber_dir() -> Result<PathBuf, Failure> {
    uber_state::file()
        .and_then(|f| f.parent().map(Path::to_path_buf))
        .ok_or_else(|| Failure::new("no_home", "no state dir", "set HOME or XDG_STATE_HOME"))
}

/// Now, RFC 3339 in UTC (`audit` prints it as `10-26 13:02Z`).
pub fn now() -> String {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// An ask by id (`<bead>~<nuid>`), or by the tag it holds now, with or
/// without its `#` (an unquoted `#a7` is a shell comment).
fn find(dir: &Path, key: &str) -> Result<Received, Failure> {
    let r = if key.contains('~') {
        received::load(dir, key)
    } else {
        received::by_tag(dir, &format!("#{}", key.trim_start_matches('#')))
    };
    r.ok_or_else(|| {
        Failure::new(
            "no_ask",
            format!("no received ask {key}"),
            "vega uber audit lists the asks received",
        )
    })
}

/// The captures that would answer `r` now: newer, unused and bound.
fn candidates(dir: &Path, r: &Received, epoch: u64, transcript: &str) -> Vec<usize> {
    (r.floor + 1..=capture::count(dir))
        .filter(|&n| {
            capture::read(dir, n)
                .is_some_and(|l| capture::build(r, n, &l, epoch, transcript).is_ok())
        })
        .collect()
}

fn refuse(dir: &Path, r: &Received, at: &str, f: Failure) -> Failure {
    let ev = audit::Event::of(r, "rejected", at, &format!("{}: {}", f.code, f.message));
    let _ = audit::append(dir, &ev);
    f
}

/// What `answer_in` did.
pub struct Sent {
    pub answer: Answer,
    pub record: Received,
    pub n: usize,
}

/// Builds the answer to `key` from capture `n` under `epoch` and sends it
/// with `send`; records and audits the outcome. Refusals are audited too.
pub fn answer_in(
    dir: &Path,
    key: &str,
    n: Option<usize>,
    epoch: u64,
    transcript: &str,
    at: &str,
    send: impl FnOnce(&Received, &str) -> Result<(), Failure>,
) -> Result<Sent, Failure> {
    let mut r = find(dir, key)?;
    let Some(n) = n else {
        let c = candidates(dir, &r, epoch, transcript);
        let hint = match c.as_slice() {
            [] => format!(
                "show {} with AskUserQuestion (vega uber show) or have the Slayer type it",
                r.tag
            ),
            _ => format!(
                "captures bound to {}: {}; vega uber answer {} --capture <n>",
                r.tag,
                c.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                r.tag
            ),
        };
        let f = Failure::new("no_capture", "an answer needs the Slayer's capture", &hint);
        return Err(refuse(dir, &r, at, f));
    };
    let line = capture::read(dir, n).ok_or_else(|| {
        let f = Failure::new(
            "no_capture",
            format!("no capture {n}"),
            &format!("captures: 1..={}", capture::count(dir)),
        );
        refuse(dir, &r, at, f)
    })?;
    let answer = capture::build(&r, n, &line, epoch, transcript).map_err(|e| {
        let f = Failure::new(
            e.code,
            e.message,
            "only the Slayer's own input answers an ask",
        );
        refuse(dir, &r, at, f)
    })?;
    let body = serde_json::to_string(&answer).unwrap_or_default();
    send(&r, &body).map_err(|f| refuse(dir, &r, at, f))?;
    r.used.push(n);
    r.state = received::State::Sent;
    let io = |e: std::io::Error| Failure::new("state_io", e.to_string(), "check the state dir");
    received::save(dir, &r).map_err(io)?;
    let answered = audit::Event::of(
        &r,
        "answered",
        at,
        &format!("capture {n}: {}", answer.slayer_text),
    );
    audit::append(dir, &answered).map_err(io)?;
    let sent = audit::Event::of(
        &r,
        "sent",
        at,
        &format!("to {} on {}", r.from_pane, r.from_host),
    );
    audit::append(dir, &sent).map_err(io)?;
    Ok(Sent {
        answer,
        record: r,
        n,
    })
}

/// `vega post send --host <h> <pane> --kind uber.answer '<json>'`.
fn post(r: &Received, body: &str) -> Result<(), Failure> {
    let out = std::process::Command::new("vega")
        .args(["post", "send", "--host", &r.from_host, &r.from_pane])
        .args(["--kind", "uber.answer", body])
        .output()
        .map_err(|e| Failure::new("vega_not_found", e.to_string(), "install vega"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Err(Failure::new(
        "send_failed",
        text.lines()
            .nth(1)
            .unwrap_or("vega post send failed")
            .trim(),
        "the ask stays open; retry vega uber answer",
    ))
}

/// This pane's Claude Code session log: the harness's record of what the
/// Slayer typed and picked, which the model cannot write (§4).
fn session_log() -> Result<String, Failure> {
    let unknown = |why: String| {
        Failure::new(
            "no_session_log",
            why,
            "the Uber-UAC is a Claude Code session in a herdr pane",
        )
    };
    let v = vega_herdr::herdr_local(&["pane", "current", "--current"])?;
    let pane = &v["result"]["pane"];
    if pane["agent"] != "claude" {
        return Err(unknown(format!(
            "this pane runs {}, not claude",
            pane["agent"]
        )));
    }
    let session = pane["agent_session"]["value"].as_str();
    let cwd = pane["cwd"].as_str().unwrap_or("");
    let home = std::env::var_os("HOME").ok_or_else(|| unknown("no HOME".into()))?;
    let path = vega_transcript::locate(
        vega_transcript::Kind::Claude,
        session,
        cwd,
        Path::new(&home),
    )
    .map_err(|e| unknown(format!("session log not found: {e:?}")))?;
    std::fs::read_to_string(&path).map_err(|e| unknown(format!("{}: {e}", path.display())))
}

/// `vega uber answer <id|#tag> --capture <n>`: only the live claim's pane.
pub fn answer(key: &str, n: Option<usize>) -> Result<(), Failure> {
    let dir = uber_dir()?;
    let me = vega_herdr::me()?;
    let host = st::post_host();
    let claim = uber_state::file()
        .and_then(|f| uber_state::local(&f, &host))
        .filter(|c| c["pane"] == me.pane.as_str());
    let Some(epoch) = claim.and_then(|c| c["epoch"].as_u64()) else {
        return Err(Failure::new(
            "not_uber",
            format!("{} on {host} holds no Uber claim", me.pane),
            "only the Uber-UAC answers: vega uber claim",
        ));
    };
    let transcript = session_log()?;
    let s = answer_in(&dir, key, n, epoch, &transcript, &now(), post)?;
    let yes = |b: bool| if b { "yes" } else { "no" };
    println!(
        "answered: {} {} (capture {}, pick {}, approval {})",
        s.record.tag,
        s.record.ask.bead,
        s.n,
        s.answer.pick.as_deref().unwrap_or("none"),
        yes(s.answer.approval)
    );
    println!("sent: {} on {}", s.record.from_pane, s.record.from_host);
    println!("help: vega uber audit (applied or refused arrives as uber.applied)");
    Ok(())
}

/// `vega uber audit [--last n]`: one line per event.
pub fn audit(last: usize) -> Result<(), Failure> {
    let dir = uber_dir()?;
    let events = audit::last(&dir, last);
    if events.is_empty() {
        println!("audit: no events yet");
        return Ok(());
    }
    println!("audit: last {} events", events.len());
    for e in events {
        println!("  {}", e.line());
    }
    Ok(())
}
