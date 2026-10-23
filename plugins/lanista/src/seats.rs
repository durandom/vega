//! The seats allowlist: which models a host can start, per provider lane.
//!
//! vega owns the file (`plugins/lanista/seats.json`, embedded here) and
//! writes it to lanista's config dir. Each pin may carry a `tier`
//! (flagship, balanced, budget) that `vega lanista pin` reads. lanista
//! ignores the key; its `profiles` cannot stand in for it, because the
//! newest models have no benchmark score yet and drop out of every profile.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The repo copy, shipped in the binary so `vega deploy` carries it.
pub const SEATS: &str = include_str!("../seats.json");

pub const DEFAULT_SEAT: &str = "herdr-agents";
pub const TIERS: [&str; 3] = ["flagship", "balanced", "budget"];

#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub provider: String,
    pub model: String,
    pub engine: String,
    pub role: String,
    pub tier: String,
}

/// lanista's seats file: `$XDG_CONFIG_HOME/lanista/seats.json`.
pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("lanista").join("seats.json")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The pins of `seat` in a seats blob, in file order.
pub fn pins(blob: &str, seat: &str) -> Result<Vec<Pin>, String> {
    let v: Value = serde_json::from_str(blob).map_err(|e| format!("not JSON: {e}"))?;
    let models = v["seats"][seat]["models"]
        .as_array()
        .ok_or_else(|| format!("no seat `{seat}`"))?;
    let s = |e: &Value, k: &str| e[k].as_str().unwrap_or("").to_string();
    Ok(models
        .iter()
        .filter_map(|e| {
            let model = e["pin"].as_str().or(e["id"].as_str())?.to_string();
            Some(Pin {
                provider: s(e, "provider"),
                model,
                engine: s(e, "engine"),
                role: s(e, "role"),
                tier: s(e, "tier"),
            })
        })
        .collect())
}

/// Seat ids in a seats blob.
pub fn seat_ids(blob: &str) -> Vec<String> {
    serde_json::from_str::<Value>(blob)
        .ok()
        .and_then(|v| v["seats"].as_object().map(|o| o.keys().cloned().collect()))
        .unwrap_or_default()
}

/// Provider lanes in file order, deduplicated.
pub fn providers(pins: &[Pin]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in pins {
        if !out.contains(&p.provider) {
            out.push(p.provider.clone());
        }
    }
    out
}

/// The first pin of `provider` with `tier`.
pub fn pick<'a>(pins: &'a [Pin], provider: &str, tier: &str) -> Option<&'a Pin> {
    pins.iter()
        .find(|p| p.provider == provider && p.tier == tier)
}

/// How the installed file relates to the repo copy.
#[derive(Debug, PartialEq)]
pub enum State {
    InSync,
    Differs,
    Missing,
}

impl State {
    pub fn label(&self) -> &'static str {
        match self {
            State::InSync => "in-sync",
            State::Differs => "differs",
            State::Missing => "missing",
        }
    }
}

pub fn state(path: &Path) -> State {
    match fs::read_to_string(path) {
        Ok(s) if s == SEATS => State::InSync,
        Ok(_) => State::Differs,
        Err(_) => State::Missing,
    }
}

/// Writes the repo copy to `path`. A differing file is backed up once to
/// `<path>.vega-bak` first. Returns what happened: `current` or `written`.
pub fn sync(path: &Path) -> Result<&'static str, String> {
    match state(path) {
        State::InSync => return Ok("current"),
        State::Differs => {
            let bak = path.with_extension("json.vega-bak");
            if !bak.exists() {
                fs::copy(path, &bak).map_err(|e| format!("backup {}: {e}", bak.display()))?;
            }
        }
        State::Missing => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
        }
    }
    fs::write(path, SEATS).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok("written")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> Vec<Pin> {
        pins(SEATS, DEFAULT_SEAT).unwrap()
    }

    #[test]
    fn repo_copy_has_no_copilot_and_tiers_every_lane() {
        let pins = repo();
        assert_eq!(providers(&pins), ["xai", "zai", "agy", "claude", "codex"]);
        for prov in providers(&pins) {
            for tier in TIERS {
                assert!(pick(&pins, &prov, tier).is_some(), "{prov} lacks {tier}");
            }
        }
        assert!(pins
            .iter()
            .all(|p| p.tier.is_empty() || TIERS.contains(&p.tier.as_str())));
    }

    #[test]
    fn pins_pi_xai_and_zai_by_tier() {
        let pins = repo();
        let m = |p, t| pick(&pins, p, t).unwrap().model.clone();
        assert_eq!(m("xai", "flagship"), "grok-4.6");
        assert_eq!(m("xai", "balanced"), "grok-4.5");
        assert_eq!(m("xai", "budget"), "grok-4.3");
        assert_eq!(m("zai", "flagship"), "glm-5.3");
        assert_eq!(m("zai", "balanced"), "glm-5.2");
        assert_eq!(m("zai", "budget"), "glm-4.7");
        assert!(pick(&pins, "github-copilot", "balanced").is_none());
    }

    #[test]
    fn pins_native_claude_and_codex_lanes_by_tier() {
        // Ids checked against the installed CLIs (claude --model, codex -m).
        let pins = repo();
        let p = |l, t| pick(&pins, l, t).unwrap();
        assert_eq!(p("claude", "flagship").model, "claude-opus-5-5");
        assert_eq!(p("claude", "balanced").model, "claude-sonnet-5-5");
        assert_eq!(p("claude", "budget").model, "claude-haiku-5-5");
        assert_eq!(p("codex", "flagship").model, "gpt-6-astra");
        assert_eq!(p("codex", "balanced").model, "gpt-6.1-sol");
        assert_eq!(p("codex", "budget").model, "gpt-6-luna");
        assert_eq!(p("claude", "budget").engine, "claude");
        assert_eq!(p("codex", "budget").engine, "codex");
    }

    #[test]
    fn reads_a_fixture_with_strings_and_ids() {
        let blob = r#"{"seats":{"s":{"models":[
            "bare",
            {"provider":"xai","id":"grok-x","tier":"budget"},
            {"provider":"xai"}
        ]}}}"#;
        let p = pins(blob, "s").unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(pick(&p, "xai", "budget").unwrap().model, "grok-x");
        assert!(pins(blob, "nope").is_err());
        assert_eq!(seat_ids(blob), ["s"]);
    }

    #[test]
    fn sync_writes_backs_up_once_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lanista/seats.json");
        assert_eq!(state(&path), State::Missing);
        assert_eq!(sync(&path).unwrap(), "written");
        assert_eq!(sync(&path).unwrap(), "current");
        fs::write(&path, "{\"local\": 1}").unwrap();
        assert_eq!(state(&path), State::Differs);
        assert_eq!(sync(&path).unwrap(), "written");
        let bak = path.with_extension("json.vega-bak");
        assert_eq!(fs::read_to_string(&bak).unwrap(), "{\"local\": 1}");
        fs::write(&path, "{\"other\": 2}").unwrap();
        sync(&path).unwrap();
        assert_eq!(fs::read_to_string(&bak).unwrap(), "{\"local\": 1}");
        assert_eq!(state(&path), State::InSync);
    }
}
