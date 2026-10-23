//! token-burn's health: the binary and its version, its user service
//! (`service-status --json`), and the age of the newest stored sample.
//! Read-only: vega never installs or starts token-burn.
//!
//! With `VEGA_CAPACITY_FIXTURE` the binary is not run; an optional
//! `service-status.json` in that dir stands in for the service.

use std::env;
use std::path::Path;

use serde_json::Value;
use vega_herdr::capacity::token_burn;
use vega_herdr::capacity::{duration, STALE_SECS};

const GET: &str = "install token-burn (github.com/durandom/token-burn), then `token-burn install` runs its daemon as a user service";
const REFRESH: &str = "`token-burn install` runs the daemon as a user service; `token-burn once --store` stores one sample";

/// token-burn's user service, as `service-status --json` reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Service {
    Running,
    Stopped,
    None,
    Unknown,
}

impl Service {
    fn text(self) -> &'static str {
        match self {
            Service::Running => "service running",
            Service::Stopped => "service installed, not running",
            Service::None => "no service",
            Service::Unknown => "service unknown",
        }
    }

    fn from_json(json: &str) -> Service {
        match serde_json::from_str::<Value>(json) {
            Ok(v) if v["Loaded"] == true => Service::Running,
            Ok(v) if v["Installed"] == true => Service::Stopped,
            Ok(v) if v["Installed"] == false => Service::None,
            _ => Service::Unknown,
        }
    }
}

/// The binary's version (`None` without one, e.g. a fixture) or why it is
/// missing, and its service.
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub version: Result<Option<String>, String>,
    pub service: Service,
}

/// Reads the version and the service; both calls in parallel.
pub fn read() -> Health {
    if let Some(dir) = env::var("VEGA_CAPACITY_FIXTURE")
        .ok()
        .filter(|d| !d.is_empty())
    {
        let service = std::fs::read_to_string(Path::new(&dir).join("service-status.json"))
            .map_or(Service::Unknown, |s| Service::from_json(&s));
        return Health {
            version: Ok(None),
            service,
        };
    }
    let bin = token_burn::bin();
    let service = {
        let bin = bin.clone();
        std::thread::spawn(move || token_burn::run(&bin, &["service-status", "--json"]))
    };
    // `token-burn v0.1.37` on the first line.
    let version = token_burn::run(&bin, &["version"]).map(|out| {
        out.lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .map(String::from)
    });
    let service = service
        .join()
        .ok()
        .and_then(Result::ok)
        .map_or(Service::Unknown, |s| Service::from_json(&s));
    Health { version, service }
}

/// `token_burn: ok (v0.1.37, service running, sample 2m old)`, with a
/// `help:` line when it is missing, has no sample or a stale one. `age` is
/// the newest sample's age in seconds.
pub fn lines(h: &Health, age: Option<i64>) -> Vec<String> {
    let version = match &h.version {
        Err(why) => {
            return vec![
                format!("token_burn: missing ({why})"),
                format!("help: {GET}"),
            ]
        }
        Ok(Some(v)) => v.clone(),
        Ok(None) => "fixture".into(),
    };
    let service = h.service.text();
    match age {
        None => vec![
            format!("token_burn: no sample ({version}, {service})"),
            format!("help: {REFRESH}"),
        ],
        Some(a) if a > STALE_SECS => vec![
            format!(
                "token_burn: stale ({version}, {service}, sample {} old)",
                duration(a)
            ),
            format!("help: {REFRESH}"),
        ],
        Some(a) => vec![format!(
            "token_burn: ok ({version}, {service}, sample {} old)",
            duration(a)
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(version: Result<Option<&str>, &str>, service: Service) -> Health {
        Health {
            version: version.map(|v| v.map(String::from)).map_err(String::from),
            service,
        }
    }

    #[test]
    fn service_from_token_burns_json() {
        let j = |i: bool, l: bool| format!(r#"{{"Installed":{i},"Loaded":{l}}}"#);
        assert_eq!(Service::from_json(&j(true, true)), Service::Running);
        assert_eq!(Service::from_json(&j(true, false)), Service::Stopped);
        assert_eq!(Service::from_json(&j(false, false)), Service::None);
        assert_eq!(Service::from_json("nope"), Service::Unknown);
    }

    #[test]
    fn ok_stale_missing_and_no_sample() {
        let ok = h(Ok(Some("v0.1.37")), Service::Running);
        assert_eq!(
            lines(&ok, Some(120)),
            ["token_burn: ok (v0.1.37, service running, sample 2m old)"]
        );
        let stale = lines(&h(Ok(Some("v0.1.37")), Service::None), Some(3 * 3600));
        assert_eq!(
            stale[0],
            "token_burn: stale (v0.1.37, no service, sample 3h old)"
        );
        assert!(stale[1].contains("token-burn install"));
        let missing = lines(&h(Err("token-burn not found"), Service::Unknown), None);
        assert_eq!(missing[0], "token_burn: missing (token-burn not found)");
        assert!(missing[1].starts_with("help: install token-burn"));
        let none = lines(&h(Ok(None), Service::Unknown), None);
        assert_eq!(none[0], "token_burn: no sample (fixture, service unknown)");
    }
}
