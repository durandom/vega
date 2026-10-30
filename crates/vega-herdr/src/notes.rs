//! Notifications vega sent the human, kept after herdr's toast is gone
//! (herdr has one toast slot and no history). Like macOS: the toast is the
//! banner, `vega board` (N) the list, the UAC pane's email badge
//! (crate::badges; docs/design/vegad.md, Notifications).
//!
//! This host's notes live in `$XDG_STATE_HOME/vega/notes/<id>.json`; vegad
//! reconciles them into the registry as `<host>.note.<id>` (local state is
//! the truth, as for agents and links), so a host without registry rights
//! (deimos) keeps them here only. Ids are Unix milliseconds, unique per host,
//! so the newest is the largest. What this host's viewer has read or cleared
//! is a watermark per source host in `notes/seen.json`.
//!
//! A note may name its project (the UAC whose badge counts it) and its
//! cause (`bd-sync:<DB>`); when the source host's vegad sees the cause
//! clear, it marks the note resolved, which counts as read everywhere.
//! Notes from before these fields have neither.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Retention for note history; unresolved asks are exempt.
pub const KEEP: usize = 200;
pub const KEEP_SECS: i64 = 7 * 24 * 3600;

/// One notification. `host` is its source; `pane` where it points (Enter
/// jumps there); `bead` a decision it asks about (`m` answers it);
/// `project` the full project ID whose UAC
/// badge counts it, none for host-level notes; `cause` what clears it;
/// `resolved` when (Unix s) it cleared.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Note {
    pub id: u64,
    pub host: String,
    pub kind: String,
    pub pane: Option<String>,
    pub bead: Option<String>,
    pub project: Option<String>,
    pub cause: Option<String>,
    pub resolved: Option<i64>,
    pub level: Option<String>,
    pub key: Option<String>,
    pub cmd: Option<String>,
    pub acked: Option<i64>,
    pub title: String,
    pub body: String,
}

impl Note {
    pub fn to_json(&self) -> Value {
        json!({
            "schema": 2,
            "id": self.id,
            "host": self.host,
            "kind": self.kind,
            "pane": self.pane,
            "bead": self.bead,
            "project": self.project,
            "cause": self.cause,
            "resolved": self.resolved,
            "level": self.level,
            "key": self.key,
            "cmd": self.cmd,
            "acked": self.acked,
            "title": self.title,
            "body": self.body,
        })
    }

    pub fn from_json(v: &Value) -> Option<Note> {
        if v["schema"] != 2 {
            return None;
        }
        let s = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
        Some(Note {
            id: v["id"].as_u64()?,
            host: s("host")?,
            kind: s("kind").unwrap_or_else(|| "note".into()),
            pane: s("pane"),
            bead: s("bead"),
            project: s("project"),
            cause: s("cause"),
            resolved: v["resolved"].as_i64(),
            level: s("level"),
            key: s("key"),
            cmd: s("cmd"),
            acked: v["acked"].as_i64(),
            title: s("title").unwrap_or_default(),
            body: s("body").unwrap_or_default(),
        })
    }

    /// Unix seconds the note was written.
    pub fn at(&self) -> i64 {
        (self.id / 1000) as i64
    }

    /// Whether the badge of project `key` counts it: its own project's, or
    /// for a note without one (or whose project has no UAC here, not in
    /// `uacs`), the vega project's `home`; every badge without a `home`.
    pub fn counts_on(&self, key: &str, home: Option<&str>, uacs: &[String]) -> bool {
        match &self.project {
            Some(p) if uacs.contains(p) => p == key,
            _ => home.is_none_or(|h| h == key),
        }
    }
}

/// The watermarks of one viewing host: per source host, the last note id
/// read, and the last one cleared (hidden from the list).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    pub read: BTreeMap<String, u64>,
    pub cleared: BTreeMap<String, u64>,
}

impl Seen {
    /// Above the watermarks and not resolved.
    pub fn is_unread(&self, n: &Note) -> bool {
        n.resolved.is_none() && self.unread(&n.host, n.id)
    }

    pub fn is_cleared(&self, n: &Note) -> bool {
        !(n.kind == "ask" && n.resolved.is_none())
            && n.id <= self.cleared.get(&n.host).copied().unwrap_or(0)
    }

