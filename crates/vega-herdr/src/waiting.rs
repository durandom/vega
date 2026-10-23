//! Agents waiting for the Slayer: finished (`done`) or asking (`blocked`)
//! and not looked at since (docs/design/mux-next.md, vega-lhc.145).
//!
//! herdr's own `done` turns `idle` as soon as anything marks the pane seen:
//! a tab visit marks every pane of the tab, a herdr restart all of them. vegad
//! keeps this ledger instead, from herdr's status and focus events: a pane
//! waits from its first `done` or `blocked` until it works again, its
//! question goes away, it closes, or it gets the focus. herdr's later
//! `done` -> `idle` alone changes nothing.
//!
//! One JSON file, `$XDG_STATE_HOME/vega/waiting.json`, written by vegad only
//! (temp file, rename); vegad publishes it as `waiting` on the registry's
//! agent entries and `vega mux next` picks the oldest.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One waiting pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    /// `done` or `blocked`.
    pub status: String,
    /// When it started waiting (Unix ms); a switch between done and blocked
    /// keeps it.
    pub since: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub panes: BTreeMap<String, Waiting>,
    /// The pane herdr last reported focused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

impl Ledger {
    /// A status event. Returns whether the ledger changed.
    pub fn status(&mut self, pane: &str, status: &str, now: i64) -> bool {
        match status {
            "done" | "blocked" => match self.panes.get_mut(pane) {
                Some(w) if w.status == status => false,
                Some(w) => {
                    w.status = status.into();
                    true
                }
                None => {
                    let w = Waiting {
                        status: status.into(),
                        since: now,
                    };
                    self.panes.insert(pane.into(), w);
                    true
                }
            },
            "working" => self.panes.remove(pane).is_some(),
            // A question that went away was answered. A finished agent herdr
            // calls seen counts only when it is the focused pane (the Slayer
            // came back to the terminal); a tab visit does not.
            "idle" => {
                let answered = self.panes.get(pane).is_some_and(|w| w.status == "blocked");
                if answered || self.focus.as_deref() == Some(pane) {
                    self.panes.remove(pane).is_some()
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// The pane got the focus: the Slayer looks at it.
    pub fn focused(&mut self, pane: &str) -> bool {
        self.focus = Some(pane.into());
        self.panes.remove(pane).is_some()
    }

    pub fn gone(&mut self, pane: &str) -> bool {
        if self.focus.as_deref() == Some(pane) {
            self.focus = None;
        }
        self.panes.remove(pane).is_some()
    }

    /// Every live pane and its status: panes not in it are gone.
    pub fn snapshot<'a>(
        &mut self,
        panes: impl IntoIterator<Item = (&'a String, &'a String)> + Clone,
        now: i64,
    ) -> bool {
        let live: Vec<&String> = panes.clone().into_iter().map(|(p, _)| p).collect();
        let before = self.panes.len();
        self.panes.retain(|p, _| live.contains(&p));
        let mut changed = self.panes.len() != before;
        for (p, s) in panes {
            changed |= self.status(p, s, now);
        }
        changed
    }

    /// The registry value of `pane`: `{status, since}` while it waits.
    pub fn entry(&self, pane: &str) -> Option<serde_json::Value> {
        let w = self.panes.get(pane)?;
        Some(serde_json::json!({"status": w.status, "since": w.since}))
    }
}

/// A waiting agent somewhere: what `vega mux next` chooses from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub host: String,
    pub pane: String,
    pub name: String,
    pub status: String,
    pub since: i64,
}

/// The longest-waiting candidate whose status is in `statuses`; ties go to
/// the host and pane that sort first.
pub fn pick<'a>(candidates: &'a [Candidate], statuses: &[String]) -> Option<&'a Candidate> {
    candidates
        .iter()
        .filter(|c| statuses.contains(&c.status))
        .min_by(|a, b| (a.since, &a.host, &a.pane).cmp(&(b.since, &b.host, &b.pane)))
}

/// `$XDG_STATE_HOME/vega/waiting.json` (default `~/.local/state`).
pub fn path() -> Option<PathBuf> {
    let state = env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("HOME").ok().map(|h| format!("{h}/.local/state")))?;
    Some(Path::new(&state).join("vega").join("waiting.json"))
}

