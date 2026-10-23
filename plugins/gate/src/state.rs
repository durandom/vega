//! One state file per reviewer pane: `$XDG_STATE_HOME/vega/gate/<pane>.json`
//! (gate.md section 4, step 6). It ties a reviewer pane to its review: what
//! `verdict` closes, what the next push finds running, what `gc` removes.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::state_dir;

#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub pane: String,
    /// The author's checkout, where bd and git run.
    pub repo: PathBuf,
    pub branch: String,
    pub target: String,
    pub head: String,
    pub base: String,
    pub keys: Vec<String>,
    pub beads: Vec<String>,
    pub review: String,
    pub author_pane: String,
    pub clone: PathBuf,
    /// Rubric export and prompt.
    pub work: PathBuf,
    pub kind: String,
    pub model: String,
    pub tab: String,
    pub started_at: i64,
    pub timeout_secs: i64,
}

pub fn file(pane: &str) -> PathBuf {
    state_dir().join(format!("{}.json", vega_shim::safe(pane)))
}

impl Review {
    pub fn to_json(&self) -> Value {
        json!({
            "pane": self.pane,
            "repo": self.repo,
            "branch": self.branch,
            "target": self.target,
            "head": self.head,
            "base": self.base,
            "keys": self.keys,
            "beads": self.beads,
            "review": self.review,
            "author_pane": self.author_pane,
            "clone": self.clone,
            "work": self.work,
            "kind": self.kind,
            "model": self.model,
            "tab": self.tab,
            "started_at": self.started_at,
            "timeout_secs": self.timeout_secs,
        })
    }

    pub fn from_json(v: &Value) -> Option<Review> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        let list = |k: &str| -> Vec<String> {
            v.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        Some(Review {
            pane: s("pane")?,
            repo: s("repo")?.into(),
            branch: s("branch").unwrap_or_default(),
            target: s("target").unwrap_or_default(),
            head: s("head").unwrap_or_default(),
            base: s("base").unwrap_or_default(),
            keys: list("keys"),
            beads: list("beads"),
            review: s("review").unwrap_or_default(),
            author_pane: s("author_pane").unwrap_or_default(),
            clone: s("clone")?.into(),
            work: s("work").unwrap_or_default().into(),
            kind: s("kind").unwrap_or_default(),
            model: s("model").unwrap_or_default(),
            tab: s("tab").unwrap_or_default(),
            started_at: v.get("started_at").and_then(Value::as_i64).unwrap_or(0),
            timeout_secs: v
                .get("timeout_secs")
                .and_then(Value::as_i64)
                .unwrap_or(1200),
        })
    }

    pub fn save(&self) -> Result<(), String> {
        let f = file(&self.pane);
        if let Some(d) = f.parent() {
            fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let tmp = f.with_extension("json.tmp");
        fs::write(&tmp, self.to_json().to_string())
            .and_then(|_| fs::rename(&tmp, &f))
            .map_err(|e| format!("{}: {e}", f.display()))
    }
}

/// Serialize verdict recording and revocation for one immutable generation.
/// Keep the lock file: unlinking it would allow callers to lock different inodes.
pub(crate) fn lock(pane: &str) -> Result<fs::File, String> {
    let path = file(pane).with_extension("lock");
    fs::create_dir_all(state_dir()).map_err(|e| format!("review lock: {e}"))?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("review lock: {e}"))?;
    lock.lock().map_err(|e| format!("review lock: {e}"))?;
    Ok(lock)
}

/// Lock only the snapshot still active. `None` means a verdict or another
/// start won; callers must refresh their lookup. Keep the state until all
/// superseded bead writes succeed, so a storage error leaves a retryable review.
pub(crate) fn lock_current(r: &Review) -> Result<Option<fs::File>, String> {
    let guard = lock(&r.pane)?;
    if load(&r.pane).as_ref() != Some(r) {
        return Ok(None);
    }
    Ok(Some(guard))
}

pub fn load(pane: &str) -> Option<Review> {
    load_file(&file(pane))
}

fn load_file(f: &Path) -> Option<Review> {
    let v: Value = serde_json::from_str(&fs::read_to_string(f).ok()?).ok()?;
    Review::from_json(&v)
}

/// Every review with a state file on this host.
pub fn all() -> Vec<Review> {
    let Ok(entries) = fs::read_dir(state_dir()) else {
        return Vec::new();
    };
    let mut out: Vec<Review> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| load_file(&e.path()))
        .collect();
    out.sort_by_key(|r| r.started_at);
    out
}

pub fn remove(pane: &str) {
    let _ = fs::remove_file(file(pane));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let r = Review {
            pane: "w1:p2".into(),
            repo: "/r".into(),
            branch: "b".into(),
            target: "origin/main".into(),
            head: "h".into(),
            base: "a".into(),
            keys: vec!["k1".into()],
            beads: vec!["g-1".into()],
            review: "rv".into(),
            author_pane: "w1:p1".into(),
            clone: "/c".into(),
            work: "/w".into(),
            kind: "codex".into(),
            model: "m".into(),
            tab: "gate-b".into(),
            started_at: 5,
            timeout_secs: 60,
        };
        assert_eq!(Review::from_json(&r.to_json()), Some(r));
        assert!(file("w1:p2").ends_with("vega/gate/w1_p2.json"));
    }
}
