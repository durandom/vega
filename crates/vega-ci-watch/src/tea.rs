//! Forgejo/Gitea API requests through `tea api -i`: tea holds the token and
//! picks the login by the URL's host.
//!
//! Unlike `gh api -i`, tea prints the status line and headers on **stderr** and
//! the body on stdout, and it exits 0 for any HTTP status (404 included; it
//! exits 1 only for its own errors such as no login). So the status line
//! decides, never the exit code. Forgejo sends no ETag and no rate-limit
//! headers (verified on git.b4mad.industries, 2026-10-12), so there are no
//! conditional requests.

use std::process::Command;
use std::time::Duration;

use crate::gh::{parse, Resp};

/// A call that hangs (network) must not stall the poller.
const TIMEOUT: Duration = Duration::from_secs(30);

/// `GET https://<host>/api/v1/<path>`.
pub fn get(host: &str, path: &str) -> Result<Resp, String> {
    let url = format!("https://{host}/api/v1/{}", path.trim_start_matches('/'));
    let mut cmd = Command::new(tea_bin());
    cmd.args(["api", "-i", &url]);
    let out = match vega_herdr::output_within(&mut cmd, TIMEOUT) {
        Ok(Some(out)) => out,
        Ok(None) => {
            return Err(format!(
                "tea api {url}: no answer in {}s",
                TIMEOUT.as_secs()
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(
                "tea not found on vegad's PATH or in ~/.local/share/mise/shims; Forgejo targets need it: brew install tea, or mise use -g go:gitea.dev/tea@0.16.0 (needs Go), then tea login add".into(),
            )
        }
        Err(e) => return Err(format!("tea: {e}")),
    };
    let (stderr, stdout) = (
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );
    from_output(&stderr, &stdout).ok_or_else(|| {
        // tea's own errors: "Error: no available login", "Error: login name … does not exist".
        let line = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no HTTP response");
        format!("tea api {url}: {line}")
    })
}

/// `VEGA_CI_TEA` (tests), else `tea` from PATH, else mise's shim: vegad is started from
/// wherever and its PATH may lack the mise shims dir that tea lives in.
fn tea_bin() -> String {
    if let Ok(bin) = std::env::var("VEGA_CI_TEA") {
        return bin;
    }
    let on_path = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("tea").is_file()));
    let shim = std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/share/mise/shims/tea"))
        .filter(|p| p.is_file());
    match (on_path, shim) {
        (false, Some(shim)) => shim.display().to_string(),
        _ => "tea".into(),
    }
}

/// The response from tea's stderr (status line and headers, maybe preceded by
/// notices) and stdout (body).
pub fn from_output(stderr: &str, stdout: &str) -> Option<Resp> {
    let from = stderr
        .lines()
        .scan(0, |off, l| {
            let at = *off;
            *off += l.len() + 1;
            Some((at, l))
        })
        .find(|(_, l)| l.starts_with("HTTP/"))?
        .0;
    parse(&format!("{}\n\n{stdout}", stderr[from..].trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn recorded(name: &str) -> Resp {
        from_output(
            &fixture(&format!("{name}.stderr")),
            &fixture(&format!("{name}.stdout")),
        )
        .unwrap()
    }

    #[test]
    fn recorded_responses_parse() {
        // Recorded 2026-10-12 from git.b4mad.industries (Forgejo, API 1.22).
        let ok = recorded("tea-pull");
        assert_eq!(
            (ok.status, ok.etag.clone(), ok.remaining),
            (200, None, None)
        );
        assert!(ok.body.contains("\"merged\": false"), "{}", ok.body);
        let missing = recorded("tea-404");
        assert_eq!(missing.status, 404);
        assert!(missing.body.contains("branch does not exist"));
        assert_eq!(recorded("tea-status-none").status, 200);
    }

    #[test]
    fn notices_before_the_status_line_are_skipped() {
        let r = from_output(
            "note: a new tea is out\nHTTP/2.0 429 Too Many Requests\nRetry-After: 90\n",
            "{}",
        )
        .unwrap();
        assert_eq!(
            (r.status, r.retry_after, r.body.as_str()),
            (429, Some(90), "{}")
        );
        assert_eq!(from_output("Error: no available login\n", ""), None);
    }
}
