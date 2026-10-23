//! Other hosts' checkouts (docs/design/janitor.md, Cross-host find/list).
//!
//! janitor stays offline: it writes this host's `summary.json` next to the
//! store, vegad publishes it as `<host>.janitor` in the registry and drops
//! the other hosts' summaries into `peers/<host>.json`, which janitor
//! imports into the `peer` table when a file changes.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use vega_herdr::collapse_home;

use crate::scan::{Entry, Kind};

/// Peer rows older than this are flagged `stale` (the broker or vegad is away).
pub const STALE: i64 = 24 * 3600;

/// One checkout as another host sees it: path (home-relative), kind, remote,
/// branch, dirty (-1: unknown), ahead, behind, touched.
pub type Row = (
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    u32,
    u32,
    i64,
);

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct Summary {
    /// When the rows last changed (Unix seconds).
    pub at: i64,
    pub rows: Vec<Row>,
}

/// A host whose rows are imported, and when they last changed there.
#[derive(Clone, Debug, PartialEq)]
pub struct Host {
    pub name: String,
    pub at: i64,
    pub rows: usize,
}

pub fn create(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS peer (
           host TEXT NOT NULL, path TEXT NOT NULL, kind TEXT NOT NULL, remote TEXT,
           branch TEXT, dirty INTEGER, ahead INTEGER, behind INTEGER, touched INTEGER,
           at INTEGER NOT NULL, PRIMARY KEY (host, path));",
    )
}

fn rows(entries: &[Entry]) -> Vec<Row> {
    entries
        .iter()
        .filter(|e| !e.prunable && e.host.is_none())
        .map(|e| {
            (
                collapse_home(&e.path),
                e.kind.as_str().to_string(),
                e.remote.clone(),
                e.branch.clone(),
                if e.slow { -1 } else { i64::from(e.dirty) },
                e.ahead,
                e.behind,
                e.touched,
            )
        })
        .collect()
}

/// Writes `summary.json` in `dir` when the rows changed (atomic): its `at`
/// moves only then, so vegad puts and peers import only real changes.
pub fn write_summary(dir: &Path, entries: &[Entry], now: i64) -> Result<(), String> {
    let path = dir.join("summary.json");
    let rows = rows(entries);
    let old: Option<Summary> = fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    if old.is_some_and(|o| o.rows == rows) {
        return Ok(());
    }
    let json = serde_json::to_vec(&Summary { at: now, rows }).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".summary.{}.tmp", std::process::id()));
    fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Imports `dir/peers/<host>.json` whose mtime moved since the last import,
/// drops hosts whose file is gone, skips `own`. A file that does not parse is
/// left as it was imported last.
pub fn import(db: &mut Connection, dir: &Path, own: &str) -> rusqlite::Result<()> {
    let mut files: BTreeMap<String, i64> = BTreeMap::new();
    if let Ok(rd) = fs::read_dir(dir.join("peers")) {
        for e in rd.flatten() {
            let p = e.path();
            let Some(host) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            if host != own && !host.starts_with('.') {
                files.insert(host.to_string(), crate::scan::mtime(&p));
            }
        }
    }
    let tx = db.transaction()?;
    let imported: BTreeMap<String, i64> = {
        let mut stmt =
            tx.prepare("SELECT substr(key, 6), value FROM meta WHERE key LIKE 'peer.%'")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.filter_map(Result::ok)
            .map(|(h, v)| (h, v.parse().unwrap_or(0)))
            .collect()
    };
    for host in imported.keys().filter(|h| !files.contains_key(*h)) {
        tx.execute("DELETE FROM peer WHERE host = ?1", [host])?;
        tx.execute("DELETE FROM meta WHERE key = ?1", [format!("peer.{host}")])?;
    }
    for (host, mtime) in &files {
        if imported.get(host) == Some(mtime) {
            continue;
        }
        let Some(s) = fs::read(dir.join("peers").join(format!("{host}.json")))
            .ok()
            .and_then(|b| serde_json::from_slice::<Summary>(&b).ok())
        else {
            continue;
        };
        tx.execute("DELETE FROM peer WHERE host = ?1", [host])?;
        for (path, kind, remote, branch, dirty, ahead, behind, touched) in s.rows {
            tx.execute(
                "INSERT OR REPLACE INTO peer
                 (host, path, kind, remote, branch, dirty, ahead, behind, touched, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    host, path, kind, remote, branch, dirty, ahead, behind, touched, s.at
                ],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            [format!("peer.{host}"), mtime.to_string()],
        )?;
    }
    tx.commit()
}

/// The peer rows as entries (with `host`), and the hosts they come from.
pub fn load(db: &Connection) -> rusqlite::Result<(Vec<Entry>, Vec<Host>)> {
    let mut stmt = db.prepare(
        "SELECT host, path, kind, remote, branch, dirty, ahead, behind, touched, at
         FROM peer ORDER BY host, path",
    )?;
    let mut hosts: BTreeMap<String, Host> = BTreeMap::new();
    let entries = stmt
        .query_map([], |r| {
            let host: String = r.get(0)?;
            let kind = match r.get::<_, String>(2)?.as_str() {
                "worktree" => Kind::Worktree,
                "bare" => Kind::Bare,
                _ => Kind::Clone,
            };
            let dirty: i64 = r.get(5)?;
            Ok((
                r.get::<_, i64>(9)?,
                Entry {
                    path: r.get(1)?,
                    kind,
                    gitdir: String::new(),
                    common: String::new(),
                    main: None,
                    remote: r.get(3)?,
                    fork: false,
                    branch: r.get(4)?,
                    upstream: None,
                    ahead: r.get(6)?,
                    behind: r.get(7)?,
                    dirty: dirty.max(0) as u32,
                    slow: dirty < 0,
                    last_commit: None,
                    head: None,
                    touched: r.get(8)?,
                    default_branch: None,
                    merged: false,
                    gone: false,
                    merged_branches: Vec::new(),
                    prunable: false,
                    fp: Vec::new(),
                    skills: Vec::new(),
                    host: Some(host),
                },
            ))
        })?
        .filter_map(Result::ok)
        .map(|(at, e)| {
            let name = e.host.clone().unwrap_or_default();
            let h = hosts
                .entry(name.clone())
                .or_insert(Host { name, at, rows: 0 });
            h.rows += 1;
            e
        })
        .collect();
    Ok((entries, hosts.into_values().collect()))
}
