//! `$XDG_STATE_HOME/vega/uber/audit.jsonl` on the Uber host
//! (docs/design/uber-uac.md §4): each ask received, resent, shown,
//! answered, sent, applied, refused or withdrawn, one JSON line each.
//! `vega uber audit` prints them one line per event, short for the phone.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::received::Received;

/// One audit line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// RFC 3339.
    pub at: String,
    pub event: String,
    pub id: String,
    #[serde(default)]
    pub tag: Option<String>,
    /// The full detail (for `answered`: the `slayer_text`).
    #[serde(default)]
    pub detail: String,
}

impl Event {
    /// An event of a received ask.
    pub fn of(r: &Received, event: &str, at: &str, detail: &str) -> Event {
        Event {
            at: at.into(),
            event: event.into(),
            id: r.ask.id.clone(),
            tag: Some(r.tag.clone()),
            detail: detail.into(),
        }
    }

    /// An event with no record behind it.
    pub fn bare(event: &str, at: &str, id: &str, detail: &str) -> Event {
        Event {
            at: at.into(),
            event: event.into(),
            id: id.into(),
            tag: None,
            detail: detail.into(),
        }
    }

    /// `10-26 13:02Z answered #a7 vega-lhc.209 capture 3: yes — go`: the
    /// bead, not the id's nuid; the detail cut to one short line.
    pub fn line(&self) -> String {
        let at = self
            .at
            .get(5..16)
            .map(|s| format!("{}Z", s.replace('T', " ")))
            .unwrap_or_else(|| self.at.clone());
        let bead = crate::id_bead(&self.id).unwrap_or(&self.id);
        let tag = self
            .tag
            .as_deref()
            .map(|t| format!(" {t}"))
            .unwrap_or_default();
        let one: String = self.detail.split_whitespace().collect::<Vec<_>>().join(" ");
        let detail = if one.chars().count() > 60 {
            format!(": {}…", one.chars().take(59).collect::<String>())
        } else if one.is_empty() {
            String::new()
        } else {
            format!(": {one}")
        };
        format!("{at} {}{tag} {bead}{detail}", self.event)
    }
}

/// `<uber_dir>/audit.jsonl`.
pub fn file(uber_dir: &Path) -> PathBuf {
    uber_dir.join("audit.jsonl")
}

/// Appends one line (one write, append mode).
pub fn append(uber_dir: &Path, ev: &Event) -> std::io::Result<()> {
    std::fs::create_dir_all(uber_dir)?;
    let mut line = serde_json::to_vec(ev)?;
    line.push(b'\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file(uber_dir))?
        .write_all(&line)
}

/// The last `n` events, oldest first; unreadable lines are skipped.
pub fn last(uber_dir: &Path, n: usize) -> Vec<Event> {
    let text = std::fs::read_to_string(file(uber_dir)).unwrap_or_default();
    let all: Vec<Event> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    all[all.len().saturating_sub(n)..].to_vec()
}
