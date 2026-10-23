//! Conditional GitHub API requests through `gh api -i`: gh holds the token and
//! the host config; the poller only adds `If-None-Match`.
//!
//! `gh api` exits 1 on a 304 ("gh: HTTP 304"), but with `-i` it prints the
//! status line and headers on stdout, so the status line decides, not the
//! exit code.

use std::process::Command;
use std::time::Duration;

/// A `gh api` call that hangs (network) must not stall the poller.
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resp {
    pub status: u16,
    pub etag: Option<String>,
    pub limit: Option<i64>,
    pub remaining: Option<i64>,
    pub reset: Option<i64>,
    pub retry_after: Option<i64>,
    pub body: String,
}

/// `GET <path>` on `host`, with `If-None-Match: <etag>` when given.
pub fn get(host: &str, path: &str, etag: Option<&str>) -> Result<Resp, String> {
    let mut cmd = Command::new(gh_bin());
    cmd.args(["api", "-i", "--hostname", host]);
    if let Some(e) = etag {
        cmd.args(["-H", &format!("If-None-Match: {e}")]);
    }
    cmd.arg(path);
    let out = vega_herdr::output_within(&mut cmd, TIMEOUT)
        .map_err(|e| format!("gh: {e}"))?
        .ok_or_else(|| format!("gh api {path}: no answer in {}s", TIMEOUT.as_secs()))?;
    parse(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
        let err = String::from_utf8_lossy(&out.stderr);
        format!(
            "gh api {path}: {}",
            err.lines().next().unwrap_or("no HTTP response")
        )
    })
}

/// `VEGA_CI_GH` (tests), else `gh` from PATH. vegad's PATH has no agent
/// shims, so its calls are not counted as an agent's.
fn gh_bin() -> String {
    std::env::var("VEGA_CI_GH").unwrap_or_else(|_| "gh".into())
}

/// Parses `gh api -i` output: status line, headers, blank line, body.
pub fn parse(out: &str) -> Option<Resp> {
    let mut lines = out.split('\n');
    let status = lines
        .next()?
        .trim()
        .strip_prefix("HTTP/")?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let mut r = Resp {
        status,
        ..Resp::default()
    };
    let mut consumed = out.find('\n').map_or(out.len(), |i| i + 1);
    for line in lines {
        consumed += line.len() + 1;
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "etag" => r.etag = Some(v.to_string()),
            "x-ratelimit-limit" => r.limit = v.parse().ok(),
            "x-ratelimit-remaining" => r.remaining = v.parse().ok(),
            "x-ratelimit-reset" => r.reset = v.parse().ok(),
            "retry-after" => r.retry_after = v.parse().ok(),
            _ => {}
        }
    }
    r.body = out.get(consumed.min(out.len())..).unwrap_or("").to_string();
    Some(r)
}

/// A path segment with `/` and other reserved characters escaped (branch
/// names such as `feat/x`).
pub fn escape(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_304_parses() {
        // `gh api -i -H 'If-None-Match: …' repos/cli/cli/pulls/1`, 2026-10-10.
        let out = std::fs::read_to_string(format!(
            "{}/tests/fixtures/api-304.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let r = parse(&out).unwrap();
        assert_eq!(r.status, 304);
        assert_eq!(
            r.etag.as_deref(),
            Some("\"22ecfb612ab041067316f24be02a07850ec41b67bf52c0ba09e42e515cb22a4d\"")
        );
        assert_eq!(
            (r.limit, r.remaining, r.reset),
            (Some(5000), Some(4856), Some(1791618450))
        );
        assert_eq!(r.body, "");
    }

    #[test]
    fn body_follows_the_blank_line() {
        let r =
            parse("HTTP/2.0 200 OK\r\nEtag: W/\"a\"\r\nRetry-After: 60\r\n\r\n{\"x\":1}").unwrap();
        assert_eq!(
            (r.status, r.etag.as_deref(), r.retry_after),
            (200, Some("W/\"a\""), Some(60))
        );
        assert_eq!(r.body, "{\"x\":1}");
        assert_eq!(parse("gh: not logged in"), None);
    }

    #[test]
    fn branch_names_are_escaped() {
        assert_eq!(escape("feat/x y"), "feat%2Fx%20y");
        assert_eq!(escape("v1.2-rc_1"), "v1.2-rc_1");
    }
}
