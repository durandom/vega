//! The Inbox tab (docs/design/board-ui.md): one list of what needs the
//! Slayer. Open decisions (the loaded beads), headed panes waiting on him
//! (vegad's badge file, `crate::badges`) and notes (`crate::notes`, quarantine
//! included). Nothing here runs a command while drawing: the sources are
//! loaded elsewhere, this module only merges them.

use crate::app::App;
use crate::bd::types::Bead;
use crate::decisions::{self, is_decision};
use crate::input::InputKind;
use crate::model::View;
use crate::notes::NoteRow;
use vega_herdr::badges::Waiting;
use vega_herdr::theme::{Topic, DECISIONS, NOTES, WAITING};

/// The kind of an Inbox row; the badges filter the Inbox to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Decision,
    Waiting,
    Note,
}

impl Kind {
    /// The theme topic whose symbol and colour the row wears.
    pub fn topic(self) -> &'static Topic {
        match self {
            Kind::Decision => &DECISIONS,
            Kind::Waiting => &WAITING,
            Kind::Note => &NOTES,
        }
    }

    /// The kind chip's text.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Decision => "decisions",
            Kind::Waiting => "waiting",
            Kind::Note => "notes",
        }
    }

    /// The kind a badge topic stands for.
    pub fn of_topic(t: &Topic) -> Option<Kind> {
        [Kind::Decision, Kind::Waiting, Kind::Note]
            .into_iter()
            .find(|k| k.topic().name == t.name)
    }
}

/// Where a row comes from, with what Enter, `m` and `c` need.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Decision { id: String, title: String },
    Waiting(Waiting),
    Note(NoteRow),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Stable across reloads: the selection follows it.
    pub key: String,
    /// Unix seconds; `None` for a waiting pane (it waits now).
    pub at: Option<i64>,
    pub source: Source,
}

impl Item {
    pub fn kind(&self) -> Kind {
        match self.source {
            Source::Decision { .. } => Kind::Decision,
            Source::Waiting(_) => Kind::Waiting,
            Source::Note(_) => Kind::Note,
        }
    }

    /// The pane Enter focuses, if any.
    pub fn pane(&self) -> Option<&str> {
        match &self.source {
            Source::Decision { .. } => None,
            Source::Waiting(w) => Some(&w.pane),
            Source::Note(n) => n.pane.as_deref(),
        }
    }

    /// The decision bead `m` answers, if any.
    pub fn bead(&self) -> Option<&str> {
        match &self.source {
            Source::Decision { id, .. } => Some(id),
            Source::Waiting(_) => None,
            Source::Note(n) => n.bead.as_deref(),
        }
    }
}

/// Every row: waiting panes first (they wait now), then decisions and notes
/// newest first; a row without a time goes last.
pub fn items(
    beads: &[Bead],
    tracker: Option<&str>,
    waiting: &[Waiting],
    notes: &[NoteRow],
) -> Vec<Item> {
    let mut timed: Vec<Item> = Vec::new();
    if let Some(t) = tracker {
        timed.extend(beads.iter().filter(|b| is_decision(b, t)).map(|b| Item {
            key: format!("d:{}", b.id),
            at: b.created_at.as_deref().and_then(decisions::parse_rfc3339),
            source: Source::Decision {
                id: b.id.clone(),
                title: b.title.clone(),
            },
        }));
    }
    timed.extend(notes.iter().map(|n| Item {
        key: format!("n:{}:{}", n.host, n.id),
        at: Some(n.at()),
        source: Source::Note(n.clone()),
    }));
    timed.sort_by_key(|i| (i.at.is_none(), std::cmp::Reverse(i.at)));
    waiting
        .iter()
        .map(|w| Item {
            key: format!("w:{}", w.pane),
            at: None,
            source: Source::Waiting(w.clone()),
        })
        .chain(timed)
        .collect()
}

impl App {
    /// Every Inbox row, unfiltered.
    pub fn inbox_all(&self) -> Vec<Item> {
        items(
            &self.beads,
            self.tracker.id(),
            &self.badges.waiting,
            &self.notes.rows,
        )
    }

    /// The rows the Inbox shows: all, or the kind its filter names.
    pub fn inbox_items(&self) -> Vec<Item> {
        let mut v = self.inbox_all();
        if let Some(k) = self.inbox_kind {
            v.retain(|i| i.kind() == k);
        }
        v
    }

    /// The tab's count. Before the notes came back, vegad's unread count
    /// stands in for them.
    pub fn inbox_count(&self) -> usize {
        let pending = match (self.notes.loaded, self.badges.now) {
            (false, Some(b)) => b.notes,
            _ => 0,
        };
        self.inbox_all().len() + pending
    }

    /// The selected row; the first when the selection left the list.
    pub fn inbox_selected(&self) -> Option<Item> {
        let v = self.inbox_items();
        let at = self
            .inbox_sel
            .as_ref()
            .and_then(|k| v.iter().position(|i| &i.key == k))
            .unwrap_or(0);
        v.into_iter().nth(at)
    }

