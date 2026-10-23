//! What happened to this host's checkouts (docs/design/janitor.md, History):
//! every save diffs the old rows against the new ones and appends events.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection};

use crate::scan::Entry;

/// Events older than this are dropped on write.
const KEEP: i64 = 365 * 86400;

pub fn create(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS event (
           path TEXT NOT NULL, at INTEGER NOT NULL, what TEXT NOT NULL, detail TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS event_path ON event (path, at);",
    )
}

/// One change of a checkout.
#[derive(Debug, PartialEq)]
pub struct Event {
    pub path: String,
    pub at: i64,
    pub what: String,
    pub detail: String,
}

/// The events between `old` (or none: it appeared) and `new`.
pub fn diff(old: Option<&Entry>, new: &Entry, at: i64) -> Vec<Event> {
    let ev = |what: &str, detail: String| Event {
        path: new.path.clone(),
        at,
        what: what.into(),
        detail,
    };
    let mut out = Vec::new();
    let branch = |e: &Entry| e.branch.clone().unwrap_or_else(|| "(detached)".into());
    let Some(old) = old else {
        out.push(ev(
            "appeared",
            format!("{} on {}", new.kind.as_str(), branch(new)),
        ));
        if new.dirty > 0 && !new.slow {
            out.push(ev("dirty", format!("{} files", new.dirty)));
        }
        return out;
    };
    if old.branch != new.branch {
        out.push(ev("branch", format!("{} -> {}", branch(old), branch(new))));
    }
    // HEAD moved on the same branch: a commit (or pull, reset). A row from
    // before `head` was recorded has none: no event.
    if old.branch == new.branch && old.head.is_some() && new.head.is_some() && old.head != new.head
    {
        out.push(ev("commit", branch(new)));
    }
    // A cut-off status (`slow`) knows no dirty count: no event either way.
    if !old.slow && !new.slow {
        if old.dirty == 0 && new.dirty > 0 {
            out.push(ev("dirty", format!("{} files", new.dirty)));
        } else if old.dirty > 0 && new.dirty == 0 {
            out.push(ev("clean", String::new()));
        }
    }
    out
}

/// Appends the events of a save: `old` rows by path (JSON), the new entries
/// of `changed` paths, and the paths that are gone.
pub fn record(
    db: &Connection,
    old: &BTreeMap<String, String>,
    entries: &[Entry],
    changed: &BTreeSet<&str>,
    gone: &[&String],
    at: i64,
) -> rusqlite::Result<()> {
    let mut events = Vec::new();
    for e in entries
        .iter()
        .filter(|e| !e.prunable && changed.contains(e.path.as_str()))
    {
        let before: Option<Entry> = old.get(&e.path).and_then(|j| serde_json::from_str(j).ok());
        events.extend(diff(before.as_ref(), e, at));
    }
    for p in gone {
        let before: Option<Entry> = old.get(*p).and_then(|j| serde_json::from_str(j).ok());
        if before.as_ref().is_some_and(|b| !b.prunable) {
            let remote = before.and_then(|b| b.remote).unwrap_or_default();
            events.push(Event {
                path: (*p).clone(),
                at,
                what: "vanished".into(),
                detail: remote,
            });
        }
    }
    for e in events {
        db.execute(
            "INSERT INTO event (path, at, what, detail) VALUES (?1, ?2, ?3, ?4)",
            params![e.path, e.at, e.what, e.detail],
        )?;
    }
    db.execute("DELETE FROM event WHERE at < ?1", [at - KEEP])?;
    Ok(())
}

/// When each path last turned dirty.
pub fn last_dirty(db: &Connection) -> rusqlite::Result<BTreeMap<String, i64>> {
    let mut stmt =
        db.prepare("SELECT path, max(at) FROM event WHERE what = 'dirty' GROUP BY path")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// Paths that vanished and did not come back, with their remote.
pub fn vanished(
    db: &Connection,
    current: &BTreeSet<&str>,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = db.prepare(
        // A bare column next to max() comes from the row with the max.
        "SELECT path, detail, max(at) FROM event WHERE what = 'vanished' GROUP BY path",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows
        .filter_map(Result::ok)
        .filter(|(p, _)| !current.contains(p.as_str()))
        .collect())
}

/// The events of `paths`, newest first.
pub fn events(db: &Connection, paths: &[String], limit: usize) -> rusqlite::Result<Vec<Event>> {
    let mut out = Vec::new();
    let mut stmt = db.prepare(
        "SELECT at, what, detail FROM event WHERE path = ?1 ORDER BY at DESC, rowid DESC",
    )?;
    for p in paths {
        let rows = stmt.query_map([p], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for (at, what, detail) in rows.filter_map(Result::ok) {
            out.push(Event {
                path: p.clone(),
                at,
                what,
                detail,
            });
        }
    }
    out.sort_by(|a, b| b.at.cmp(&a.at));
    if limit > 0 {
        out.truncate(limit);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Kind;

    fn entry(branch: &str, dirty: u32, head: &str) -> Entry {
        serde_json::from_value(serde_json::json!({
            "path": "/src/x", "kind": "clone", "gitdir": "", "common": "", "main": null,
            "remote": null, "fork": false, "branch": branch, "upstream": null, "ahead": 0,
            "behind": 0, "dirty": dirty, "slow": false, "last_commit": 1, "head": head, "touched": 0,
            "default_branch": null, "merged": false, "gone": false, "merged_branches": [],
            "prunable": false, "fp": []
        }))
        .unwrap()
    }

    #[test]
    fn diffs_name_what_changed() {
        let what = |old: Option<&Entry>, new: &Entry| -> Vec<String> {
            diff(old, new, 1).into_iter().map(|e| e.what).collect()
        };
        let a = entry("main", 0, "aa");
        assert_eq!(a.kind, Kind::Clone);
        assert_eq!(what(None, &entry("main", 2, "aa")), ["appeared", "dirty"]);
        assert_eq!(what(Some(&a), &entry("main", 3, "aa")), ["dirty"]);
        assert_eq!(what(Some(&entry("main", 3, "aa")), &a), ["clean"]);
        // Same second, new commit: HEAD tells.
        assert_eq!(what(Some(&a), &entry("main", 0, "bb")), ["commit"]);
        assert_eq!(what(Some(&a), &entry("feat", 0, "bb")), ["branch"]);
        let mut slow = entry("main", 0, "aa");
        slow.slow = true;
        assert!(what(Some(&a), &slow).is_empty(), "unknown dirty: no event");
    }
}
