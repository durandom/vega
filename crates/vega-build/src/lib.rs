//! Version stamping for the vega binaries.
//!
//! Each binary crate has a one-line `build.rs` calling [`emit`], which runs git and exports
//! `VEGA_VERSION` (`<ver> (release, <sha>)` or `<ver>-dev (...)`). Binaries print it with
//! [`version_line!`]. `env!` resolves per crate, so every binary crate needs its own build script.

use std::path::PathBuf;
use std::process::Command;

/// What git said about the tree at build time.
#[derive(Debug, Default, PartialEq)]
pub struct GitInfo {
    /// Short commit hash; `None` when git is unavailable or this is no checkout.
    pub sha: Option<String>,
    /// Commits between `v<ver>` and HEAD; `None` when the tag does not exist.
    pub past_tag: Option<u32>,
    /// Tracked files differ from HEAD.
    pub dirty: bool,
}

/// Release = HEAD exactly at `v<ver>` and a clean tree; anything else is dev.
pub fn format_version(ver: &str, git: &GitInfo) -> String {
    let sha = git.sha.as_deref().unwrap_or("unknown");
    let dirty = if git.dirty { ", dirty" } else { "" };
    match git.past_tag {
        Some(0) if !git.dirty => format!("{ver} (release, {sha})"),
        Some(0) => format!("{ver}-dev (at v{ver}, {sha}{dirty})"),
        Some(1) => format!("{ver}-dev (1 commit past v{ver}, {sha}{dirty})"),
        Some(n) => format!("{ver}-dev ({n} commits past v{ver}, {sha}{dirty})"),
        None => format!("{ver}-dev ({sha}{dirty})"),
    }
}

/// `<binary> <version string>`, the `--version` output. Expands in the calling binary.
#[macro_export]
macro_rules! version_line {
    () => {
        concat!(env!("CARGO_BIN_NAME"), " ", env!("VEGA_VERSION"))
    };
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn probe(ver: &str) -> GitInfo {
    let Some(sha) = git(&["rev-parse", "--short", "HEAD"]) else {
        return GitInfo::default();
    };
    let tag = format!("v{ver}");
    let past_tag = git(&[
        "rev-parse",
        "-q",
        "--verify",
        &format!("refs/tags/{tag}^{{commit}}"),
    ])
    .and_then(|tag_commit| {
        let head = git(&["rev-parse", "HEAD"])?;
        if head == tag_commit {
            return Some(0);
        }
        git(&["rev-list", "--count", &format!("{tag}..HEAD")])?
            .parse()
            .ok()
    });
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    GitInfo {
        sha: Some(sha),
        past_tag,
        dirty,
    }
}

/// Existing paths whose change can alter the stamp; `git --git-path` resolves them inside
/// worktrees, where `.git` is a file.
fn watch_paths() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut names = vec![
        "HEAD".to_owned(),
        "index".to_owned(),
        "packed-refs".to_owned(),
    ];
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        names.push(head_ref);
    }
    // The tag dir, not `refs/tags/v<ver>`: cargo treats a missing path as always stale, so
    // watching a tag that does not exist yet would rebuild every binary on every build.
    names.push("refs/tags".to_owned());
    names
        .iter()
        .filter_map(|n| git(&["rev-parse", "--git-path", n]))
        .map(|p| cwd.join(p))
        .filter(|p| p.exists())
        .collect()
}

/// Call from `build.rs`: exports `VEGA_VERSION` and sets the re-run triggers.
pub fn emit() {
    let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    for p in watch_paths() {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    // Edits to tracked files do not touch the index until staged; watching the crate's own
    // sources keeps the dirty flag right for the usual edit-build loop.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!(
        "cargo:rustc-env=VEGA_VERSION={}",
        format_version(&ver, &probe(&ver))
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(sha: Option<&str>, past_tag: Option<u32>, dirty: bool) -> GitInfo {
        GitInfo {
            sha: sha.map(str::to_owned),
            past_tag,
            dirty,
        }
    }

    #[test]
    fn release() {
        assert_eq!(
            format_version("0.1.0", &info(Some("abc1234"), Some(0), false)),
            "0.1.0 (release, abc1234)"
        );
    }

    #[test]
    fn dev_with_count() {
        assert_eq!(
            format_version("0.1.0", &info(Some("abc1234"), Some(3), false)),
            "0.1.0-dev (3 commits past v0.1.0, abc1234)"
        );
        assert_eq!(
            format_version("0.1.0", &info(Some("abc1234"), Some(1), false)),
            "0.1.0-dev (1 commit past v0.1.0, abc1234)"
        );
    }

    #[test]
    fn dev_without_tag() {
        assert_eq!(
            format_version("0.1.0", &info(Some("abc1234"), None, false)),
            "0.1.0-dev (abc1234)"
        );
    }

    #[test]
    fn dirty() {
        assert_eq!(
            format_version("0.1.0", &info(Some("abc1234"), Some(2), true)),
            "0.1.0-dev (2 commits past v0.1.0, abc1234, dirty)"
        );
        assert_eq!(
            format_version("0.1.0", &info(Some("abc1234"), None, true)),
            "0.1.0-dev (abc1234, dirty)"
        );
        // Dirty at the tag is no release.
        assert_eq!(
            format_version("0.1.0", &info(Some("abc1234"), Some(0), true)),
            "0.1.0-dev (at v0.1.0, abc1234, dirty)"
        );
    }

    #[test]
    fn unknown_commit() {
        assert_eq!(
            format_version("0.1.0", &info(None, None, false)),
            "0.1.0-dev (unknown)"
        );
    }
}