    /// A note known only by key (host, id), as vegad sees other hosts' notes.
    pub fn unread(&self, host: &str, id: u64) -> bool {
        id > self.read.get(host).copied().unwrap_or(0)
            && id > self.cleared.get(host).copied().unwrap_or(0)
    }

    /// The unread notes: this host's plus the other hosts' (from the
    /// registry; this host's own notes there are the same notes).
    pub fn unread_of(&self, local: &[Note], remote: &[Note], own: &str) -> Vec<Note> {
        local
            .iter()
            .chain(remote.iter().filter(|n| n.host != own))
            .filter(|n| self.is_unread(n))
            .cloned()
            .collect()
    }

    /// Raises the watermarks to `upto` (never lowers them).
    pub fn mark(&mut self, upto: &BTreeMap<String, u64>, clear: bool) {
        for (h, id) in upto {
            let r = self.read.entry(h.clone()).or_default();
            *r = (*r).max(*id);
            if clear {
                let c = self.cleared.entry(h.clone()).or_default();
                *c = (*c).max(*id);
            }
        }
    }
}

/// Per source host, the newest id among `notes`.
pub fn newest(notes: &[Note]) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for n in notes {
        let e = out.entry(n.host.clone()).or_default();
        *e = (*e).max(n.id);
    }
    out
}

/// The notes directory of one host.
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    /// `$XDG_STATE_HOME/vega/notes` (default `~/.local/state`).
    pub fn local() -> Option<Store> {
        let state = env::var("XDG_STATE_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| env::var("HOME").ok().map(|h| format!("{h}/.local/state")))?;
        Some(Store {
            dir: Path::new(&state).join("vega").join("notes"),
        })
    }

    /// Writes `note` with a fresh id (now in ms, bumped past a taken one),
    /// then prunes. Returns the note.
    pub fn add(&self, mut note: Note, now_ms: u64) -> std::io::Result<Note> {
        std::fs::create_dir_all(&self.dir)?;
        let mut id = now_ms.max(self.list().last().map_or(0, |n| n.id + 1));
        loop {
            note.id = id;
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.dir.join(format!("{id}.json")))
            {
                Ok(mut f) => {
                    use std::io::Write;
                    f.write_all(note.to_json().to_string().as_bytes())?;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => id += 1,
                Err(e) => return Err(e),
            }
        }
        self.prune((now_ms / 1000) as i64);
        Ok(note)
    }

    /// Atomically replace an existing note; callers serialize ask mutations.
    pub fn save(&self, note: &Note) -> std::io::Result<()> {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new_in(&self.dir)?;
        tmp.write_all(note.to_json().to_string().as_bytes())?;
        tmp.persist(self.dir.join(format!("{}.json", note.id)))
            .map_err(|e| e.error)?;
        Ok(())
    }

    /// Every note here, oldest first.
    pub fn list(&self) -> Vec<Note> {
        let mut out: Vec<Note> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|s| serde_json::from_str::<Value>(&s).ok())
            .filter_map(|v| Note::from_json(&v))
            .collect();
        out.sort_by_key(|n| n.id);
        out
    }

    /// Drops history older than KEEP_SECS and all but the newest KEEP;
    /// unresolved asks remain until resolved.
    pub fn prune(&self, now: i64) {
        let notes = self.list();
        let excess = notes.len().saturating_sub(KEEP);
        for (i, n) in notes.iter().enumerate() {
            if n.kind == "ask" && n.resolved.is_none() {
                continue;
            }
            if i < excess || now - n.at() > KEEP_SECS {
                let _ = std::fs::remove_file(self.dir.join(format!("{}.json", n.id)));
            }
        }
    }

    /// Marks the unresolved notes with a cause that `clears` resolved at
    /// `now` (Unix s). Returns how many.
    pub fn resolve(&self, now: i64, clears: impl Fn(&Note) -> bool) -> usize {
        let mut n = 0;
        for mut note in self.list() {
            if note.resolved.is_some() || note.cause.is_none() || !clears(&note) {
                continue;
            }
            note.resolved = Some(now);
            let f = self.dir.join(format!("{}.json", note.id));
            let tmp = self.dir.join(format!("{}.json.tmp", note.id));
            if std::fs::write(&tmp, note.to_json().to_string()).is_ok()
                && std::fs::rename(&tmp, &f).is_ok()
            {
                n += 1;
            }
        }
        n
    }

    fn seen_file(&self) -> PathBuf {
        self.dir.join("seen.json")
    }

    pub fn seen(&self) -> Seen {
        let v: Value = std::fs::read_to_string(self.seen_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let map = |k: &str| -> BTreeMap<String, u64> {
            v[k].as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(h, id)| Some((h.clone(), id.as_u64()?)))
                        .collect()
                })
                .unwrap_or_default()
        };
        Seen {
            read: map("read"),
            cleared: map("cleared"),
        }
    }

    /// Raises the watermarks (see `Seen::mark`) and saves them.
    pub fn mark(&self, upto: &BTreeMap<String, u64>, clear: bool) -> std::io::Result<Seen> {
        let mut seen = self.seen();
        seen.mark(upto, clear);
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("seen.json.tmp");
        std::fs::write(
            &tmp,
            json!({"read": seen.read, "cleared": seen.cleared}).to_string(),
        )?;
        std::fs::rename(tmp, self.seen_file())?;
        Ok(seen)
    }

    /// When a note or the watermarks last changed: every write adds,
    /// renames or removes a file here (vegad redraws the badge then).
    pub fn changed(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(&self.dir).and_then(|m| m.modified()).ok()
    }
}

