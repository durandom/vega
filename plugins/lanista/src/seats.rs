//! The seats allowlist: which models a host can start, per provider lane.
//!
//! vega owns the file (`plugins/lanista/seats.json`, embedded here) and
//! writes it to lanista's config dir. Each pin may carry a `tier`
//! (flagship, balanced, budget) that `vega lanista pin` reads. lanista
//! ignores the key; its `profiles` cannot stand in for it, because the
//! newest models have no benchmark score yet and drop out of every profile.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// The repo copy, shipped in the binary so `vega deploy` carries it.
pub const SEATS: &str = include_str!("../seats.json");

pub const DEFAULT_SEAT: &str = "herdr-agents";
pub const TIERS: [&str; 3] = ["flagship", "balanced", "budget"];
pub const COSTS: [&str; 3] = ["subscription", "budget", "unknown"];

#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub provider: String,
    pub model: String,
    pub engine: String,
    pub role: String,
    pub tier: String,
    /// The catalog's canonical name, when the pin id is gateway-specific.
    pub canonical: Option<String>,
    /// How using this pin is paid for; `unknown` unless the overlay says.
    pub cost: String,
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

/// lanista's seats file: `$XDG_CONFIG_HOME/lanista/seats.json`.
pub fn path() -> PathBuf {
    config_home().join("lanista").join("seats.json")
}

/// The host-local seats overlay: `$XDG_CONFIG_HOME/vega/lanista/seats.local.json`.
/// A host pins its own tiers (e.g. an EnMaaS gateway id) without repo edits;
/// [`sync`] never touches this file.
pub fn overlay_path() -> PathBuf {
    config_home()
        .join("vega")
        .join("lanista")
        .join("seats.local.json")
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
            let cost = s(e, "cost");
            let cost = if cost.is_empty() {
                "unknown".to_string()
            } else {
                cost
            };
            Some(Pin {
                provider: s(e, "provider"),
                model,
                engine: s(e, "engine"),
                role: s(e, "role"),
                tier: s(e, "tier"),
                canonical: e["canonical"].as_str().map(str::to_string),
                cost,
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

/// Pi has no Google global tiers: retain the historical zai fallback. An
/// overlay pi pin with a tier (e.g. an EnMaaS gateway) wins over it.
pub fn global<'a>(pins: &'a [Pin], lane: &str, tier: &str) -> Option<&'a Pin> {
    if lane == "pi" {
        return pick(pins, "pi", tier).or_else(|| pick(pins, "zai", tier));
    }
    pick(pins, lane, tier)
}

/// One overlay entry of `seats.local.json`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayEntry {
    provider: String,
    pin: String,
    tier: String,
    canonical: Option<String>,
    cost: String,
}

impl OverlayEntry {
    /// A validated pin, or why the entry cannot be used. The engine is the
    /// repo lane's, so spawned pins keep their provider's engine.
    fn to_pin(&self, repo: &[Pin]) -> Result<Pin, String> {
        if !repo.iter().any(|p| p.provider == self.provider) {
            return Err(format!(
                "unknown lane `{}` (lanes: {})",
                self.provider,
                providers(repo).join(" ")
            ));
        }
        if !TIERS.contains(&self.tier.as_str()) {
            return Err(format!("tier must be one of {}", TIERS.join("|")));
        }
        if !COSTS.contains(&self.cost.as_str()) {
            return Err(format!("cost must be one of {}", COSTS.join("|")));
        }
        if self.pin.is_empty() || self.pin.chars().any(char::is_control) {
            return Err("pin must be nonempty and contain no control characters".into());
        }
        if self.canonical.as_ref().is_some_and(|c| c.is_empty()) {
            return Err("canonical must be nonempty when present".into());
        }
        Ok(Pin {
            provider: self.provider.clone(),
            model: self.pin.clone(),
            engine: repo
                .iter()
                .find(|p| p.provider == self.provider)
                .map(|p| p.engine.clone())
                .unwrap_or_default(),
            role: String::new(),
            tier: self.tier.clone(),
            canonical: self.canonical.clone(),
            cost: self.cost.clone(),
        })
    }
}

/// The parsed overlay: extra pins per seat, plus one report per invalid
/// entry (invalid entries are skipped, never silently used).
#[derive(Debug, Default)]
pub struct Overlay {
    pub seats: BTreeMap<String, Vec<Pin>>,
    pub reports: Vec<String>,
}

