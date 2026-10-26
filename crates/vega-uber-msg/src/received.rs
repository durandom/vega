//! The Uber host's record of the asks it received (docs/design/uber-uac.md
//! §3-4): `$XDG_STATE_HOME/vega/uber/received/<id>.json`, one file per ask.
//! The UAC host keeps its own open asks in `asks/` (vega-lhc.209.4); both
//! may live on one host, so the names differ.
//!
//! vegad on the Uber host calls `ingest` for each `uber.ask`, `uber.withdraw`
//! and `uber.applied` it receives for the live Uber pane; `vega uber answer`
//! reads and updates the record. Every change appends an audit line.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audit;
use crate::capture;
use crate::{tag, Ask, Msg};

/// Where an ask stands on the Uber host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Waiting for the Slayer.
    Open,
    /// `vega uber answer` sent the answer; `uber.applied` not yet back.
    Sent,
    /// The UAC host applied the answer.
    Applied,
    /// The UAC host refused the answer for good (`result` holds the code,
    /// one of `FINAL`). Any other refusal reopens the ask.
    Refused,
    /// The UAC withdrew the ask.
    Withdrawn,
}

impl State {
    /// Open for the tag rule: the ask still holds its tag.
    pub fn holds_tag(self) -> bool {
        !matches!(self, State::Applied | State::Withdrawn | State::Refused)
    }
}

/// Refusal codes after which no answer to the ask can apply: it is closed,
/// answered or gone on the UAC host. Every other code (`bd_failed`,
/// `bead_unknown`, a wrong pick or rendering, ...) reopens the ask, and the
/// capture that was refused may be sent again.
pub const FINAL: [&str; 4] = ["answered", "withdrawn", "bead_closed", "no_ask"];

/// One received ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Received {
    pub ask: Ask,
    pub tag: String,
    /// The UAC's host and pane: where the answer goes.
    pub from_host: String,
    pub from_pane: String,
    /// First received, RFC 3339.
    pub at: String,
    /// `captures.jsonl` lines at receipt: only later captures bind.
    pub floor: usize,
    pub state: State,
    /// The capture numbers already used for this ask.
    #[serde(default)]
    pub used: Vec<usize>,
    /// `refused:<code>` from `uber.applied`.
    #[serde(default)]
    pub result: Option<String>,
}

/// `<dir>/received`.
pub fn dir(uber_dir: &Path) -> PathBuf {
    uber_dir.join("received")
}

fn file(uber_dir: &Path, id: &str) -> PathBuf {
    dir(uber_dir).join(format!("{id}.json"))
}

