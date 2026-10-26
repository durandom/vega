//! The UAC host's open asks (docs/design/uber-uac.md §3-4, vega-lhc.209.4):
//! one file per ask, `$XDG_STATE_HOME/vega/uber/asks/<id>.json`. This state
//! is the truth: `vega uber ask` writes it, vegad resends open asks to a new
//! Uber epoch and withdraws asks whose bead closed, and vegad's answer check
//! (§4) reads it. A closed ask keeps its file, so a late answer is refused as
//! `answered`; `prune` drops closed asks after `KEEP_CLOSED`.
//!
//! Transitions (`close`, `resend`) run under a lock on the directory, so the
//! first one wins.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vega_uber_msg::Ask;

/// Closed asks stay this long (seconds) before `prune` removes them.
pub const KEEP_CLOSED: i64 = 30 * 86400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    /// A verified `uber.answer` was applied.
    Answered,
    /// `uber.withdraw` was sent: answered elsewhere, or obsolete.
    Withdrawn,
}

/// The claim an ask was (re)sent to; only its answer counts (§4 check 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentTo {
    pub host: String,
    pub pane: String,
    pub epoch: u64,
}

impl From<&crate::UberClaim> for SentTo {
    fn from(c: &crate::UberClaim) -> Self {
        SentTo {
            host: c.host.clone(),
            pane: c.pane.clone(),
            epoch: c.epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAsk {
    pub schema: u32,
    /// The `uber.ask` body as sent.
    pub ask: Ask,
    /// The UAC that asked: the answer's addressee and the resend's sender.
    pub uac_pane: String,
    pub uac_host: String,
    /// The project checkout the UAC asked from; bd runs there.
    pub cwd: PathBuf,
    pub sent_to: SentTo,
    /// Unix seconds of the last send.
    pub sent_at: i64,
    pub state: State,
    /// Unix seconds of `close`.
    pub closed_at: Option<i64>,
}

/// `$XDG_STATE_HOME/vega/uber/asks` (else `~/.local/state/...`).
pub fn dir() -> Option<PathBuf> {
    let any = vega_herdr::state::state_file_on("uber", None, "x")?;
    Some(any.parent()?.join("asks"))
}

/// The file of ask `id`; `None` for an id that is not `<bead>~<nuid>`, so
/// an id from mail never names another path.
fn path(dir: &Path, id: &str) -> Option<PathBuf> {
    vega_uber_msg::id_bead(id).ok()?;
    Some(dir.join(format!("{id}.json")))
}

/// Ask `id` in any state; `None` when there is no such ask.
pub fn load(dir: &Path, id: &str) -> Option<OpenAsk> {
    let text = std::fs::read_to_string(path(dir, id)?).ok()?;
    let a: OpenAsk = serde_json::from_str(&text).ok()?;
    (a.schema == 1 && a.ask.id == id).then_some(a)
}

/// Every ask in `dir`, oldest send first.
pub fn list(dir: &Path) -> Vec<OpenAsk> {
    let mut asks: Vec<OpenAsk> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            load(dir, name.strip_suffix(".json")?)
        })
        .collect();
    asks.sort_by(|a, b| (a.sent_at, &a.ask.id).cmp(&(b.sent_at, &b.ask.id)));
    asks
}

/// Writes ask `a` atomically.
pub fn save(dir: &Path, a: &OpenAsk) -> std::io::Result<()> {
    let file = path(dir, &a.ask.id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "ask id is not <bead>~<nuid>",
        )
    })?;
    std::fs::create_dir_all(dir)?;
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(
        &tmp,
        serde_json::to_string(a).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(tmp, file)
}

/// Removes ask `id`'s file (a send that failed); a missing file is fine.
pub fn remove(dir: &Path, id: &str) -> std::io::Result<()> {
    let Some(file) = path(dir, id) else {
        return Ok(());
    };
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Holds the directory lock until dropped.
fn lock(dir: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(dir)?;
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".lock"))?;
    f.lock()?;
    Ok(f)
}

/// Applies `f` to ask `id` under the lock if it is open; false otherwise.
fn update_open(dir: &Path, id: &str, f: impl FnOnce(&mut OpenAsk)) -> std::io::Result<bool> {
    let _lock = lock(dir)?;
    let Some(mut a) = load(dir, id).filter(|a| a.state == State::Open) else {
        return Ok(false);
    };
    f(&mut a);
    save(dir, &a)?;
    Ok(true)
}