/// Show an existing note without creating another note. A refused toast is not a send.
pub fn toast(title: &str, body: &str) -> Result<(), crate::Failure> {
    let v = crate::herdr_on(
        None,
        &[
            "notification",
            "show",
            title,
            "--body",
            body,
            "--sound",
            "request",
        ],
        std::time::Duration::from_secs(5),
    )?;
    let result = &v["result"];
    if result["shown"] != true {
        let reason = result["reason"].as_str().unwrap_or("toast was not shown");
        return Err(crate::Failure::new(
            "channel_unavailable",
            format!("toast: {reason}"),
            "retry when a herdr client is available",
        ));
    }
    Ok(())
}

/// A notification for the human: herdr's toast (one slot, gone when the
/// next arrives) plus a note in this host's store for the board's list.
/// Host-level: the vega UAC's badge counts it (see `notify_project`).
pub fn notify(kind: &str, pane: Option<&str>, title: &str, body: &str) {
    notify_project(kind, pane, None, None, title, body)
}

/// `notify` for `project`'s UAC badge, cleared when `cause` clears
/// (`Store::resolve`).
pub fn notify_project(
    kind: &str,
    pane: Option<&str>,
    project: Option<&str>,
    cause: Option<&str>,
    title: &str,
    body: &str,
) {
    let _ = crate::herdr(&[
        "notification",
        "show",
        title,
        "--body",
        body,
        "--sound",
        "request",
    ]);
    if let Some(store) = Store::local() {
        let note = Note {
            host: crate::state::post_host(),
            kind: kind.into(),
            pane: pane.map(String::from),
            project: project.map(String::from),
            cause: cause.map(String::from),
            title: title.into(),
            body: body.into(),
            ..Default::default()
        };
        let _ = store.add(note, now_ms());
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(host: &str, kind: &str, pane: Option<&str>, title: &str) -> Note {
        Note {
            host: host.into(),
            kind: kind.into(),
            pane: pane.map(String::from),
            title: title.into(),
            ..Default::default()
        }
    }

    fn store(name: &str) -> Store {
        let dir = env::temp_dir().join(format!("vega-notes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store { dir }
    }

    #[test]
    fn ids_are_unique_and_rising_within_one_ms() {
        let s = store("ids");
        let t = 1_800_000_000_000;
        let a = s
            .add(note("proteus", "draft", Some("w1:p1"), "a"), t)
            .unwrap();
        let b = s.add(note("proteus", "draft", None, "b"), t).unwrap();
        assert!(b.id > a.id);
        let l = s.list();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].pane.as_deref(), Some("w1:p1"));
        assert_eq!(l[1].title, "b");
    }

    #[test]
    fn prune_keeps_a_week_and_at_most_keep() {
        let s = store("prune");
        let t = 1_800_000_000_000;
        s.add(note("h", "x", None, "old"), t).unwrap();
        let later = t + (KEEP_SECS as u64 + 1) * 1000;
        for _ in 0..KEEP + 3 {
            s.add(note("h", "x", None, "new"), later).unwrap();
        }
        let l = s.list();
        assert_eq!(l.len(), KEEP);
        assert!(l.iter().all(|n| n.title == "new"));
    }

    #[test]
    fn watermarks_rise_and_clear_hides() {
        let s = store("seen");
        let n = |host: &str, id| Note {
            id,
            ..note(host, "x", None, "")
        };
        let notes = vec![n("a", 5), n("a", 7), n("b", 3)];
        assert_eq!(s.seen().read.len(), 0);
        let seen = s.mark(&newest(&notes[..2]), false).unwrap();
        assert_eq!(notes.iter().filter(|x| seen.is_unread(x)).count(), 1);
        // Never lowered.
        s.mark(&BTreeMap::from([("a".into(), 1)]), false).unwrap();
        assert_eq!(s.seen().read["a"], 7);
        // Local "a" notes count once even though the registry lists them too.
        let remote = vec![n("a", 7), n("b", 3), n("b", 4)];
        assert_eq!(seen.unread_of(&notes[..2], &remote, "a").len(), 2);
        let seen = s.mark(&newest(&notes), true).unwrap();
        assert!(notes
            .iter()
            .all(|x| seen.is_cleared(x) && !seen.is_unread(x)));
        assert_eq!(seen.unread_of(&notes[..2], &remote, "a").len(), 1);
    }

    #[test]
    fn host_notes_need_no_project_cause_or_resolved() {
        let v: Value = serde_json::from_str(
            r#"{"schema":2,"id":1791000000123,"host":"proteus","kind":"bd-sync","pane":null,"bead":null,"title":"t","body":"b"}"#,
        )
        .unwrap();
        let n = Note::from_json(&v).unwrap();
        assert_eq!((n.project, n.cause, n.resolved), (None, None, None));
        let mut legacy = v.clone();
        legacy["schema"] = 1.into();
        assert!(Note::from_json(&legacy).is_none());
        assert!(Seen::default().is_unread(&Note::from_json(&v).unwrap()));
    }

    #[test]
    fn a_badge_counts_its_projects_notes_and_the_vega_one_the_rest() {
        let vega = "~/src/vega/.git";
        let x = "~/src/x/.git";
        let uacs = vec![vega.to_string(), x.to_string()];
        let of = |p: Option<&str>| Note {
            project: p.map(String::from),
            ..note("h", "bd-sync", None, "")
        };
        let host_level = of(None);
        assert!(host_level.counts_on(vega, Some(vega), &uacs));
        assert!(!host_level.counts_on(x, Some(vega), &uacs));
        assert!(of(Some(x)).counts_on(x, Some(vega), &uacs));
        assert!(!of(Some(x)).counts_on(vega, Some(vega), &uacs));
        // A project without a UAC here falls back to the vega UAC.
        let orphan = of(Some("~/src/y/.git"));
        assert!(orphan.counts_on(vega, Some(vega), &uacs));
        assert!(!orphan.counts_on(x, Some(vega), &uacs));
        // No vega UAC known: every badge counts the host-level notes.
        assert!(host_level.counts_on(x, None, &uacs));
    }

    #[test]
    fn resolve_marks_matching_notes_read_and_keeps_them() {
        let s = store("resolve");
        let t = 1_800_000_000_000;
        let stop = Note {
            project: Some("~/src/x/.git".into()),
            cause: Some("bd-sync:X".into()),
            ..note("h", "bd-sync", None, "stopped")
        };
        let a = s.add(stop.clone(), t).unwrap();
        let other = Note {
            cause: Some("bd-sync:Y".into()),
            ..stop.clone()
        };
        s.add(other, t).unwrap();
        s.add(note("h", "draft", None, "no cause"), t).unwrap();
        let clears = |n: &Note| n.cause.as_deref() == Some("bd-sync:X");
        assert_eq!(s.resolve(1_800_000_100, clears), 1);
        assert_eq!(s.resolve(1_800_000_200, clears), 0, "once");
        let l = s.list();
        assert_eq!(l.len(), 3);
        let seen = s.seen();
        let got = l.iter().find(|n| n.id == a.id).unwrap();
        assert_eq!(got.resolved, Some(1_800_000_100));
        assert!(!seen.is_unread(got));
        assert_eq!(seen.unread_of(&l, &[], "h").len(), 2);
    }
}
