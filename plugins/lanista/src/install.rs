//! How lanista is installed: read from uv's tool dir, never by running lanista.
//!
//! `uv tool install` leaves `<tool dir>/lanista/uv-receipt.toml` (the
//! requested source: `git = ...` or `editable = ...`) and, for a git
//! install, the resolved commit in the dist-info's `direct_url.json`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use vega_herdr::output_within;

pub const REPO: &str = "https://github.com/b4arena/lanista";
pub const DEFAULT_REF: &str = "main";

const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq)]
pub enum Install {
    Missing,
    /// `uv tool install --editable <path>`: runs a checkout's working tree.
    Editable {
        path: String,
    },
    /// From git; `commit` is the resolved full sha, if found.
    Git {
        commit: Option<String>,
    },
    /// A uv tool from another source (PyPI, a local path) or a non-uv `lanista`.
    Other {
        what: String,
    },
}

pub struct Found {
    pub install: Install,
    pub version: Option<String>,
}

/// uv's tool dir (`uv tool dir`, honours `UV_TOOL_DIR`).
pub fn tool_dir() -> Result<PathBuf, String> {
    let out = output_within(Command::new("uv").args(["tool", "dir"]), TIMEOUT)
        .map_err(|_| "uv not found".to_string())?
        .ok_or("uv tool dir timed out")?;
    if !out.status.success() {
        return Err("uv tool dir failed".into());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// Reads the install from `<tool_dir>/lanista`; `on_path` is a `lanista`
/// found on PATH, for a non-uv install.
pub fn detect(tool_dir: &Path, on_path: Option<&Path>) -> Found {
    let dir = tool_dir.join("lanista");
    let Ok(receipt) = fs::read_to_string(dir.join("uv-receipt.toml")) else {
        let install = match on_path {
            Some(p) => Install::Other {
                what: format!("not a uv tool: {}", p.display()),
            },
            None => Install::Missing,
        };
        return Found {
            install,
            version: None,
        };
    };
    let dist = dist_info(&dir);
    let version = dist.as_ref().and_then(|d| {
        d.file_name()?
            .to_str()?
            .strip_prefix("lanista-")?
            .strip_suffix(".dist-info")
            .map(str::to_string)
    });
    let install = if let Some(path) = receipt_value(&receipt, "editable") {
        Install::Editable { path }
    } else if receipt_value(&receipt, "git").is_some() {
        let commit = dist
            .and_then(|d| fs::read_to_string(d.join("direct_url.json")).ok())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["vcs_info"]["commit_id"].as_str().map(str::to_string));
        Install::Git { commit }
    } else {
        Install::Other {
            what: receipt
                .lines()
                .find(|l| l.contains("name = \"lanista\""))
                .unwrap_or("unknown source")
                .trim()
                .to_string(),
        }
    };
    Found { install, version }
}

/// `key = "value"` inside the receipt's lanista requirement.
fn receipt_value(receipt: &str, key: &str) -> Option<String> {
    let line = receipt.lines().find(|l| l.contains("name = \"lanista\""))?;
    let start = line.find(&format!("{key} = \""))? + key.len() + 4;
    let rest = &line[start..];
    Some(rest[..rest.find('"')?].to_string())
}

/// `<dir>/lib/python*/site-packages/lanista-*.dist-info`.
fn dist_info(dir: &Path) -> Option<PathBuf> {
    let lib = dir.join("lib");
    fs::read_dir(lib).ok()?.flatten().find_map(|py| {
        fs::read_dir(py.path().join("site-packages"))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("lanista-") && n.ends_with(".dist-info"))
            })
    })
}

/// The commit `git_ref` names on GitHub: a branch or tag via `git ls-remote`,
/// or a full sha as given.
pub fn resolve(git_ref: &str) -> Result<String, String> {
    if is_sha(git_ref) {
        return Ok(git_ref.to_lowercase());
    }
    let out = output_within(
        Command::new("git").args([
            "ls-remote",
            REPO,
            &format!("refs/heads/{git_ref}"),
            &format!("refs/tags/{git_ref}"),
        ]),
        TIMEOUT,
    )
    .map_err(|e| format!("git: {e}"))?
    .ok_or("git ls-remote timed out")?;
    if !out.status.success() {
        return Err(format!("git ls-remote {REPO} failed"));
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| format!("no branch or tag `{git_ref}` in {REPO}"))
}

/// The uv requirement for a commit; pinned so a cached ref cannot win.
pub fn spec(commit: &str) -> String {
    format!("git+{REPO}@{commit}")
}

pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// A ref for output: a full sha shortened, a branch or tag as is.
pub fn ref_label(git_ref: &str) -> &str {
    if is_sha(git_ref) {
        short(git_ref)
    } else {
        git_ref
    }
}

fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(receipt: &str, dist: Option<(&str, &str)>) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("lanista");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("uv-receipt.toml"), receipt).unwrap();
        if let Some((name, direct)) = dist {
            let di = d.join("lib/python3.12/site-packages").join(name);
            fs::create_dir_all(&di).unwrap();
            fs::write(di.join("direct_url.json"), direct).unwrap();
        }
        t
    }

    #[test]
    fn detects_a_git_install_with_commit_and_version() {
        let t = tool(
            include_str!("../tests/fixtures/receipt-git.toml"),
            Some((
                "lanista-0.1.0.dist-info",
                include_str!("../tests/fixtures/direct_url-git.json"),
            )),
        );
        let f = detect(t.path(), None);
        assert_eq!(
            f.install,
            Install::Git {
                commit: Some("03892da05d3403408fc055a2122de1fc164ebc1d".into())
            }
        );
        assert_eq!(f.version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn detects_ganymedes_editable_install() {
        let t = tool(
            include_str!("../tests/fixtures/receipt-editable.toml"),
            None,
        );
        assert_eq!(
            detect(t.path(), None).install,
            Install::Editable {
                path: "/Users/mhild/src/durandom/lanista".into()
            }
        );
    }

    #[test]
    fn missing_or_foreign() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(detect(t.path(), None).install, Install::Missing);
        let other = detect(t.path(), Some(Path::new("/opt/bin/lanista"))).install;
        assert!(matches!(other, Install::Other { what } if what.contains("/opt/bin/lanista")));
        let pypi = tool("[tool]\nrequirements = [{ name = \"lanista\" }]\n", None);
        assert!(matches!(
            detect(pypi.path(), None).install,
            Install::Other { .. }
        ));
    }

    #[test]
    fn full_shas_resolve_offline_and_specs_pin_them() {
        let sha = "03892DA05D3403408FC055A2122DE1FC164EBC1D";
        assert_eq!(resolve(sha).unwrap(), sha.to_lowercase());
        assert_eq!(spec("abc"), "git+https://github.com/b4arena/lanista@abc");
        assert_eq!(short("03892da05d"), "03892da");
        assert_eq!(ref_label(&sha.to_lowercase()), "03892da");
        assert_eq!(ref_label("main"), "main");
    }
}