    pub fn inbox_nav(&mut self, delta: i32) {
        let v = self.inbox_items();
        if v.is_empty() {
            return;
        }
        let cur = self
            .inbox_sel
            .as_ref()
            .and_then(|k| v.iter().position(|i| &i.key == k))
            .unwrap_or(0);
        let ni = (cur as i64 + delta as i64).clamp(0, v.len() as i64 - 1) as usize;
        self.inbox_sel = Some(v[ni].key.clone());
    }

    /// `N` and the badges: the Inbox, filtered to `kind` or showing all.
    pub fn open_inbox(&mut self, kind: Option<Kind>) {
        self.set_view(View::Inbox);
        self.inbox_kind = kind;
    }

    /// Back to the bead tab the Inbox was opened from.
    pub fn leave_inbox(&mut self) {
        self.set_view(self.last_bead_view);
    }

    /// Enter: a decision opens in the bead tab, a pane gets the focus.
    pub fn inbox_enter(&mut self) {
        let Some(item) = self.inbox_selected() else {
            return;
        };
        match &item.source {
            Source::Decision { id, .. } => {
                let id = id.clone();
                self.leave_inbox();
                self.jump_to(id);
                self.open_detail();
            }
            Source::Note(n) if n.pane.is_some() && !n.local => {
                self.status_msg = format!(
                    "{} is on {}: switch to that machine",
                    n.pane.as_deref().unwrap_or_default(),
                    n.host
                );
            }
            _ => match item.pane() {
                Some(p) => self.jump_pane(p.to_string()),
                None => self.status_msg = "this note names no pane".into(),
            },
        }
    }

    /// `m`: answer the decision the row is or names.
    pub fn inbox_answer(&mut self) {
        let Some(item) = self.inbox_selected() else {
            return;
        };
        match (item.bead(), item.kind()) {
            (Some(id), _) => self.open_input(InputKind::Comment(id.to_string())),
            (None, Kind::Waiting) => {
                self.status_msg = "Enter jumps to the pane: answer it there".into()
            }
            (None, _) => self.status_msg = "this note asks no decision".into(),
        }
    }

    /// `x`: dismiss a decision: close it with a reason, no answer needed.
    pub fn inbox_dismiss(&mut self) {
        let Some(item) = self.inbox_selected() else {
            return;
        };
        match &item.source {
            Source::Decision { id, .. } => self.open_input(InputKind::Dismiss(id.clone())),
            _ => self.status_msg = "only a decision can be dismissed".into(),
        }
    }

    /// `c`: clear a note. Decisions and panes leave by themselves.
    pub fn inbox_clear(&mut self) {
        let Some(item) = self.inbox_selected() else {
            return;
        };
        match &item.source {
            Source::Note(n) => {
                self.notes.clear_through(n);
                self.status_msg = format!("cleared: that note and the older ones of {}", n.host);
            }
            Source::Decision { .. } => {
                self.status_msg =
                    "the UAC closes it after your answer (m), or dismiss it (x)".into()
            }
            Source::Waiting(_) => self.status_msg = "it leaves when the pane moves on".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(host: &str, id: u64) -> NoteRow {
        NoteRow {
            id,
            host: host.into(),
            kind: "ci".into(),
            pane: Some("w1:p1".into()),
            bead: None,
            title: format!("note {id}"),
            body: String::new(),
            unread: true,
            local: true,
        }
    }

    fn decision(id: &str, at: &str) -> Bead {
        Bead {
            id: id.into(),
            issue_type: "decision".into(),
            status: "open".into(),
            labels: vec![decisions::DECISION_LABEL.into()],
            parent: Some("t".into()),
            created_at: Some(at.into()),
            ..Default::default()
        }
    }

    #[test]
    fn waiting_first_then_newest_first_across_kinds() {
        let beads = vec![
            decision("t.1", "2026-10-12T10:00:00Z"),
            decision("t.2", "2026-10-12T12:00:00Z"),
        ];
        let t11 = decisions::parse_rfc3339("2026-10-12T11:00:00Z").unwrap() as u64;
        let notes = vec![note("a", t11 * 1000)];
        let waiting = vec![Waiting {
            pane: "w3V:p2".into(),
            ..Default::default()
        }];
        let keys: Vec<String> = items(&beads, Some("t"), &waiting, &notes)
            .into_iter()
            .map(|i| i.key)
            .collect();
        let n = format!("n:a:{}", t11 * 1000);
        assert_eq!(keys, vec!["w:w3V:p2", "d:t.2", &n, "d:t.1"]);
        assert_eq!(
            items(&beads, None, &[], &[]),
            vec![],
            "no tracker, no decisions"
        );
    }

    #[test]
    fn badge_topics_map_to_kinds() {
        assert_eq!(Kind::of_topic(&NOTES), Some(Kind::Note));
        assert_eq!(Kind::of_topic(&WAITING), Some(Kind::Waiting));
        assert_eq!(Kind::of_topic(&DECISIONS), Some(Kind::Decision));
        assert_eq!(Kind::of_topic(&vega_herdr::theme::OPEN), None);
    }
}