/// The record of `id`, if any.
pub fn load(uber_dir: &Path, id: &str) -> Option<Received> {
    // An id is `<bead>~<nuid>`: no path separators get through.
    crate::id_bead(id).ok()?;
    let text = std::fs::read_to_string(file(uber_dir, id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes the record atomically.
pub fn save(uber_dir: &Path, r: &Received) -> std::io::Result<()> {
    let d = dir(uber_dir);
    std::fs::create_dir_all(&d)?;
    let f = file(uber_dir, &r.ask.id);
    let tmp = f.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(r)?)?;
    std::fs::rename(tmp, f)
}

/// Every record, oldest first.
pub fn all(uber_dir: &Path) -> Vec<Received> {
    let mut v: Vec<Received> = std::fs::read_dir(dir(uber_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .collect();
    v.sort_by(|a: &Received, b| a.at.cmp(&b.at).then(a.ask.id.cmp(&b.ask.id)));
    v
}

/// The record that holds `tag` now.
pub fn by_tag(uber_dir: &Path, tag: &str) -> Option<Received> {
    all(uber_dir)
        .into_iter()
        .find(|r| r.state.holds_tag() && r.tag == tag)
}

/// The id's tag: the shortest not held by another ask (`tag::of`).
fn assign_tag(uber_dir: &Path, id: &str) -> Option<String> {
    let taken: Vec<String> = all(uber_dir)
        .into_iter()
        .filter(|r| r.state.holds_tag() && r.ask.id != id)
        .map(|r| r.tag)
        .collect();
    tag::of(id, &taken.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Records one received message: a new or resent ask, a withdraw, an
/// applied result. `from_host` is broker-enforced. Returns the audit line
/// it wrote. An error is file I/O; a message that changes nothing (unknown
/// id, wrong sender) is audited as `ignored`.
pub fn ingest(
    uber_dir: &Path,
    msg: &Msg,
    from_host: &str,
    from_pane: &str,
    now: &str,
) -> std::io::Result<audit::Event> {
    let ev = match msg {
        Msg::Ask(ask) => ingest_ask(uber_dir, ask, from_host, from_pane, now)?,
        Msg::Withdraw(w) => match load(uber_dir, &w.id).filter(|r| r.from_host == from_host) {
            Some(mut r) if r.state != State::Applied => {
                r.state = State::Withdrawn;
                save(uber_dir, &r)?;
                audit::Event::of(&r, "withdrawn", now, &w.reason)
            }
            _ => audit::Event::bare("ignored", now, &w.id, "withdraw of no open ask"),
        },
        Msg::Applied(a) => match load(uber_dir, &a.id).filter(|r| r.from_host == from_host) {
            Some(mut r) if r.state == State::Sent => {
                let code = a.result.strip_prefix("refused:");
                let detail = match code {
                    None => {
                        r.state = State::Applied;
                        a.result.clone()
                    }
                    Some(c) if FINAL.contains(&c) => {
                        r.state = State::Refused;
                        a.result.clone()
                    }
                    Some(_) => {
                        // Retryable: open again, and the refused capture
                        // may be sent once more.
                        r.state = State::Open;
                        r.used.pop();
                        format!("{}; open again: vega uber answer {}", a.result, r.tag)
                    }
                };
                r.result = code.map(|_| a.result.clone());
                save(uber_dir, &r)?;
                let event = if code.is_some() { "refused" } else { "applied" };
                audit::Event::of(&r, event, now, &detail)
            }
            _ => audit::Event::bare(
                "ignored",
                now,
                &a.id,
                &format!("{} of no sent answer", a.result),
            ),
        },
        Msg::Answer(a) => audit::Event::bare("ignored", now, &a.id, "an answer on the Uber host"),
        Msg::Brief(_) => audit::Event::bare("ignored", now, "-", "a brief on the Uber host"),
    };
    audit::append(uber_dir, &ev)?;
    Ok(ev)
}

fn ingest_ask(
    uber_dir: &Path,
    ask: &Ask,
    from_host: &str,
    from_pane: &str,
    now: &str,
) -> std::io::Result<audit::Event> {
    if let Some(mut r) = load(uber_dir, &ask.id) {
        if r.from_host != from_host {
            return Ok(audit::Event::of(
                &r,
                "ignored",
                now,
                "resend from another host",
            ));
        }
        // A resend (new epoch): the UAC's state is the truth, so even an
        // ask refused for good opens again; a sent one waits for its result.
        r.ask = ask.clone();
        r.from_pane = from_pane.into();
        if r.state == State::Refused {
            // The tag was free while refused; take a new one if it is held.
            if by_tag(uber_dir, &r.tag).is_some() {
                match assign_tag(uber_dir, &r.ask.id) {
                    Some(t) => r.tag = t,
                    None => return Ok(audit::Event::of(&r, "ignored", now, "no free tag")),
                }
            }
            r.state = State::Open;
            r.result = None;
        }
        save(uber_dir, &r)?;
        return Ok(audit::Event::of(&r, "resent", now, ""));
    }
    let Some(tag) = assign_tag(uber_dir, &ask.id) else {
        return Ok(audit::Event::bare("ignored", now, &ask.id, "no free tag"));
    };
    let r = Received {
        ask: ask.clone(),
        tag,
        from_host: from_host.into(),
        from_pane: from_pane.into(),
        at: now.into(),
        floor: capture::count(uber_dir),
        state: State::Open,
        used: vec![],
        result: None,
    };
    save(uber_dir, &r)?;
    let detail = format!("{} from {from_pane} on {from_host}", r.ask.project);
    Ok(audit::Event::of(&r, "received", now, &detail))
}
