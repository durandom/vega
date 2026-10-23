//! The lanista model catalog across hosts (docs/design/lanista-catalog.md):
//! one builder host (ganymede) runs `lanista fetch` daily and puts
//! `model_index.json` into the broker's object store; every other host's
//! vegad copies it into lanista's cache. vegad (plugins/post/src/catalog.rs)
//! moves the bytes; this module holds the names, the state file both vegad
//! and `vega lanista` read, and the rules.

use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The object store bucket on the broker (stream `OBJ_VEGA_LANISTA`).
pub const BUCKET: &str = "VEGA_LANISTA";

/// The one object in it: lanista's index, byte for byte.
pub const OBJECT: &str = "model_index.json";

/// Object metadata keys, readable without downloading the object.
pub const META_GENERATED: &str = "generated_at";
pub const META_SOURCE: &str = "source";

/// Slayer decision vega-nic.12 (2026-10-19): ganymede is always on.
pub const DEFAULT_BUILDER: &str = "ganymede";

/// The builder fetches this often after a fetch that worked ...
pub const FETCH_EVERY: i64 = 24 * 3600;
/// ... and retries this soon after one that failed.
pub const FETCH_RETRY: i64 = 3600;
/// Other hosts look at the bucket this often.
pub const CHECK_EVERY: i64 = 15 * 60;
/// A catalog older than this is stale: the builder is down or failing.
pub const STALE_DAYS: i64 = 2;

/// The host that builds the catalog: `VEGA_LANISTA_BUILDER`, else
/// `$XDG_CONFIG_HOME/vega/lanista-builder` (first non-comment line), else
/// ganymede.
pub fn builder() -> String {
    if let Ok(v) = env::var("VEGA_LANISTA_BUILDER") {
        if !v.trim().is_empty() {
            return crate::state::host_token(v.trim());
        }
    }
    let file = config_home().map(|c| c.join("vega").join("lanista-builder"));
    file.and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| {
            t.lines()
                .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
                .find(|l| !l.is_empty())
        })
        .map(|h| crate::state::host_token(&h))
        .unwrap_or_else(|| DEFAULT_BUILDER.into())
}

/// lanista's index: `$XDG_CACHE_HOME/lanista/model_index.json` (lanista's
/// paths.py).
pub fn index_path() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
        .join("lanista")
        .join("model_index.json")
}

/// `$XDG_STATE_HOME/vega/lanista.json`, written by vegad only.
pub fn state_path() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
        .join("vega")
        .join("lanista.json")
}

fn config_home() -> Option<PathBuf> {
    env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|c| !c.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var("HOME").ok().map(|h| Path::new(&h).join(".config")))
}

fn xdg(var: &str, rel: &str) -> PathBuf {
    env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(&env::var("HOME").unwrap_or_default()).join(rel))
}

/// What vegad last did with the catalog on this host. Times are Unix
/// seconds, 0 for never; `check` and `fetch` are `ok` or the error.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct State {
    /// The host whose fetch made the index vegad last wrote or published.
    pub source: String,
    /// That index's `generated_at`.
    pub generated_at: String,
    /// When vegad wrote it into lanista's cache (other hosts).
    pub received_at: i64,
    /// The last look at the bucket (other hosts).
    pub checked_at: i64,
    pub check: String,
    /// The last `lanista fetch` (builder).
    pub fetched_at: i64,
    pub fetch: String,
    /// The last put into the bucket (builder).
    pub published_at: i64,
}

impl State {
    pub fn load(path: &Path) -> Option<State> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// Atomic: a reader never sees half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }
}

/// `generated_at` of a lanista index, without the models.
pub fn generated_at(index: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Head {
        generated_at: String,
    }
    serde_json::from_slice::<Head>(index)
        .ok()
        .map(|h| h.generated_at)
}

/// Is the bucket's index newer than the local one? lanista writes
/// `generated_at` as UTC ISO 8601 with a fixed format, so the strings order
/// like the times. A newer local index (a hand-run `lanista fetch`) stays.
pub fn newer(remote: &str, local: Option<&str>) -> bool {
    !remote.is_empty() && local.is_none_or(|l| remote > l)
}

/// Is the builder's next `lanista fetch` due?
pub fn fetch_due(st: &State, now: i64) -> bool {
    let wait = if st.fetch == "ok" {
        FETCH_EVERY
    } else {
        FETCH_RETRY
    };
    st.fetched_at == 0 || now - st.fetched_at >= wait
}

/// Where the local index came from: the state's source while the index is
/// the one vegad wrote or published, else this host's own `lanista fetch`.
pub fn source(st: Option<&State>, local_generated: Option<&str>, host: &str) -> String {
    match (st, local_generated) {
        (Some(s), Some(g)) if !s.source.is_empty() && s.generated_at == g => s.source.clone(),
        _ => host.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_compares_generated_at() {
        let a = "2026-10-11T11:14:04.481208+00:00";
        let b = "2026-10-12T02:00:00.000001+00:00";
        assert!(newer(b, Some(a)));
        assert!(!newer(a, Some(b)));
        assert!(!newer(a, Some(a)));
        assert!(newer(a, None));
        assert!(!newer("", None));
    }

    #[test]
    fn fetch_is_daily_and_retries_hourly() {
        let mut st = State::default();
        assert!(fetch_due(&st, 100));
        st.fetched_at = 1000;
        st.fetch = "ok".into();
        assert!(!fetch_due(&st, 1000 + FETCH_EVERY - 1));
        assert!(fetch_due(&st, 1000 + FETCH_EVERY));
        st.fetch = "error: exit 1".into();
        assert!(!fetch_due(&st, 1000 + FETCH_RETRY - 1));
        assert!(fetch_due(&st, 1000 + FETCH_RETRY));
    }

    #[test]
    fn source_is_the_state_while_the_index_is_vegads() {
        let st = State {
            source: "ganymede".into(),
            generated_at: "g1".into(),
            ..Default::default()
        };
        assert_eq!(source(Some(&st), Some("g1"), "proteus"), "ganymede");
        assert_eq!(source(Some(&st), Some("g2"), "proteus"), "proteus");
        assert_eq!(source(None, Some("g1"), "proteus"), "proteus");
    }

    #[test]
    fn generated_at_reads_the_head_only() {
        let idx = br#"{"generated_at": "2026-10-11T11:14:04+00:00", "models": {"a": {}}}"#;
        assert_eq!(
            generated_at(idx).as_deref(),
            Some("2026-10-11T11:14:04+00:00")
        );
        assert_eq!(generated_at(b"{}"), None);
        assert_eq!(generated_at(b"not json"), None);
    }

    #[test]
    fn state_round_trips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("vega/lanista.json");
        assert_eq!(State::load(&p), None);
        let st = State {
            source: "ganymede".into(),
            check: "ok".into(),
            checked_at: 7,
            ..Default::default()
        };
        st.save(&p).unwrap();
        assert_eq!(State::load(&p), Some(st));
        assert!(!p.with_extension("json.tmp").exists());
    }
}
