//! `vega post ask`: one question to several agents on one thread; the relay
//! holds the answers and gives the asker one pointer when every target
//! answered or the deadline passed (docs/design/vegad.md, Gather).
//!
//! The ask lives in `$XDG_STATE_HOME/vega/asks/local_<thread>` on the asker's
//! machine, written before the question goes out, so even a quick answer
//! finds it.

use std::path::PathBuf;

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub asker: String,
    /// `<host>/<pane>` of each target.
    pub targets: Vec<String>,
    pub answered: Vec<String>,
    pub deadline_ms: u128,
    pub question: String,
    pub created_ms: u128,
}

/// What an answer does to its ask.
#[derive(Debug, PartialEq)]
pub enum Answer {
    /// Not an answer to an ask of this pane: deliver as usual.
    NotOurs,
    /// Held until the others answered.
    Held { answered: usize, of: usize },
    /// The last answer: deliver everything now.
    Complete { of: usize },
}

pub fn key(host: &str, pane: &str) -> String {
    format!("{host}/{pane}")
}

impl Ask {
    /// An answer from `from` (`<host>/<pane>`) to `to` on this ask's thread.
    pub fn answer(&mut self, to: &str, from: &str) -> Answer {
        if to != self.asker || !self.targets.iter().any(|t| t == from) {
            return Answer::NotOurs;
        }
        if !self.answered.iter().any(|a| a == from) {
            self.answered.push(from.to_string());
        }
        let of = self.targets.len();
        if self.answered.len() >= of {
            Answer::Complete { of }
        } else {
            Answer::Held {
                answered: self.answered.len(),
                of,
            }
        }
    }

    pub fn to_json(&self) -> String {
        json!({
            "asker": self.asker,
            "targets": self.targets,
            "answered": self.answered,
            "deadline_ms": self.deadline_ms as u64,
            "question": self.question,
            "created_ms": self.created_ms as u64,
        })
        .to_string()
    }

    pub fn from_json(text: &str) -> Option<Ask> {
        let v: Value = serde_json::from_str(text).ok()?;
        let list = |k: &str| -> Option<Vec<String>> {
            v[k].as_array()?
                .iter()
                .map(|x| x.as_str().map(String::from))
                .collect()
        };
        Some(Ask {
            asker: v["asker"].as_str()?.to_string(),
            targets: list("targets")?,
            answered: list("answered")?,
            deadline_ms: u128::from(v["deadline_ms"].as_u64()?),
            question: v["question"].as_str().unwrap_or("").into(),
            created_ms: u128::from(v["created_ms"].as_u64().unwrap_or(0)),
        })
    }
}

fn file(thread: &str) -> Option<PathBuf> {
    crate::state::state_file_on("asks", None, thread)
}

pub fn save(thread: &str, ask: &Ask) {
    if let Some(f) = file(thread) {
        let _ = f.parent().map(std::fs::create_dir_all);
        let _ = atomic_write(&f, &ask.to_json());
    }
    let _ = save_open(thread, ask);
}

pub fn load(thread: &str) -> Option<Ask> {
    Ask::from_json(&std::fs::read_to_string(file(thread)?).ok()?)
}

pub fn remove(thread: &str) {
    for f in [file(thread), open_file(thread)].into_iter().flatten() {
        let _ = std::fs::remove_file(f);
    }
}

