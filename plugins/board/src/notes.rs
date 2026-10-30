//! The notes in the Inbox: what vega showed the human as herdr toasts, kept
//! after the toast is gone (macOS model: toast = banner, this = the history,
//! the UAC pane's email icon = the badge). The notes come from `vega post
//! notes --json` (every host via the registry, else this host's store),
//! loaded in the background. Loading them while the Inbox shows marks them
//! read, which clears the badge.

use crate::bd::output_within;
use crate::uac::resolve_vega;
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// The registry read inside `vega post notes` takes up to 4 s.
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub struct NoteRow {
    pub id: u64,
    pub host: String,
    pub kind: String,
    pub pane: Option<String>,
    pub bead: Option<String>,
    pub title: String,
    pub body: String,
    pub unread: bool,
    /// Written on this host (Enter can focus its pane).
    pub local: bool,
}

impl NoteRow {
    fn parse(line: &str) -> Option<NoteRow> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let s = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
        Some(NoteRow {
            id: v["id"].as_u64()?,
            host: s("host")?,
            kind: s("kind").unwrap_or_default(),
            pane: s("pane"),
            bead: s("bead"),
            title: s("title").unwrap_or_default(),
            body: s("body").unwrap_or_default(),
            unread: v["unread"].as_bool().unwrap_or(false),
            local: v["local"].as_bool().unwrap_or(false),
        })
    }

    /// Unix seconds.
    pub fn at(&self) -> i64 {
        (self.id / 1000) as i64
    }
}

/// Grouped by source (host, then kind), newest first inside a group.
pub fn group(mut rows: Vec<NoteRow>) -> Vec<NoteRow> {
    rows.sort_by(|a, b| {
        (&a.host, &a.kind)
            .cmp(&(&b.host, &b.kind))
            .then(b.id.cmp(&a.id))
    });
    rows
}

/// `<host>=<id>` of the newest row per host: what `read` / `clear` cover.
pub fn upto(rows: &[NoteRow]) -> Vec<String> {
    let mut m: BTreeMap<&str, u64> = BTreeMap::new();
    for r in rows {
        let e = m.entry(&r.host).or_default();
        *e = (*e).max(r.id);
    }
    m.into_iter().map(|(h, id)| format!("{h}={id}")).collect()
}

fn vega(args: &[&str]) -> Result<String> {
    let Some(vega) = resolve_vega() else {
        bail!("vega not found (PATH, ~/.local/bin): run mise run install in the vega repo");
    };
    let mut cmd = Command::new(vega);
    cmd.args(args);
    let out = output_within(cmd, TIMEOUT)?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text
            .lines()
            .nth(1)
            .or(text.lines().next())
            .unwrap_or("failed");
        bail!("vega {}: {}", args.join(" "), line.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Loads the list in the background; the receiver gets one answer.
pub fn load() -> Receiver<Result<Vec<NoteRow>, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let res = vega(&["post", "notes", "--json"])
            .map(|out| group(out.lines().filter_map(NoteRow::parse).collect()))
            .map_err(|e| e.to_string());
        let _ = tx.send(res);
    });
    rx
}

/// `vega post notes read|clear --upto ...` in the background.
pub fn mark(action: &'static str, upto: Vec<String>) {
    if upto.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let mut args = vec!["post", "notes", action];
        for u in &upto {
            args.extend(["--upto", u.as_str()]);
        }
        let _ = vega(&args);
    });
}

/// The notes the Inbox lists; the rows stay while a reload runs.
#[derive(Default)]
pub struct Notes {
    pub rows: Vec<NoteRow>,
    pub loading: Option<Receiver<Result<Vec<NoteRow>, String>>>,
    pub error: Option<String>,
    /// A load came back once: `rows` is the list, not "not asked yet".
    pub loaded: bool,
}

impl Notes {
    /// Loads in the background unless a load runs.
    pub fn reload(&mut self) {
        if self.loading.is_none() {
            self.loading = Some(load());
        }
    }

    /// Takes the loaded list once it arrived; marks it read.
    pub fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        match rx.try_recv() {
            Ok(Ok(rows)) => {
                mark("read", upto(&rows));
                self.rows = rows;
                self.error = None;
                self.loaded = true;
                self.loading = None;
            }
            Ok(Err(e)) => {
                self.error = Some(e);
                self.loading = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.loading = None,
        }
    }

    /// Hides `r` and, as the store keeps one watermark per host, the older
    /// notes of its host; the store keeps them all.
    pub fn clear_through(&mut self, r: &NoteRow) {
        mark("clear", self.forget(r));
    }

    /// Drops what `clear_through` hides; asks stay until the reload checks their
    /// resolved state on the origin. The watermark to send.
    fn forget(&mut self, r: &NoteRow) -> Vec<String> {
        self.rows
            .retain(|x| x.kind == "ask" || x.host != r.host || x.id > r.id);
        vec![format!("{}={}", r.host, r.id)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(host: &str, kind: &str, id: u64) -> NoteRow {
        NoteRow {
            id,
            host: host.into(),
            kind: kind.into(),
            pane: None,
            bead: None,
            title: String::new(),
            body: String::new(),
            unread: true,
            local: false,
        }
    }

    #[test]
    fn parses_the_json_lines_of_vega_post_notes() {
        let line = r#"{"id":1791000000123,"host":"proteus","kind":"lost-draft","pane":"w3V:p1","bead":null,"title":"vegad: your draft in w3V:p1","body":"hi","unread":true,"local":true}"#;
        let r = NoteRow::parse(line).unwrap();
        assert_eq!(r.pane.as_deref(), Some("w3V:p1"));
        assert_eq!(r.bead, None);
        assert!(r.unread && r.local);
        assert_eq!(r.at(), 1791000000);
        assert!(NoteRow::parse("notes: 0").is_none());
    }

    #[test]
    fn clearing_a_note_hides_the_older_ones_of_its_host() {
        let mut n = Notes {
            rows: vec![
                row("a", "x", 9),
                row("a", "x", 5),
                row("a", "x", 3),
                row("b", "x", 4),
            ],
            ..Default::default()
        };
        let r = n.rows[1].clone();
        assert_eq!(n.forget(&r), vec!["a=5"]);
        let left: Vec<(&str, u64)> = n.rows.iter().map(|r| (r.host.as_str(), r.id)).collect();
        assert_eq!(left, vec![("a", 9), ("b", 4)]);
    }

    #[test]
    fn clearing_history_keeps_asks_until_origin_resolution() {
        let mut notes = Notes {
            rows: vec![row("a", "ask", 1), row("a", "draft", 2)],
            ..Default::default()
        };
        let target = notes.rows[1].clone();
        assert_eq!(notes.forget(&target), vec!["a=2"]);
        assert_eq!(notes.rows, vec![row("a", "ask", 1)]);
    }

    #[test]
    fn grouped_by_source_newest_first_and_upto_per_host() {
        let rows = group(vec![
            row("b", "draft", 5),
            row("a", "lost-draft", 9),
            row("a", "draft", 3),
            row("a", "draft", 7),
        ]);
        let order: Vec<(&str, u64)> = rows.iter().map(|r| (r.host.as_str(), r.id)).collect();
        assert_eq!(order, vec![("a", 7), ("a", 3), ("a", 9), ("b", 5)]);
        assert_eq!(upto(&rows), vec!["a=9", "b=5"]);
    }
}