/// The overlay parsed from its JSON text. Err: the file itself is unusable
/// and none of it applies.
pub fn overlay_pins(blob: &str) -> Result<Overlay, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct File {
        schema: u8,
        seats: BTreeMap<String, Vec<Value>>,
    }
    let file: File = serde_json::from_str(blob).map_err(|e| format!("not an overlay: {e}"))?;
    if file.schema != 1 {
        return Err(format!("unknown overlay schema {}", file.schema));
    }
    let mut seats: BTreeMap<String, Vec<Pin>> = BTreeMap::new();
    let mut reports = Vec::new();
    for (seat, entries) in &file.seats {
        let repo = match pins(SEATS, seat) {
            Ok(repo) => repo,
            Err(_) => {
                reports.push(format!(
                    "seat `{seat}` is not in the repo seats ({}), entries skipped",
                    seat_ids(SEATS).join(" ")
                ));
                continue;
            }
        };
        for (i, value) in entries.iter().enumerate() {
            match serde_json::from_value::<OverlayEntry>(value.clone())
                .map_err(|e| e.to_string())
                .and_then(|e| e.to_pin(&repo))
            {
                Ok(pin) => seats.entry(seat.clone()).or_default().push(pin),
                Err(e) => reports.push(format!("{seat} entry {i}: {e}")),
            }
        }
    }
    Ok(Overlay { seats, reports })
}

fn report_overlay(path: &Path, reports: &[String]) {
    for r in reports {
        eprintln!("vega lanista: seats overlay {}: {r}", path.display());
    }
}

