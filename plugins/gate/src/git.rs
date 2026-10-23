//! git calls of the gate, through the real git (`VEGA_GATE_GIT`).

use std::path::{Path, PathBuf};

use crate::{config, run, run_ok, tool};

/// `git <args>` in `dir`; Err with git's first stderr line.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    run_ok(&tool("git"), args, Some(dir), None)
}

/// True when `git <args>` exits 0.
pub fn git_ok(dir: &Path, args: &[&str]) -> bool {
    run(&tool("git"), args, Some(dir), None).is_ok_and(|o| o.ok)
}

/// The commit `rev` names, or None.
pub fn commit(dir: &Path, rev: &str) -> Option<String> {
    git(
        dir,
        &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")],
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

/// The object id of `rev` (a blob or tree path at a commit), or None.
pub fn object(dir: &Path, rev: &str) -> Option<String> {
    git(dir, &["rev-parse", "--verify", "-q", rev])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn toplevel(dir: &Path) -> Result<PathBuf, String> {
    git(dir, &["rev-parse", "--show-toplevel"]).map(|s| PathBuf::from(s.trim()))
}

/// The main checkout (parent of the common git dir); a worktree's own top
/// level when that fails.
pub fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let common = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    Path::new(common.trim()).parent().map(Path::to_path_buf)
}

/// A beads repo: `.beads` in this checkout or the main checkout.
pub fn is_beads_repo(dir: &Path) -> bool {
    toplevel(dir).is_ok_and(|t| t.join(".beads").is_dir())
        || main_checkout(dir).is_some_and(|m| m.join(".beads").is_dir())
}

pub fn current_branch(dir: &Path) -> Option<String> {
    git(dir, &["symbolic-ref", "-q", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The review target of a checkout without a push: the branch's upstream
/// when it is a remote-tracking ref, else `origin/HEAD`, else `origin/main`.
pub fn default_target(dir: &Path) -> Result<String, String> {
    let upstream = git(
        dir,
        &[
            "rev-parse",
            "--symbolic-full-name",
            "--abbrev-ref",
            "@{upstream}",
        ],
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|u| {
        !u.is_empty() && object(dir, &format!("refs/remotes/{u}")).is_some() && u.contains('/')
    });
    upstream
        .or_else(|| {
            ["origin/HEAD", "origin/main"]
                .iter()
                .find(|r| commit(dir, &format!("refs/remotes/{r}")).is_some())
                .map(|r| r.to_string())
        })
        .ok_or_else(|| "no target: no upstream, origin/HEAD or origin/main".into())
}

/// The gate config at `target`: Ok(None) when the file is absent there.
/// Returns its blob id and parsed content.
pub fn config_at(dir: &Path, target: &str) -> Result<Option<(String, config::Config)>, String> {
    let spec = format!("{target}:{}", config::PATH);
    let Some(blob) = object(dir, &spec) else {
        return Ok(None);
    };
    let text = git(dir, &["show", &spec])?;
    Ok(Some((blob, config::parse(&text)?)))
}
