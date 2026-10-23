//! Which forge a host runs: github.com is GitHub without a probe; any other
//! host is asked once (`GET /api/v1/version`, which Forgejo and Gitea answer
//! and GitHub Enterprise does not) and the answer is cached per host in
//! `forges.json` (see [`crate::store`]). No guessing from the host name.

use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::target::GITHUB;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    #[default]
    GitHub,
    /// Forgejo or Gitea: the same `/api/v1`.
    Forgejo,
}

impl Forge {
    pub fn is_github(&self) -> bool {
        *self == Forge::GitHub
    }
}

/// `host -> (HTTP status, body)` of `GET https://<host>/api/v1/version`:
/// [`http_probe`] in the CLI, a fake in tests.
pub type Probe<'a> = dyn FnMut(&str) -> Result<(u16, String), String> + 'a;

/// What one probe says about a host's forge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detected {
    /// Certain, so cached: a 2xx with `{"version": "…"}` is Forgejo/Gitea, a
    /// 404 is not (GitHub Enterprise has no such route).
    Sure(Forge),
    /// A guess, so not cached: any other 2xx, 3xx or 4xx is taken as GitHub,
    /// but a captive portal (302) or a Forgejo that wants a login (401, 403)
    /// answers the same, and the next probe may tell.
    Guess(Forge),
}

impl Detected {
    pub fn forge(self) -> Forge {
        match self {
            Detected::Sure(f) | Detected::Guess(f) => f,
        }
    }
}

/// The forge of `host` from one probe. A 5xx or a failed request tells
/// nothing: an error.
pub fn detect(host: &str, probe: &mut Probe) -> Result<Detected, String> {
    if host == GITHUB {
        return Ok(Detected::Sure(Forge::GitHub));
    }
    let (status, body) = probe(host)?;
    match status {
        200..=299 => {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            Ok(if v.get("version").is_some_and(|v| v.is_string()) {
                Detected::Sure(Forge::Forgejo)
            } else {
                Detected::Guess(Forge::GitHub)
            })
        }
        404 => Ok(Detected::Sure(Forge::GitHub)),
        300..=499 => Ok(Detected::Guess(Forge::GitHub)),
        s => Err(format!(
            "{host}/api/v1/version: HTTP {s}, cannot tell the forge"
        )),
    }
}

/// The forge of `host`: github.com, else the cached answer, else [`detect`]
/// (cached only when [`Detected::Sure`]).
pub fn resolve(host: &str, probe: &mut Probe) -> Result<Forge, String> {
    if host == GITHUB {
        return Ok(Forge::GitHub);
    }
    let mut forges = crate::store::load_forges();
    if let Some(f) = forges.get(host) {
        return Ok(*f);
    }
    let d = detect(host, probe)?;
    if let Detected::Sure(f) = d {
        forges.insert(host.to_string(), f);
        crate::store::save_forges(&forges);
    }
    Ok(d.forge())
}

const TIMEOUT: Duration = Duration::from_secs(15);

/// The real probe: `curl` (the endpoint needs no token).
pub fn http_probe(host: &str) -> Result<(u16, String), String> {
    let url = format!("https://{host}/api/v1/version");
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--max-time", "10", "-w", "\n%{http_code}", &url]);
    let out = vega_herdr::output_within(&mut cmd, TIMEOUT)
        .map_err(|e| format!("curl: {e}"))?
        .ok_or_else(|| format!("{url}: no answer in {}s", TIMEOUT.as_secs()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    match code.trim().parse::<u16>() {
        Ok(s) if s > 0 => Ok((s, body.to_string())),
        _ => Err(format!(
            "{url}: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or("no HTTP response")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(status: u16, body: &str) -> impl FnMut(&str) -> Result<(u16, String), String> + '_ {
        move |_| Ok((status, body.to_string()))
    }

    #[test]
    fn version_endpoint_identifies_forgejo() {
        // git.b4mad.industries, 2026-10-12.
        let body = r#"{"version":"16.0.5+gitea-1.22.0"}"#;
        use Detected::{Guess, Sure};
        assert_eq!(
            detect("git.example.org", &mut fake(200, body)),
            Ok(Sure(Forge::Forgejo))
        );
        assert_eq!(
            detect("ghe.corp.com", &mut fake(404, "")),
            Ok(Sure(Forge::GitHub))
        );
        // A captive portal, a Forgejo behind a login: GitHub for now, not for good.
        for status in [302, 401, 403] {
            assert_eq!(
                detect("x.org", &mut fake(status, "")),
                Ok(Guess(Forge::GitHub)),
                "{status}"
            );
        }
        assert_eq!(
            detect("x.org", &mut fake(200, "<html>")),
            Ok(Guess(Forge::GitHub))
        );
        assert!(detect("x.org", &mut fake(502, "")).is_err());
        let mut never = |_: &str| -> Result<(u16, String), String> { panic!("probed github.com") };
        assert_eq!(detect(GITHUB, &mut never), Ok(Sure(Forge::GitHub)));
    }

    #[test]
    fn serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Forge::Forgejo).unwrap(),
            "\"forgejo\""
        );
        assert_eq!(
            serde_json::from_str::<Forge>("\"github\"").unwrap(),
            Forge::GitHub
        );
    }
}
