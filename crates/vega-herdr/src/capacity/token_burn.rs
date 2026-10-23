//! The token-burn source adapter: maps token-burn's stored sample
//! (`status --json`, `forecast --json`) into vega's readings. The only place
//! that knows token-burn's field names; another source is another adapter.
//!
//! Overrides: `VEGA_TOKEN_BURN` names the binary; `VEGA_CAPACITY_FIXTURE` is
//! a dir with recorded `status.json` and `forecast.json` (tests, live checks).

use std::env;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::{parse_rfc3339, Reading, Sample, Scope, Source};
use crate::output_within;

/// token-burn reads SQLite and answers in about a second.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Forecasts below this confidence are a weak signal and ignored.
const MIN_CONFIDENCE: f64 = 0.5;

/// token-burn, as `VEGA_TOKEN_BURN` / `VEGA_CAPACITY_FIXTURE` say.
pub struct TokenBurn {
    pub bin: String,
    pub fixture: Option<String>,
}

impl TokenBurn {
    pub fn from_env() -> Self {
        TokenBurn {
            bin: bin(),
            fixture: env::var("VEGA_CAPACITY_FIXTURE")
                .ok()
                .filter(|d| !d.is_empty()),
        }
    }
}

impl Source for TokenBurn {
    /// Both commands in parallel. A failed forecast leaves the readings
    /// without run-outs.
    fn read(&self) -> Result<Sample, String> {
        if let Some(dir) = &self.fixture {
            let file = |f: &str| std::fs::read_to_string(Path::new(dir).join(f)).ok();
            let status = file("status.json").ok_or("fixture without status.json")?;
            let status = rows(&status).ok_or("fixture status.json is no JSON array")?;
            let forecast = file("forecast.json")
                .and_then(|f| rows(&f))
                .unwrap_or_default();
            return Ok(sample(&status, &forecast));
        }
        let forecast = {
            let bin = self.bin.clone();
            thread::spawn(move || run(&bin, &["forecast", "--json"]))
        };
        let status = run(&self.bin, &["status", "--json"])?;
        let status = rows(&status).ok_or("token-burn status --json is no JSON array")?;
        let forecast = forecast
            .join()
            .ok()
            .and_then(Result::ok)
            .and_then(|f| rows(&f))
            .unwrap_or_default();
        Ok(sample(&status, &forecast))
    }
}

/// The token-burn binary: `VEGA_TOKEN_BURN`, else `token-burn` on PATH.
pub fn bin() -> String {
    env::var("VEGA_TOKEN_BURN").unwrap_or_else(|_| "token-burn".into())
}

/// Runs `bin args` within the timeout; `Err` says why it gave nothing.
pub fn run(bin: &str, args: &[&str]) -> Result<String, String> {
    match output_within(Command::new(bin).args(args), TIMEOUT) {
        Err(_) => Err(format!("{bin} not found")),
        Ok(None) => Err(format!("{bin} {} timed out", args[0])),
        Ok(Some(o)) if !o.status.success() => Err(format!("{bin} {} failed", args[0])),
        Ok(Some(o)) => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
    }
}

pub(super) fn rows(json: &str) -> Option<Vec<Value>> {
    match serde_json::from_str(json).ok()? {
        Value::Array(a) => Some(a),
        Value::Null => Some(Vec::new()),
        _ => None,
    }
}

/// token-burn's rows as readings, in status order; every scope `account`
/// (the window scopes are vega's config, not token-burn's).
pub(super) fn sample(status: &[Value], forecast: &[Value]) -> Sample {
    let readings = status
        .iter()
        .map(|r| {
            let provider = r["Provider"].as_str().unwrap_or_default();
            let window = r["WindowName"].as_str().unwrap_or_default();
            let left = if r["LimitReached"] == true {
                0.0
            } else {
                r["RemainingPercent"].as_f64().unwrap_or(0.0)
            };
            let runs_out_at = forecast
                .iter()
                .find(|f| f["provider"] == provider && f["window_name"] == window)
                .filter(|f| f["confidence"].as_f64().unwrap_or(0.0) >= MIN_CONFIDENCE)
                .and_then(|f| f["estimated_100_at"].as_str().and_then(parse_rfc3339));
            Reading {
                provider: provider.into(),
                window: window.into(),
                scope: Scope::Account,
                left,
                reset_at: r["ResetAt"].as_str().and_then(parse_rfc3339),
                runs_out_at,
                observed_at: r["ObservedAt"].as_str().and_then(parse_rfc3339),
            }
        })
        .collect();
    Sample { readings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_token_burn_rows_and_drops_weak_forecasts() {
        let status = json!([
            {"Provider": "codex", "WindowName": "primary", "RemainingPercent": 40.0,
             "LimitReached": true, "ResetAt": "2026-10-12T00:00:00Z",
             "ObservedAt": "2026-10-10T07:39:45.2Z"},
            {"Provider": "claude", "WindowName": "five_hour", "RemainingPercent": 6.0,
             "LimitReached": false}
        ]);
        let forecast = json!([
            {"provider": "codex", "window_name": "primary", "confidence": 0.9,
             "estimated_100_at": "2026-10-11T00:00:00Z"},
            {"provider": "claude", "window_name": "five_hour", "confidence": 0.3,
             "estimated_100_at": "2026-10-11T00:00:00Z"}
        ]);
        let s = sample(status.as_array().unwrap(), forecast.as_array().unwrap());
        let codex = &s.readings[0];
        assert_eq!(codex.left, 0.0, "LimitReached is empty");
        assert_eq!(codex.scope, Scope::Account);
        assert_eq!(codex.reset_at, parse_rfc3339("2026-10-12T00:00:00Z"));
        assert_eq!(codex.runs_out_at, parse_rfc3339("2026-10-11T00:00:00Z"));
        assert_eq!(codex.observed_at, parse_rfc3339("2026-10-10T07:39:45Z"));
        let claude = &s.readings[1];
        assert_eq!((claude.left, claude.runs_out_at), (6.0, None));
        assert_eq!((claude.reset_at, claude.observed_at), (None, None));
    }

    #[test]
    fn missing_binary_is_unknown() {
        assert_eq!(
            run("/nonexistent/token-burn", &["status", "--json"]),
            Err("/nonexistent/token-burn not found".into())
        );
        let tb = TokenBurn {
            bin: "/nonexistent/token-burn".into(),
            fixture: None,
        };
        assert_eq!(
            tb.read().err().as_deref(),
            Some("/nonexistent/token-burn not found")
        );
    }

    #[test]
    fn fixture_reads_recorded_files() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/token-burn");
        let tb = TokenBurn {
            bin: "/nonexistent/token-burn".into(),
            fixture: Some(dir.to_string_lossy().into_owned()),
        };
        let s = tb.read().unwrap();
        assert_eq!(s.readings.len(), 14);
        assert!(s.readings.iter().any(|r| r.runs_out_at.is_some()));
    }
}