fn open_file(thread: &str) -> Option<PathBuf> {
    crate::state::state_file_on("asks-open", None, thread)
}
fn atomic_write(file: &std::path::Path, text: &str) -> bool {
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    file.parent()
        .is_some_and(|p| std::fs::create_dir_all(p).is_ok())
        && std::fs::write(&tmp, text).is_ok()
        && std::fs::rename(&tmp, file).is_ok()
}
pub fn save_open(thread: &str, ask: &Ask) -> bool {
    open_file(thread).is_some_and(|f| atomic_write(&f, &ask.to_json()))
}
pub fn load_open(thread: &str) -> Option<Ask> {
    let ask = Ask::from_json(&std::fs::read_to_string(open_file(thread)?).ok()?)?;
    retained(&ask, u128::from(crate::notes::now_ms())).then_some(ask)
}
/// Deadline-expired question metadata is retained for seven days, not forever.
const OPEN_RETENTION_MS: u128 = 7 * 24 * 60 * 60 * 1000;
fn retained(ask: &Ask, now_ms: u128) -> bool {
    now_ms.saturating_sub(ask.deadline_ms) < OPEN_RETENTION_MS
}
fn temporary(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.starts_with("tmp"))
}
/// Called by the asker's daemon after releasing deadline-expired mail. Retention
/// expiry removes only question metadata; it never records an answer or deletes mail.
pub fn prune_open(now_ms: u128) {
    let Some(dir) = open_file("x").and_then(|f| f.parent().map(|p| p.to_path_buf())) else {
        return;
    };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if temporary(&entry.path()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(thread) = name.strip_prefix("local_") else {
            continue;
        };
        // Keep active gathers until their deadline handling has released mail.
        if file(thread).is_some_and(|p| p.exists()) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path()) {
            if Ask::from_json(&text).is_some_and(|a| !retained(&a, now_ms)) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
/// An ask deadline releases held mail but never proves that the question was answered.
pub fn expire(thread: &str, ask: &Ask) -> bool {
    if !save_open(thread, ask) {
        return false;
    }
    file(thread).is_some_and(|f| std::fs::remove_file(f).is_ok())
}
/// Unresolved asks, including deadline-expired threads. Missing/corrupt sources
/// are errors, not an empty list. Active records cover pre-upgrade open asks.
pub fn open() -> Result<Vec<(String, Ask)>, String> {
    open_at(u128::from(crate::notes::now_ms()))
}
fn open_at(now_ms: u128) -> Result<Vec<(String, Ask)>, String> {
    let mut asks = std::collections::BTreeMap::new();
    let mut available = false;
    for kind in ["asks-open", "asks"] {
        let dir = crate::state::state_file_on(kind, None, "x")
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
            .ok_or("state directory unavailable")?;
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => {
                available = true;
                e
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("ask source unreadable".into()),
        };
        for entry in entries {
            let entry = entry.map_err(|_| "ask source unreadable")?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if temporary(&entry.path()) {
                continue;
            }
            let Some(thread) = name.strip_prefix("local_") else {
                continue;
            };
            let text =
                std::fs::read_to_string(entry.path()).map_err(|_| "ask record unreadable")?;
            let ask = Ask::from_json(&text).ok_or("ask record invalid")?;
            if kind == "asks" || retained(&ask, now_ms) {
                asks.insert(thread.to_string(), ask);
            }
        }
    }
    if !available {
        return Err("ask source absent".into());
    }
    Ok(asks.into_iter().collect())
}
pub fn owes(ask: &Ask, host: &str, pane: &str) -> bool {
    let target = key(host, pane);
    ask.targets.contains(&target) && !ask.answered.contains(&target)
}

/// Every open ask on this machine with its thread.
pub fn all() -> Vec<(String, Ask)> {
    let Some(dir) = file("x").and_then(|f| f.parent().map(|p| p.to_path_buf())) else {
        return Vec::new();
    };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            if temporary(&e.path()) {
                return None;
            }
            let name = e.file_name().into_string().ok()?;
            let thread = name.strip_prefix("local_")?.to_string();
            let ask = Ask::from_json(&std::fs::read_to_string(e.path()).ok()?)?;
            Some((thread, ask))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask() -> Ask {
        Ask {
            asker: "w1:p1".into(),
            targets: vec!["proteus/w1:p2".into(), "ganymede/w5:p1".into()],
            answered: vec![],
            deadline_ms: 42,
            question: "Which owner?".into(),
            created_ms: 1,
        }
    }

    #[test]
    fn answers_are_held_until_the_last() {
        let mut a = ask();
        assert_eq!(a.answer("w1:p1", "proteus/w1:p9"), Answer::NotOurs);
        assert_eq!(a.answer("w1:p7", "proteus/w1:p2"), Answer::NotOurs);
        assert_eq!(
            a.answer("w1:p1", "proteus/w1:p2"),
            Answer::Held { answered: 1, of: 2 }
        );
        // A second message from the same target counts once.
        assert_eq!(
            a.answer("w1:p1", "proteus/w1:p2"),
            Answer::Held { answered: 1, of: 2 }
        );
        assert_eq!(
            a.answer("w1:p1", "ganymede/w5:p1"),
            Answer::Complete { of: 2 }
        );
    }

    #[test]
    fn round_trips_through_json() {
        let a = ask();
        assert_eq!(Ask::from_json(&a.to_json()), Some(a));
        assert_eq!(Ask::from_json("nope"), None);
    }

    #[test]
    fn unanswered_threads_survive_deadline_and_late_replies_resolve_them() {
        if std::env::var_os("VEGA_GATHER_TEST_CHILD").is_none() {
            let dir = tempfile::tempdir().unwrap();
            let result=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","gather::tests::unanswered_threads_survive_deadline_and_late_replies_resolve_them"])
                .env("VEGA_GATHER_TEST_CHILD","1").env("XDG_STATE_HOME",dir.path()).output().unwrap();
            assert!(
                result.status.success(),
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        assert!(open().is_err());
        let mut a = ask();
        a.deadline_ms = u128::from(crate::notes::now_ms());
        save("thread", &a);
        let tmp = file("thread").unwrap().with_extension("tmp123");
        std::fs::write(&tmp, a.to_json()).unwrap();
        assert_eq!(all(), vec![("thread".into(), a.clone())]);
        assert_eq!(open().unwrap(), vec![("thread".into(), a.clone())]);
        assert!(expire("thread", &a));
        assert!(all().is_empty());
        assert!(tmp.exists());
        assert!(load("thread").is_none());
        let mut late = load_open("thread").unwrap();
        assert!(owes(&late, "proteus", "w1:p2"));
        // Reading a pending thread, like reading an inbox, makes no answer.
        assert!(owes(&open().unwrap()[0].1, "proteus", "w1:p2"));
        assert!(matches!(
            late.answer("w1:p1", "proteus/w1:p2"),
            Answer::Held { .. }
        ));
        assert!(save_open("thread", &late));
        assert!(!owes(&load_open("thread").unwrap(), "proteus", "w1:p2"));
        assert_eq!(
            late.answer("w1:p1", "ganymede/w5:p1"),
            Answer::Complete { of: 2 }
        );
        save("thread", &late);
        assert!(open().unwrap()[0]
            .1
            .targets
            .iter()
            .all(|target| open().unwrap()[0].1.answered.contains(target)));
        remove("thread");
        assert!(open().unwrap().is_empty());
        save("abandoned", &a);
        assert!(expire("abandoned", &a));
        let end = a.deadline_ms + OPEN_RETENTION_MS;
        assert_eq!(open_at(end - 1).unwrap().len(), 1);
        assert!(open_at(end).unwrap().is_empty());
        prune_open(end - 1);
        assert!(open_file("abandoned").unwrap().exists());
        prune_open(end);
        assert!(!open_file("abandoned").unwrap().exists());
        assert!(tmp.exists());
        // Retention does not delete a gather whose held mail still needs release.
        save("active", &a);
        prune_open(end);
        assert!(load("active").is_some());
        remove("active");
        let bad = file("bad").unwrap();
        std::fs::write(bad, "{}").unwrap();
        assert!(open().is_err());
    }
}