/// Repo pins for `seat` merged with the overlay file at `path`: overlay pins
/// come first, so an overlay pin wins for its (provider, tier). An absent
/// overlay is the normal case; a broken overlay is reported on stderr and
/// ignored, never silently used. Err only for an unknown seat, as [`pins`].
pub fn effective_at(path: &Path, seat: &str) -> Result<Vec<Pin>, String> {
    let mut pins = pins(SEATS, seat)?;
    match fs::read_to_string(path) {
        Ok(blob) => match overlay_pins(&blob) {
            Ok(overlay) => {
                report_overlay(path, &overlay.reports);
                if let Some(extra) = overlay.seats.get(seat) {
                    let mut merged = extra.clone();
                    merged.append(&mut pins);
                    pins = merged;
                }
            }
            Err(e) => eprintln!(
                "vega lanista: seats overlay {}: {e}; ignoring the overlay",
                path.display()
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!(
            "vega lanista: seats overlay {}: {e}; ignoring the overlay",
            path.display()
        ),
    }
    Ok(pins)
}

/// [`effective_at`] against the host-local [`overlay_path`].
pub fn effective(seat: &str) -> Result<Vec<Pin>, String> {
    effective_at(&overlay_path(), seat)
}

/// Per repo seat (file order), the effective pins of [`effective_at`].
pub fn effective_seats_at(path: &Path) -> Vec<(String, Vec<Pin>)> {
    seat_ids(SEATS)
        .into_iter()
        .map(|seat| {
            let pins = effective_at(path, &seat).unwrap_or_default();
            (seat, pins)
        })
        .collect()
}

/// [`effective_seats_at`] against the host-local [`overlay_path`].
pub fn effective_seats() -> Vec<(String, Vec<Pin>)> {
    effective_seats_at(&overlay_path())
}

/// One line for `vega lanista doctor`: the overlay's state at `path`.
pub fn overlay_summary(path: &Path) -> String {
    let blob = match fs::read_to_string(path) {
        Ok(blob) => blob,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return "none".into(),
        Err(e) => return format!("unreadable ({e})"),
    };
    match overlay_pins(&blob) {
        Ok(overlay) => {
            let pins: usize = overlay.seats.values().map(Vec::len).sum();
            if overlay.reports.is_empty() {
                format!("{pins} pin(s)")
            } else {
                format!(
                    "{} pin(s), {} invalid entr{} skipped",
                    pins,
                    overlay.reports.len(),
                    if overlay.reports.len() == 1 {
                        "y"
                    } else {
                        "ies"
                    }
                )
            }
        }
        Err(e) => format!("invalid ({e})"),
    }
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

/// Writes the repo copy to `path` (never the host-local overlay,
/// [`overlay_path`]). A differing file is backed up once to
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
        assert_eq!(providers(&pins), ["pi", "zai", "agy", "claude", "codex"]);
        for prov in providers(&pins) {
            for tier in TIERS {
                assert!(global(&pins, &prov, tier).is_some(), "{prov} lacks {tier}");
            }
        }
        assert!(pins
            .iter()
            .all(|p| p.tier.is_empty() || TIERS.contains(&p.tier.as_str())));
    }

    #[test]
    fn pins_pi_zai_by_tier() {
        let pins = repo();
        let m = |p, t| pick(&pins, p, t).unwrap().model.clone();
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

    #[test]
    fn overlay_parses_validates_and_reports_invalid_entries() {
        let blob = r#"{"schema":1,"seats":{
            "herdr-agents":[
                {"provider":"pi","pin":"enmaas/gome-3.1","tier":"flagship","canonical":"gome-3.1","cost":"subscription"},
                {"provider":"enmaas","pin":"enmaas/gome-3.1","tier":"flagship","cost":"unknown"},
                {"provider":"pi","pin":"enmaas/x","tier":"thrifty","cost":"unknown"},
                {"provider":"pi","pin":"enmaas/x","tier":"budget","cost":"cheap"},
                {"provider":"pi","pin":"","tier":"budget","cost":"unknown"},
                {"provider":"pi","pin":"enmaas/x","tier":"budget","cost":"unknown","extra":1},
                {"provider":"pi","pin":"enmaas/y","tier":"budget","cost":"budget"}
            ],
            "no-such-seat":[
                {"provider":"pi","pin":"enmaas/z","tier":"budget","cost":"unknown"}
            ]
        }}"#;
        let (seats, reports) = {
            let overlay = overlay_pins(blob).unwrap();
            (overlay.seats, overlay.reports)
        };
        let pins = &seats[DEFAULT_SEAT];
        assert_eq!(pins.len(), 2);
        let p = &pins[0];
        assert_eq!(
            (p.provider.as_str(), p.model.as_str(), p.tier.as_str()),
            ("pi", "enmaas/gome-3.1", "flagship")
        );
        assert_eq!(p.engine, "pi");
        assert_eq!(p.canonical.as_deref(), Some("gome-3.1"));
        assert_eq!(p.cost, "subscription");
        assert_eq!(pins[1].cost, "budget");
        let reports = reports.join("\n");
        for fragment in [
            "unknown lane `enmaas`",
            "tier must be one of flagship|balanced|budget",
            "cost must be one of subscription|budget|unknown",
            "pin must be nonempty",
            "unknown field `extra`",
            "seat `no-such-seat` is not in the repo seats",
        ] {
            assert!(
                reports.contains(fragment),
                "missing {fragment} in:\n{reports}"
            );
        }
        assert!(overlay_pins("[]").is_err());
        assert!(overlay_pins("{\"schema\":2,\"seats\":{}}").is_err());
    }

    #[test]
    fn effective_merges_overlay_before_repo_pins() {
        let dir = tempfile::tempdir().unwrap();
        let overlay = dir.path().join("seats.local.json");
        // Absent overlay: exactly the repo pins.
        assert_eq!(
            effective_at(&overlay, DEFAULT_SEAT).unwrap(),
            pins(SEATS, DEFAULT_SEAT).unwrap()
        );
        fs::write(
            &overlay,
            r#"{"schema":1,"seats":{"herdr-agents":[
                {"provider":"pi","pin":"enmaas/gome-3.1","tier":"flagship","cost":"subscription"}
            ]}}"#,
        )
        .unwrap();
        let merged = effective_at(&overlay, DEFAULT_SEAT).unwrap();
        assert_eq!(
            pick(&merged, "pi", "flagship").unwrap().model,
            "enmaas/gome-3.1"
        );
        assert_eq!(
            global(&merged, "pi", "flagship").unwrap().model,
            "enmaas/gome-3.1"
        );
        // Repo pins stay reachable below overlay pins.
        assert_eq!(global(&merged, "zai", "budget").unwrap().model, "glm-4.7");
        assert_eq!(global(&merged, "pi", "budget").unwrap().model, "glm-4.7");
        // A broken file is reported and ignored, never silently used.
        fs::write(&overlay, "not json").unwrap();
        assert_eq!(
            effective_at(&overlay, DEFAULT_SEAT).unwrap(),
            pins(SEATS, DEFAULT_SEAT).unwrap()
        );
        assert!(effective_at(&overlay, "nope").is_err());
        // An invalid entry is skipped; the valid one still applies.
        fs::write(
            &overlay,
            r#"{"schema":1,"seats":{"herdr-agents":[
                {"provider":"pi","pin":"enmaas/bad","tier":"nope","cost":"unknown"},
                {"provider":"pi","pin":"enmaas/good","tier":"budget","cost":"unknown"}
            ]}}"#,
        )
        .unwrap();
        let merged = effective_at(&overlay, DEFAULT_SEAT).unwrap();
        assert_eq!(pick(&merged, "pi", "budget").unwrap().model, "enmaas/good");
        assert!(pick(&merged, "pi", "nope").is_none());
    }

    #[test]
    fn sync_never_touches_the_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let seats = dir.path().join("lanista/seats.json");
        let overlay_dir = dir.path().join("vega/lanista");
        fs::create_dir_all(&overlay_dir).unwrap();
        let overlay = overlay_dir.join("seats.local.json");
        fs::write(&overlay, "{\"schema\":1,\"seats\":{}}").unwrap();
        sync(&seats).unwrap();
        fs::write(&seats, "{\"local\": 1}").unwrap();
        sync(&seats).unwrap();
        assert_eq!(
            fs::read_to_string(&overlay).unwrap(),
            "{\"schema\":1,\"seats\":{}}"
        );
        assert!(!overlay.with_extension("json.vega-bak").exists());
        assert_eq!(state(&seats), State::InSync);
    }
}
