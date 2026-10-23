//! The cached index: `$XDG_STATE_HOME/vega/janitor/janitor.db` (SQLite),
//! refreshed per entry when its fingerprint moves (docs/design/janitor.md,
//! refresh rules and Storage).

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::history;
use crate::peers;
use crate::scan::{self, Entry, Kind};
use crate::search;

/// Bumped when `Entry` or the tables change shape; an older store is rescanned.
const SCHEMA: u32 = 4;
/// The last JSON cache format (v1), imported once so the move costs no rescan.
const JSON_VERSION: u32 = 3;
/// A cache older than this re-walks the roots on the next call (new and
/// vanished checkouts); only new checkouts are queried.
pub const MAX_AGE: i64 = 24 * 3600;

#[derive(Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub roots: Vec<String>,
    /// Unix time of the last full scan.
    pub scanned_at: i64,
    /// How long that scan took.
    pub scan_ms: u64,
    /// Unix time of the last walk (a full scan walks too).
    pub walked_at: i64,
    pub entries: Vec<Entry>,
    /// Other hosts' checkouts (`peer` rows), filled by `load_fresh`.
    #[serde(skip)]
    pub peers: Vec<Entry>,
    #[serde(skip)]
    pub hosts: Vec<peers::Host>,
    /// When each checkout last turned dirty (History), filled by `load_fresh`.
    #[serde(skip)]
    pub last_dirty: BTreeMap<String, i64>,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `VEGA_JANITOR_ROOTS` (colon-separated), default `~/src`; existing dirs, canonical.
pub fn roots() -> Vec<PathBuf> {
    let home = env::var("HOME").unwrap_or_default();
    let raw = env::var("VEGA_JANITOR_ROOTS")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{home}/src"));
    raw.split(':')
        .filter(|s| !s.is_empty())
        .map(|s| match s.strip_prefix("~/") {
            Some(rest) => PathBuf::from(&home).join(rest),
            None => PathBuf::from(s),
        })
        .filter_map(|p| fs::canonicalize(p).ok())
        .filter(|p| p.is_dir())
        .collect()
}

pub fn cache_path() -> PathBuf {
    let state = env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.local/state", env::var("HOME").unwrap_or_default()));
    PathBuf::from(state).join("vega/janitor/janitor.db")
}

/// Opens (and creates) the store: WAL, so a reader never waits on a writer
/// and never sees half a refresh.
pub fn open(path: &Path) -> Result<Connection, String> {
    let dir = path.parent().ok_or("cache path has no parent")?;
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let err = |e: rusqlite::Error| format!("{}: {e}", path.display());
    let db = Connection::open(path).map_err(err)?;
    db.busy_timeout(Duration::from_secs(5)).map_err(err)?;
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS checkout (
           path TEXT PRIMARY KEY, remote TEXT, touched INTEGER, entry TEXT NOT NULL);",
    )
    .map_err(err)?;
    peers::create(&db).map_err(err)?;
    search::create(&db).map_err(err)?;
    history::create(&db).map_err(err)?;
    Ok(db)
}

fn meta(db: &Connection, key: &str) -> Option<String> {
    db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .ok()
}

fn set_meta(db: &Connection, key: &str, value: &str) -> rusqlite::Result<usize> {
    db.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )
}

fn load(path: &Path) -> Option<Index> {
    if !path.exists() {
        return None;
    }
    let db = open(path).ok()?;
    (meta(&db, "schema")?.parse::<u32>().ok()? == SCHEMA).then_some(())?;
    let num = |k: &str| meta(&db, k).and_then(|v| v.parse::<i64>().ok());
    let mut stmt = db
        .prepare("SELECT entry FROM checkout ORDER BY path")
        .ok()?;
    let entries = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .ok()?
        .map(|j| serde_json::from_str(&j.ok()?).ok())
        .collect::<Option<Vec<Entry>>>()?;
    Some(Index {
        version: SCHEMA,
        roots: serde_json::from_str(&meta(&db, "roots")?).ok()?,
        scanned_at: num("scanned_at")?,
        scan_ms: num("scan_ms")? as u64,
        walked_at: num("walked_at")?,
        entries,
        peers: Vec::new(),
        hosts: Vec::new(),
        last_dirty: BTreeMap::new(),
    })
}