/// Open -> `to` (answered or withdrawn) at `now`. False when the ask is not
/// open: the first transition wins (§4 check 4).
pub fn close(dir: &Path, id: &str, to: State, now: i64) -> std::io::Result<bool> {
    update_open(dir, id, |a| {
        a.state = to;
        a.closed_at = Some(now);
    })
}

/// Records that the open ask `id` went to `to` at `now`; from then on only
/// that claim's answer counts (§2 New epoch). False when it is not open.
pub fn resend(dir: &Path, id: &str, to: SentTo, now: i64) -> std::io::Result<bool> {
    update_open(dir, id, |a| {
        a.sent_to = to;
        a.sent_at = now;
    })
}

/// Removes asks closed more than `KEEP_CLOSED` before `now`.
pub fn prune(dir: &Path, now: i64) {
    for a in list(dir) {
        if a.closed_at.is_some_and(|t| now - t > KEEP_CLOSED) {
            let _ = remove(dir, &a.ask.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vega_uber_msg::{Level, Opt};

    pub(crate) fn sample(id: &str, sent_at: i64) -> OpenAsk {
        let bead = vega_uber_msg::id_bead(id).unwrap().to_string();
        OpenAsk {
            schema: 1,
            ask: Ask {
                id: id.into(),
                project: "github.com/durandom/vega".into(),
                bead,
                level: Level::L3,
                urgent: None,
                question: "Ship it?".into(),
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
                deadline: "no-rush".into(),
            },
            uac_pane: "w1:p1".into(),
            uac_host: "proteus".into(),
            cwd: "/src/vega".into(),
            sent_to: SentTo {
                host: "ganymede".into(),
                pane: "w9:p1".into(),
                epoch: 10,
            },
            sent_at,
            state: State::Open,
            closed_at: None,
        }
    }

    const ID: &str = "vega-1~ABCDEFGHIJKLMNOPQRSTUV";
    const ID2: &str = "vega-2~ABCDEFGHIJKLMNOPQRSTUV";

    #[test]
    fn save_load_list_round_trip_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("asks");
        assert_eq!(load(&d, ID), None);
        assert!(list(&d).is_empty());
        save(&d, &sample(ID, 20)).unwrap();
        save(&d, &sample(ID2, 10)).unwrap();
        assert_eq!(load(&d, ID), Some(sample(ID, 20)));
        let ids: Vec<_> = list(&d).into_iter().map(|a| a.ask.id).collect();
        assert_eq!(ids, [ID2, ID]);
    }

    #[test]
    fn ids_that_are_not_ask_ids_name_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path(), "../claim"), None);
        assert_eq!(path(dir.path(), "vega-1~short"), None);
        let mut bad = sample(ID, 1);
        bad.ask.id = "../x~ABCDEFGHIJKLMNOPQRSTUV".into();
        assert!(save(dir.path(), &bad).is_err());
    }

    #[test]
    fn the_first_close_wins_and_a_closed_ask_is_not_resent() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        save(d, &sample(ID, 1)).unwrap();
        let new = SentTo {
            host: "titan".into(),
            pane: "w2:p1".into(),
            epoch: 11,
        };
        assert!(resend(d, ID, new.clone(), 5).unwrap());
        assert_eq!(load(d, ID).unwrap().sent_to, new);
        assert!(close(d, ID, State::Answered, 7).unwrap());
        assert!(!close(d, ID, State::Withdrawn, 8).unwrap());
        assert!(!resend(d, ID, new, 9).unwrap());
        let a = load(d, ID).unwrap();
        assert_eq!(
            (a.state, a.closed_at, a.sent_at),
            (State::Answered, Some(7), 5)
        );
        assert!(!close(d, ID2, State::Answered, 7).unwrap());
    }

    #[test]
    fn prune_drops_only_long_closed_asks() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        save(d, &sample(ID, 1)).unwrap();
        save(d, &sample(ID2, 1)).unwrap();
        close(d, ID2, State::Withdrawn, 100).unwrap();
        prune(d, 100 + KEEP_CLOSED);
        assert_eq!(list(d).len(), 2);
        prune(d, 101 + KEEP_CLOSED);
        let ids: Vec<_> = list(d).into_iter().map(|a| a.ask.id).collect();
        assert_eq!(ids, [ID]);
    }
}
