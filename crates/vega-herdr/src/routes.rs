//! lanista's private routes (lanista `docs/routes.md`, b4arena/lanista):
//! route id → canonical id plus route facts, resolved by lanista from its
//! host-local `$XDG_CONFIG_HOME/lanista/routes.local.json` overlay.
//!
//! vega never reads that file: it calls `lanista --json routes` (one bounded
//! subprocess per command, `bin`), which prints every route with each field
//! resolved and an `origin` (`local`, `catalog` or `unknown`). The join key
//! is the exact runtime route id, string-equal. Unknown is never zero: a
//! fact lanista cannot resolve is `None`, never a fabricated value.
//!
//! A missing, old, hung or broken lanista is [`Unavailable`]: callers fall
//! back to the deprecated `canonical`/`cost` of vega's seats overlay
//! (`plugins/lanista/src/seats.rs`) with the hint on stderr.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

/// `lanista` answers fast; a hang must not stall a spawn.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Names the lanista binary, as [`bin`] resolves it (tests point it at a
/// fake). Mirrors token-burn's `VEGA_TOKEN_BURN`.
pub const BIN_ENV: &str = "VEGA_LANISTA_BIN";

/// One resolved route, as `lanista --json routes` prints it. Only the facts
/// vega displays; the rest of lanista's fields stay unparsed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Route {
    pub route_id: String,
    /// The catalog's canonical id, when established.
    pub canonical: Option<String>,
    /// `subscription|budget|unknown` (lanista's vocabulary); `None` is
    /// unknown — never zero, never a guess.
    pub cost: Option<String>,
    /// The cost field's origin: `local`, `catalog` or `unknown`.
    pub cost_origin: String,
}

/// The parsed `lanista --json routes` output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub routes: BTreeMap<String, Route>,
    /// lanista's overlay summary: present/loaded/route_count.
    pub loaded: Option<bool>,
    pub route_count: Option<u64>,
}

impl Table {
    pub fn route(&self, route_id: &str) -> Option<&Route> {
        self.routes.get(route_id)
    }
}

/// The facts one spawned model runs on, merged per the ownership contract
/// with vega (vega's seats overlay owns provider+tier → route id; lanista
/// owns route id → these facts; seats `canonical`/`cost` are fallbacks).
/// Built by vega's lanista plugin (`vega_lanista::routes::resolve`).
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub route_id: String,
    pub canonical: Option<String>,
    /// `subscription|budget|unknown` (lanista's vocabulary); `None` unknown.
    pub cost: Option<String>,
    /// Where the values come from: lanista's per-field origin (`local`,
    /// `catalog`, `unknown`) or `seats-overlay` for the fallback.
    pub origin: &'static str,
}

impl Facts {
    /// `route budget, origin local` — the cost class and its origin, as
    /// the capacity line shows them.
    pub fn summary(&self) -> String {
        format!(
            "route {}, origin {}",
            self.cost.as_deref().unwrap_or("cost unknown"),
            self.origin
        )
    }
}

/// Why the routes table could not be read. `Display` is one line a caller
/// can print after `vega lanista: `.
#[derive(Debug, Clone, PartialEq)]
pub struct Unavailable {
    pub why: String,
    pub hint: &'static str,
}

impl Unavailable {
    fn new(why: impl Into<String>) -> Self {
        Self {
            why: why.into(),
            hint: "upgrade or install it: vega lanista install",
        }
    }
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "lanista routes unavailable ({}); {}",
            self.why, self.hint
        )
    }
}

/// The lanista binary: `VEGA_LANISTA_BIN`, else `lanista` from PATH.
pub fn bin() -> String {
    std::env::var_os(BIN_ENV)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "lanista".into())
}

/// Reads the routes table once: `lanista --json routes`, bounded by
/// [`TIMEOUT`]. Every failure mode — not installed, too old to know the
/// subcommand, hung, or unexpected output — is [`Unavailable`], so a host
/// without route facts still works on the seats fallback.
pub fn read() -> Result<Table, Unavailable> {
    read_bin(&bin(), TIMEOUT)
}

/// [`read`] against an explicit binary and timeout (tests).
pub fn read_bin(bin: &str, timeout: Duration) -> Result<Table, Unavailable> {
    let out = crate::output_within(
        std::process::Command::new(bin).args(["--json", "routes"]),
        timeout,
    )
    .map_err(|e| Unavailable::new(format!("cannot run {bin}: {e}")))?;
    let Some(out) = out else {
        return Err(Unavailable::new(format!(
            "{bin} --json routes did not answer within {}s",
            timeout.as_secs()
        )));
    };
    if !out.status.success() {
        return Err(Unavailable::new(format!(
            "{bin} --json routes exited with {} (an old lanista has no routes command?)",
            out.status.code().unwrap_or(-1)
        )));
    }
    parse(&out.stdout)
}