/// The v1 JSON cache next to the store, if it has the last JSON format.
fn load_json(path: &Path) -> Option<Index> {
    let idx: Index =
        serde_json::from_slice(&fs::read(path.with_file_name("index.json")).ok()?).ok()?;
    (idx.version == JSON_VERSION).then_some(idx)
}

/// Writes only the rows that changed, in one transaction: a reader never sees
/// half a refresh. READMEs are re-read for changed checkouts.
pub fn save(path: &Path, idx: &Index) -> Result<(), String> {
    write(path, idx, false)
}

/// `save`; `all` re-reads every README (a full scan).
fn write(path: &Path, idx: &Index, all: bool) -> Result<(), String> {
    let mut db = open(path)?;
    let err = |e: rusqlite::Error| format!("{}: {e}", path.display());
    let tx = db.transaction().map_err(err)?;
    let old: BTreeMap<String, String> = {
        let mut stmt = tx
            .prepare("SELECT path, entry FROM checkout")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)?
    };
    let mut kept = BTreeSet::new();
    let mut changed = BTreeSet::new();
    for e in &idx.entries {
        let json = serde_json::to_string(e).map_err(|e| e.to_string())?;
        kept.insert(e.path.as_str());
        if all || old.get(&e.path) != Some(&json) {
            changed.insert(e.path.as_str());
            tx.execute(
                "INSERT OR REPLACE INTO checkout (path, remote, touched, entry) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![e.path, e.remote, e.touched, json],
            )
            .map_err(err)?;
        }
    }
    let gone: Vec<&String> = old.keys().filter(|p| !kept.contains(p.as_str())).collect();
    for p in &gone {
        tx.execute("DELETE FROM checkout WHERE path = ?1", [p])
            .map_err(err)?;
    }
    history::record(&tx, &old, &idx.entries, &changed, &gone, now()).map_err(err)?;
    let roots = serde_json::to_string(&idx.roots).map_err(|e| e.to_string())?;
    for (k, v) in [
        ("schema", SCHEMA.to_string()),
        ("roots", roots),
        ("scanned_at", idx.scanned_at.to_string()),
        ("scan_ms", idx.scan_ms.to_string()),
        ("walked_at", idx.walked_at.to_string()),
    ] {
        set_meta(&tx, k, &v).map_err(err)?;
    }
    search::sync(&tx, &idx.entries, &changed).map_err(err)?;
    tx.commit().map_err(err)?;
    peers::write_summary(
        path.parent().ok_or("cache path has no parent")?,
        &idx.entries,
        now(),
    )
}

/// Imports changed peer files and loads the other hosts' rows into `idx`;
/// writes this host's summary if it is missing.
pub fn attach_peers(cache: &Path, idx: &mut Index) -> Result<(), String> {
    let dir = cache.parent().ok_or("cache path has no parent")?;
    peers::write_summary(dir, &idx.entries, now())?;
    let mut db = open(cache)?;
    let err = |e: rusqlite::Error| format!("{}: {e}", cache.display());
    peers::import(&mut db, dir, &vega_herdr::state::post_host()).map_err(err)?;
    (idx.peers, idx.hosts) = peers::load(&db).map_err(err)?;
    idx.last_dirty = history::last_dirty(&db).map_err(err)?;
    Ok(())
}

/// A full scan of `roots`, saved.
pub fn rescan(roots: &[PathBuf], cache: &Path) -> Result<Index, String> {
    let t = Instant::now();
    let entries = scan::scan(roots);
    let idx = Index {
        version: SCHEMA,
        roots: roots.iter().map(|r| r.display().to_string()).collect(),
        scanned_at: now(),
        walked_at: now(),
        scan_ms: t.elapsed().as_millis() as u64,
        entries,
        peers: Vec::new(),
        hosts: Vec::new(),
        last_dirty: BTreeMap::new(),
    };
    write(cache, &idx, true)?;
    Ok(idx)
}