/// The ledger at `file`; empty when there is none or it is unreadable (it
/// refills from herdr's next statuses).
pub fn load(file: &Path) -> Ledger {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// The ledger at the default path.
pub fn read() -> Ledger {
    path().map(|p| load(&p)).unwrap_or_default()
}

/// Writes `ledger` to `file` atomically.
pub fn save(file: &Path, ledger: &Ledger) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(ledger).expect("ledger serializes");
    std::fs::write(&tmp, json + "\n")?;
    std::fs::rename(tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn since(l: &Ledger, pane: &str) -> Option<(String, i64)> {
        l.panes.get(pane).map(|w| (w.status.clone(), w.since))
    }

    #[test]
    fn done_and_blocked_enter_once() {
        let mut l = Ledger::default();
        assert!(l.status("w1:p1", "done", 10));
        assert!(!l.status("w1:p1", "done", 20));
        assert!(l.status("w1:p1", "blocked", 30));
        assert_eq!(since(&l, "w1:p1"), Some(("blocked".into(), 10)));
        assert!(!l.status("w1:p2", "idle", 40));
        assert!(!l.status("w1:p2", "unknown", 40));
        assert!(!l.panes.contains_key("w1:p2"));
    }

    #[test]
    fn work_focus_and_close_end_the_wait() {
        let mut l = Ledger::default();
        for p in ["a", "b", "c"] {
            l.status(p, "done", 1);
        }
        assert!(l.status("a", "working", 2));
        assert!(l.focused("b"));
        assert!(l.gone("c"));
        assert!(l.panes.is_empty());
        assert!(!l.focused("b"));
    }

    #[test]
    fn herdr_seen_alone_keeps_a_finished_agent_waiting() {
        let mut l = Ledger::default();
        l.status("a", "done", 1);
        assert!(!l.status("a", "idle", 2));
        assert_eq!(since(&l, "a"), Some(("done".into(), 1)));
    }

    #[test]
    fn idle_ends_an_answered_question_and_a_focused_pane() {
        let mut l = Ledger::default();
        l.status("a", "blocked", 1);
        assert!(l.status("a", "idle", 2));
        // Done while the Slayer was away from the focused pane; herdr marks it
        // seen on the return.
        l.focused("b");
        l.status("b", "done", 3);
        assert!(l.status("b", "idle", 4));
        assert!(l.panes.is_empty());
    }

    #[test]
    fn snapshot_drops_gone_panes_and_adds_new_waits() {
        let mut l = Ledger::default();
        l.status("gone", "done", 1);
        l.status("kept", "done", 1);
        l.focused("gone");
        let live: BTreeMap<String, String> = [
            ("kept".to_string(), "idle".to_string()),
            ("new".to_string(), "blocked".to_string()),
        ]
        .into();
        assert!(l.snapshot(&live, 5));
        assert_eq!(since(&l, "kept"), Some(("done".into(), 1)));
        assert_eq!(since(&l, "new"), Some(("blocked".into(), 5)));
        assert!(!l.panes.contains_key("gone"));
        assert!(!l.snapshot(&live, 6));
    }

    #[test]
    fn pick_takes_the_oldest_of_the_wanted_statuses() {
        let c = |host: &str, pane: &str, status: &str, since: i64| Candidate {
            host: host.into(),
            pane: pane.into(),
            name: pane.into(),
            status: status.into(),
            since,
        };
        let all = [
            c("proteus", "w1:p1", "done", 30),
            c("ganymede", "w2:p1", "blocked", 10),
            c("io", "w3:p1", "done", 20),
        ];
        let done = vec!["done".to_string()];
        assert_eq!(pick(&all, &done).unwrap().host, "io");
        let both = vec!["blocked".to_string(), "done".to_string()];
        assert_eq!(pick(&all, &both).unwrap().host, "ganymede");
        assert!(pick(&[], &done).is_none());
    }

    #[test]
    fn ledger_survives_a_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("vega/waiting.json");
        assert_eq!(load(&file), Ledger::default());
        let mut l = Ledger::default();
        l.status("w1:p1", "done", 7);
        save(&file, &l).unwrap();
        assert_eq!(load(&file), l);
        assert_eq!(
            l.entry("w1:p1"),
            Some(serde_json::json!({"status": "done", "since": 7}))
        );
    }
}