/// Parses `lanista --json routes` output (pure, for tests). Unexpected
/// shapes are an error, never silently-empty facts.
pub fn parse(stdout: &[u8]) -> Result<Table, Unavailable> {
    let v: Value = serde_json::from_slice(stdout)
        .map_err(|e| Unavailable::new(format!("output is not JSON: {e}")))?;
    let routes = v["routes"]
        .as_object()
        .ok_or_else(|| Unavailable::new("output has no routes object"))?;
    let mut table = Table {
        loaded: v["overlay"]["loaded"].as_bool(),
        route_count: v["overlay"]["route_count"].as_u64(),
        routes: BTreeMap::new(),
    };
    for (id, body) in routes {
        let route = Route {
            route_id: body["route_id"].as_str().unwrap_or(id).to_string(),
            canonical: body["canonical"].as_str().map(str::to_string),
            cost: body["fields"]["cost"]["value"].as_str().map(str::to_string),
            cost_origin: body["fields"]["cost"]["origin"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
        };
        table.routes.insert(id.clone(), route);
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn script(dir: &std::path::Path, body: &str) -> String {
        let path = dir.join("fake-lanista");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        make_executable(&path);
        path.to_string_lossy().into_owned()
    }

    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn listing() -> Value {
        json!({
            "overlay": {"path": "/x/routes.local.json", "present": true, "loaded": true,
                        "route_count": 2, "diagnostics": []},
            "routes": {
                "example-gw/glm-5.3": {
                    "route_id": "example-gw/glm-5.3", "visibility": "private-local",
                    "canonical": "glm-5-3", "has_public_card": true,
                    "fields": {
                        "cost": {"value": "budget", "source_note": "gw list", "as_of": "2026-01-01", "origin": "local"},
                        "pricing_per_million": {"value": null, "status": "unknown", "origin": "unknown"},
                        "modalities": {"value": {"input": ["text"], "output": ["text"]}, "origin": "catalog"}
                    },
                    "overlay_path": "/x/routes.local.json"
                },
                "enmaas/gome-3.1": {
                    "route_id": "enmaas/gome-3.1", "visibility": "private-local",
                    "canonical": null, "has_public_card": false,
                    "fields": {
                        "cost": {"value": null, "status": "unknown", "origin": "local"}
                    },
                    "overlay_path": "/x/routes.local.json"
                }
            }
        })
    }

    #[test]
    fn parses_the_documented_shape() {
        let table = parse(listing().to_string().as_bytes()).unwrap();
        assert_eq!(table.loaded, Some(true));
        assert_eq!(table.route_count, Some(2));
        let gw = table.route("example-gw/glm-5.3").unwrap();
        assert_eq!(gw.canonical.as_deref(), Some("glm-5-3"));
        assert_eq!(gw.cost.as_deref(), Some("budget"));
        assert_eq!(gw.cost_origin, "local");
        // An explicit null masks the card: unknown, never zero, never a guess.
        let enmaas = table.route("enmaas/gome-3.1").unwrap();
        assert_eq!(enmaas.canonical, None);
        assert_eq!(enmaas.cost, None);
        assert_eq!(enmaas.cost_origin, "local");
        assert!(table.route("glm-5.3").is_none());
    }

    #[test]
    fn rejects_output_that_is_not_the_contract() {
        for bad in ["", "not json", r#"{"overlay": {}}"#, r#"{"routes": []}"#] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn reads_a_fake_lanista_or_names_why_not() {
        let dir = tempfile::tempdir().unwrap();
        let ok = script(
            dir.path(),
            r#"echo '{"overlay":{"loaded":true,"route_count":0},"routes":{}}'"#,
        );
        let table = read_bin(&ok, TIMEOUT).unwrap();
        assert_eq!(table.route_count, Some(0));
        let old = script(dir.path(), "echo 'usage: no routes command' >&2; exit 2");
        assert_eq!(
            read_bin(&old, TIMEOUT).unwrap_err().why,
            format!("{old} --json routes exited with 2 (an old lanista has no routes command?)")
        );
        let garbage = script(dir.path(), "echo not-json");
        assert!(read_bin(&garbage, TIMEOUT)
            .unwrap_err()
            .why
            .contains("not JSON"));
        let slow = script(dir.path(), "sleep 5");
        assert!(read_bin(&slow, Duration::from_millis(100))
            .unwrap_err()
            .why
            .contains("did not answer"));
        let missing = dir.path().join("missing").to_string_lossy().into_owned();
        assert!(read_bin(&missing, TIMEOUT)
            .unwrap_err()
            .why
            .contains("cannot run"));
        for u in [
            read_bin(&old, TIMEOUT).unwrap_err(),
            read_bin(&slow, Duration::from_millis(100)).unwrap_err(),
        ] {
            assert_eq!(u.hint, "upgrade or install it: vega lanista install");
            assert!(u.to_string().starts_with("lanista routes unavailable ("));
        }
    }
}