/// The index: from cache with changed entries re-queried, or a full scan when
/// there is no usable cache, the roots changed, or it is older than `MAX_AGE`.
pub fn load_fresh(roots: &[PathBuf], cache: &Path) -> Result<Index, String> {
    let root_strs: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
    let imported = || {
        let idx = load_json(cache).filter(|i| i.roots == root_strs)?;
        save(cache, &idx).ok().map(|()| idx)
    };
    let Some(mut idx) = load(cache)
        .filter(|i| i.roots == root_strs)
        .or_else(imported)
    else {
        eprintln!("janitor: no index for these roots yet; scanning them once (about a minute for ~900 checkouts)");
        let mut idx = rescan(roots, cache)?;
        with_peers(cache, &mut idx);
        return Ok(idx);
    };
    let mut changed = false;
    if now() - idx.walked_at > MAX_AGE {
        rewalk(roots, &mut idx);
        changed = true;
    }
    if refresh(&mut idx.entries) || changed {
        save(cache, &idx)?;
    }
    with_peers(cache, &mut idx);
    Ok(idx)
}

/// `attach_peers`; a failure costs the other hosts' rows, not the command.
fn with_peers(cache: &Path, idx: &mut Index) {
    if let Err(e) = attach_peers(cache, idx) {
        eprintln!("janitor: other hosts left out: {e}");
    }
}

/// Re-walks the roots: keeps known checkouts as they are, queries new ones,
/// drops those no longer found, and takes the fresh list of prunable worktrees.
pub fn rewalk(roots: &[PathBuf], idx: &mut Index) {
    let (found, prunable) = scan::discover(roots);
    let (old_prunable, old): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut idx.entries)
        .into_iter()
        .partition(|e| e.prunable);
    let old_prunable: BTreeSet<String> = old_prunable.into_iter().map(|e| e.path).collect();
    let mut old: BTreeMap<String, Entry> = old.into_iter().map(|e| (e.path.clone(), e)).collect();
    let mut entries = Vec::new();
    let mut new = Vec::new();
    for f in found {
        match old.remove(&f.0.display().to_string()) {
            Some(e) => entries.push(e),
            None => new.push(f),
        }
    }
    let fresh = scan::par_map(new, |(p, k, g)| scan::query(&p, k, &g));
    // Repo info only where the set of checkouts changed: repos with new,
    // vanished or newly prunable worktrees (which branches are checked out).
    let mut commons: BTreeSet<String> = fresh.iter().map(|e| e.common.clone()).collect();
    commons.extend(old.into_values().map(|e| e.common));
    commons.extend(
        prunable
            .iter()
            .filter(|e| !old_prunable.contains(&e.path))
            .map(|e| e.common.clone()),
    );
    entries.extend(fresh);
    entries.extend(prunable);
    scan::apply_repo_info(&mut entries, &commons);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries.dedup_by(|a, b| a.path == b.path);
    idx.entries = entries;
    idx.walked_at = now();
}

/// Drops vanished entries and re-queries those whose fingerprint moved;
/// true if anything changed.
pub fn refresh(entries: &mut Vec<Entry>) -> bool {
    let before = entries.len();
    entries.retain(|e| {
        if e.prunable {
            Path::new(&e.gitdir).is_dir()
        } else {
            Path::new(&e.gitdir).exists() && Path::new(&e.path).is_dir()
        }
    });
    let mut changed = entries.len() != before;
    let stale: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            !e.prunable && scan::fingerprint(Path::new(&e.gitdir), Path::new(&e.common)) != e.fp
        })
        .map(|(i, _)| i)
        .collect();
    if stale.is_empty() {
        return changed;
    }
    changed = true;
    let fresh = scan::par_map(stale.clone(), |i| {
        let e = &entries[i];
        scan::query(Path::new(&e.path), e.kind, Path::new(&e.gitdir))
    });
    let mut commons = BTreeSet::new();
    for (i, e) in stale.into_iter().zip(fresh) {
        commons.insert(e.common.clone());
        entries[i] = e;
    }
    scan::apply_repo_info(entries, &commons);
    changed
}

/// The entries of `kind`.
pub fn count(entries: &[Entry], kind: Kind) -> usize {
    entries
        .iter()
        .filter(|e| e.kind == kind && !e.prunable)
        .count()
}
