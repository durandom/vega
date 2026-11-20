//! Owner records (design, decision 3): which pane called agent-browser with
//! which session name. One JSON map, `$XDG_STATE_HOME/vega/browser/
//! sessions.json` (default `~/.local/state/vega/browser/sessions.json`):
//! `{session: {pane, host, project, cwd, first_seen, last_seen}}`.
//!
//! Everything here is best effort: a call whose record cannot be written
//! still passes through.

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Map, Value};

/// One session's owner: the agent that last named it.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The calling agent's pane (`w1:p2`, `t7`), or None outside a pane.
    pub pane: Option<String>,
    pub host: String,
    /// The worktree directory name (or the cwd's, outside a repo).
    pub project: Option<String>,
    pub cwd: String,
    pub first_seen: i64,
    pub last_seen: i64,
}

impl Record {
    fn to_json(&self) -> Value {
        serde_json::json!({
            "pane": self.pane,
            "host": self.host,
            "project": self.project,
            "cwd": self.cwd,
            "first_seen": self.first_seen,
            "last_seen": self.last_seen,
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            pane: v["pane"].as_str().map(str::to_owned),
            host: v["host"].as_str()?.to_owned(),
            project: v["project"].as_str().map(str::to_owned),
            cwd: v["cwd"].as_str().unwrap_or_default().to_owned(),
            first_seen: v["first_seen"].as_i64()?,
            last_seen: v["last_seen"].as_i64()?,
        })
    }
}

/// The records file; None without a home to put it under.
pub fn file() -> Option<PathBuf> {
    Some(
        env::var_os("XDG_STATE_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))?
            .join("vega")
            .join("browser")
            .join("sessions.json"),
    )
}

/// Inventory accepts legacy rows and skips malformed entries.
pub fn load() -> BTreeMap<String, Record> {
    read_map()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(session, v)| Record::from_json(&v).map(|r| (session, r)))
        .collect()
}

fn read_map() -> io::Result<Map<String, Value>> {
    let file = file().ok_or_else(|| io::Error::other("owner file path unavailable"))?;
    match fs::read(file) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e),
    }
}

pub fn get(session: &str) -> io::Result<Option<Record>> {
    read_map()?
        .get(session)
        .map(|v| {
            if v.get("pane")
                .is_some_and(|p| !p.is_null() && !p.is_string())
            {
                return Err(io::Error::other("invalid owner pane"));
            }
            Record::from_json(v).ok_or_else(|| io::Error::other("invalid owner record"))
        })
        .transpose()
}

/// Serialize read/check/write across shim calls and cleanup commands.
pub struct Locked {
    _guard: File,
    file: PathBuf,
}

impl Locked {
    pub fn acquire() -> io::Result<Self> {
        let file = file().ok_or_else(|| io::Error::other("owner file path unavailable"))?;
        let parent = file
            .parent()
            .ok_or_else(|| io::Error::other("owner directory unavailable"))?;
        fs::create_dir_all(parent)?;
        let guard = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(file.with_extension("lock"))?;
        vega_herdr::platform::lock(&guard)?;
        Ok(Self {
            _guard: guard,
            file,
        })
    }

    fn save(&self, all: &Map<String, Value>) -> io::Result<()> {
        let mut tmp =
            tempfile::NamedTempFile::new_in(self.file.parent().expect("owner directory"))?;
        serde_json::to_writer(tmp.as_file_mut(), all)?;
        tmp.persist(&self.file)?;
        Ok(())
    }

    pub fn get(&self, session: &str) -> io::Result<Option<Record>> {
        get(session)
    }

    pub fn remove(&self, session: &str) -> io::Result<()> {
        let mut all = read_map()?;
        if all.remove(session).is_some() {
            self.save(&all)?;
        }
        Ok(())
    }
}

/// Upserts the owner of `session`; `first_seen` of an existing record stays.
pub fn upsert(session: &str, mut record: Record) {
    let Ok(lock) = Locked::acquire() else { return };
    let Ok(mut all) = read_map() else { return };
    record.first_seen = all
        .get(session)
        .and_then(Record::from_json)
        .map_or(record.first_seen, |r| r.first_seen);
    all.insert(session.to_owned(), record.to_json());
    let _ = lock.save(&all);
}

/// Drops the record of `session`.
pub fn remove(session: &str) {
    if let Ok(lock) = Locked::acquire() {
        let _ = lock.remove(session);
    }
}

/// The owner of a call made from this process: its pane (a real pane id only,
/// as `vega mux` resolves it, else None), host, project and cwd.
pub fn record_from_env(now: i64) -> Record {
    let key = vega_shim::pane_key();
    let cwd = env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    Record {
        pane: vega_shim::is_pane(&key).then_some(key),
        host: vega_herdr::local_host(),
        project: project_of(&cwd),
        cwd,
        first_seen: now,
        last_seen: now,
    }
}

/// The worktree's directory name, else the cwd's; None without either.
fn project_of(cwd: &str) -> Option<String> {
    let top = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty());
    let dir = top.unwrap_or_else(|| cwd.to_owned());
    Path::new(&dir)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(pane: Option<&str>, first: i64, last: i64) -> Record {
        Record {
            pane: pane.map(str::to_owned),
            host: "proteus".into(),
            project: Some("vega".into()),
            cwd: "/Users/x/src/vega".into(),
            first_seen: first,
            last_seen: last,
        }
    }

    #[test]
    fn upsert_keeps_first_seen_and_remove_drops() {
        let tmp = tempfile::tempdir().unwrap();
        env::set_var("XDG_STATE_HOME", tmp.path().join("state"));
        upsert("s1", record(Some("w1:p2"), 100, 100));
        upsert("s1", record(Some("w1:p9"), 100, 200));
        upsert("s2", record(None, 150, 150));
        let all = load();
        assert_eq!(all["s1"].pane.as_deref(), Some("w1:p9"));
        assert_eq!(all["s1"].first_seen, 100);
        assert_eq!(all["s1"].last_seen, 200);
        assert_eq!(all["s2"].pane, None);
        remove("s1");
        assert!(!load().contains_key("s1"));
        env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn round_trips_through_the_documented_shape() {
        let r = record(Some("t7"), 5, 6);
        let v = r.to_json();
        assert_eq!(v["pane"], "t7");
        assert_eq!(v["host"], "proteus");
        assert_eq!(v["first_seen"], 5);
        assert_eq!(Record::from_json(&v), Some(r.clone()));
        let mut null_pane = r;
        null_pane.pane = None;
        assert_eq!(null_pane.to_json()["pane"], Value::Null);
    }
}
