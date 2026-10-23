//! Carries vega janitor's checkout summaries between hosts
//! (docs/design/janitor.md, Cross-host find/list). janitor stays offline:
//! this host's `summary.json` goes into the registry as `<host>.janitor`,
//! the other hosts' come back as `peers/<host>.json` for janitor to import.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

/// The other hosts' summaries are fetched this often (~70 KB each).
pub const PULL_EVERY: Duration = Duration::from_secs(300);
/// A larger summary is not published (NATS `max_payload` is 1 MiB).
const MAX_BYTES: u64 = 512 * 1024;

/// `$XDG_STATE_HOME/vega/janitor` (default `~/.local/state`).
fn dir() -> Option<PathBuf> {
    let state = env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("HOME").ok().map(|h| format!("{h}/.local/state")))?;
    Some(Path::new(&state).join("vega/janitor"))
}

/// This host's summary as a registry entry; none before janitor ran, or
/// when it is too large (logged).
pub fn registry_entry(host: &str) -> Option<(String, Value)> {
    entry_from(&dir()?, host)
}

fn entry_from(dir: &Path, host: &str) -> Option<(String, Value)> {
    let path = dir.join("summary.json");
    let len = fs::metadata(&path).ok()?.len();
    if len > MAX_BYTES {
        crate::log(&format!(
            "janitor: summary.json is {len} bytes, over {MAX_BYTES}; not published"
        ));
        return None;
    }
    let v: Value = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    Some((vega_registry::janitor_key(host), v))
}

/// Fetches the other hosts' summaries the view lists and writes them to
/// `peers/`; the number of files written or removed.
pub async fn pull(
    kv: &async_nats::jetstream::kv::Store,
    view: &vega_registry::View,
    host: &str,
) -> Result<usize, String> {
    let own = vega_registry::token(host);
    let mut got = Vec::new();
    for key in view.keys.iter().filter(|k| vega_registry::is_janitor(k)) {
        let Some(h) = key.split('.').next().filter(|h| *h != own) else {
            continue;
        };
        // A deleted key keeps its subject: no value, the host is gone.
        let v = kv
            .get(key)
            .await
            .map_err(|e| format!("{key}: {e}"))?
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        got.push((h.to_string(), v));
    }
    let dir = dir().ok_or("no state dir")?.join("peers");
    sync(&dir, &got)
}

/// Writes each host's summary whose `at` moved (atomic, so janitor imports
/// only changes) and removes the files of hosts without one.
fn sync(dir: &Path, got: &[(String, Option<Value>)]) -> Result<usize, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut n = 0;
    for (h, v) in got {
        let path = dir.join(format!("{h}.json"));
        let Some(v) = v else {
            continue;
        };
        let old: Option<Value> = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        if old.is_some_and(|o| o["at"] == v["at"]) {
            continue;
        }
        let tmp = dir.join(format!(".{h}.{}.tmp", std::process::id()));
        fs::write(&tmp, v.to_string()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        n += 1;
    }
    let keep = |name: &str| {
        got.iter()
            .any(|(h, v)| v.is_some() && name == format!("{h}.json"))
    };
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") && !keep(&name) {
            fs::remove_file(e.path()).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn publishes_the_summary_under_the_host_key() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(entry_from(tmp.path(), "proteus"), None, "janitor never ran");
        fs::write(tmp.path().join("summary.json"), r#"{"at":5,"rows":[]}"#).unwrap();
        let (k, v) = entry_from(tmp.path(), "proteus").unwrap();
        assert_eq!(k, "proteus.janitor");
        assert_eq!(v["at"], 5);
    }

    #[test]
    fn writes_changed_peers_and_drops_gone_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("peers");
        let g = |at: i64| Some(json!({"at": at, "rows": []}));
        let got = vec![("ganymede".to_string(), g(1)), ("io".to_string(), g(2))];
        assert_eq!(sync(&dir, &got).unwrap(), 2);
        assert_eq!(sync(&dir, &got).unwrap(), 0, "same at: no write");
        let got = vec![("ganymede".to_string(), g(3)), ("io".to_string(), None)];
        assert_eq!(sync(&dir, &got).unwrap(), 2, "ganymede moved, io gone");
        assert!(dir.join("ganymede.json").is_file());
        assert!(!dir.join("io.json").exists());
    }
}
