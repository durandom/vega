//! Git sync (docs/design/git-sync.md): vegad keeps the git checkouts of the
//! projects it knows fresh, next to their beads sync.
//!
//! - Every [`FETCH_EVERY`]: `git fetch origin` in the common dir. Touches no
//!   working tree.
//! - At most every [`FF_EVERY`], when the default branch is behind its
//!   upstream: fast-forward the checkout that has it checked out, only when
//!   its tracked files are clean and the branch has no commits of its own;
//!   else skip and record why.
//! - Every other worktree: how many upstream commits its HEAD lacks. vegad
//!   never rebases or merges them; the bd shim tells their agents.
//!
//! The projects are the bd registry's ([`crate::store::projects`]) plus the
//! local locations the caller adds (UAC claims). State is keyed by project
//! ID and canonical common directory under `git/v2`, one writer (vegad).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::bd::{last_line, run};
use crate::store;

pub const FETCH_EVERY: i64 = 300;
pub const FF_EVERY: i64 = 900;

const FETCH_LIMIT: Duration = Duration::from_secs(120);
const GIT_LIMIT: Duration = Duration::from_secs(30);

/// A project: its key and its git common dir.
#[derive(Clone, Debug, PartialEq)]
pub struct Repo {
    pub key: String,
    pub common: PathBuf,
}

/// Resolve an explicitly local common-dir path. Never pass a remote ID here.
pub fn repo(local_path: &str) -> Option<Repo> {
    let path = match local_path.strip_prefix("~/") {
        Some(rest) => Path::new(&std::env::var_os("HOME")?).join(rest),
        None => PathBuf::from(local_path),
    };
    match vega_herdr::project::resolve(&path, &vega_herdr::state::post_host()) {
        Ok(p) => Some(Repo {
            key: p.id.to_string(),
            common: p.location.common_dir,
        }),
        Err(e) => {
            eprintln!("git sync: {}: {}", e.code, e.message);
            None
        }
    }
}

/// Known local v2 UAC claims, retaining their expected ID. Never replace a
/// claim's identity by resolving a changed URL without a journaled rekey.
pub fn claimed() -> Vec<Repo> {
    vega_herdr::state::project_uac_locations()
        .into_iter()
        .map(|(key, location)| Repo {
            key,
            common: location.common_dir,
        })
        .collect()
}

impl Repo {
    pub fn validate_location(&self) -> Result<(), vega_herdr::Failure> {
        let p = vega_herdr::project::resolve(&self.common, &vega_herdr::state::post_host())?;
        if p.id.as_str() == self.key && p.location.common_dir == self.common {
            Ok(())
        } else {
            Err(vega_herdr::project::identity_changed(
                "A sync location",
                &self.key,
                &p,
            ))
        }
    }
}

/// All recorded local clones, including failures, for read-only status.
pub fn known_repos(extra: impl IntoIterator<Item = Repo>) -> Vec<Repo> {
    let mut repos: Vec<Repo> = store::projects()
        .into_iter()
        .map(|p| Repo {
            key: p.key,
            common: p.common,
        })
        .collect();
    repos.extend(extra);
    repos.sort_by(|a, b| (&a.key, &a.common).cmp(&(&b.key, &b.common)));
    repos.dedup_by(|a, b| a.key == b.key && a.common == b.common);
    repos
}

/// Worker locations: each clone must still resolve to its recorded ID.
pub fn repos(extra: impl IntoIterator<Item = Repo>) -> Vec<Repo> {
    known_repos(extra)
        .into_iter()
        .filter(|r| match r.validate_location() {
            Ok(()) => {
                vega_herdr::project::registered_diagnostic("git sync", &r.key, &r.common, None);
                true
            }
            Err(e) => {
                if let Some(message) = vega_herdr::project::registered_diagnostic(
                    "git sync",
                    &r.key,
                    &r.common,
                    Some(&e),
                ) {
                    eprintln!("git sync: {message}");
                }
                false
            }
        })
        .collect()
}

/// A worktree that is behind the upstream of the default branch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lag {
    pub dir: PathBuf,
    pub branch: Option<String>,
    pub behind: u64,
}

/// vegad's git state of a project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GitState {
    /// Last fetch attempt, last successful fetch.
    #[serde(default)]
    pub fetch_tried: i64,
    #[serde(default)]
    pub fetched: i64,
    /// Last fast-forward attempt and success.
    #[serde(default)]
    pub ff_tried: i64,
    #[serde(default)]
    pub fast_forwarded: i64,
    /// The default branch (`main`); its upstream is `origin/<branch>`.
    pub branch: Option<String>,
    /// The checkout that has the default branch checked out.
    pub main_dir: Option<PathBuf>,
    /// The default branch against its upstream.
    pub behind: Option<u64>,
    pub ahead: Option<u64>,
    /// Why the last fast-forward did not happen (dirty, diverged, ...).
    pub skip: Option<String>,
    /// A fetch or git error: retried on the next interval.
    pub error: Option<String>,
    /// No `origin` remote: a local-only repo, nothing to sync.
    #[serde(default)]
    pub local: bool,
    /// The other worktrees, with how far each is behind the upstream.
    #[serde(default)]
    pub worktrees: Vec<Lag>,
}

impl GitState {
    /// One word for tables: `local` (no origin), `error`, `skipped`,
    /// `pending` (not fetched yet), `behind` (a fast-forward is due), `ok`.
    pub fn label(&self) -> &'static str {
        if self.local {
            "local"
        } else if self.error.is_some() {
            "error"
        } else if self.skip.is_some() {
            "skipped"
        } else if self.fetched == 0 {
            "pending"
        } else if self.behind.unwrap_or(0) > 0 {
            "behind"
        } else {
            "ok"
        }
    }

    fn problem(&self) -> Option<String> {
        self.error.clone().or_else(|| self.skip.clone())
    }
}

pub fn state(key: &str, common: &Path) -> GitState {
    store::load("git", key, common).unwrap_or_default()
}

pub fn save_state(key: &str, common: &Path, s: &GitState) -> bool {
    store::save("git", key, common, s)
}

/// The columns `vega bd status` and `vega uac status` print per project.
pub const COLUMNS: &str = "project,location,branch,behind,ahead,fetched,state";

/// The cells of [`COLUMNS`] (unquoted), and the problem to show below the
/// table: the error, or why the fast-forward was skipped.
pub fn row(repo: &Repo, s: &GitState, now: i64) -> ([String; 7], Option<String>) {
    let n = |v: Option<u64>| v.map_or("-".to_string(), |v| v.to_string());
    let fetched = if s.fetched > 0 {
        crate::ago(now - s.fetched)
    } else {
        "never".into()
    };
    let failure = repo.validate_location().err();
    let problem = store::location_problem(s.problem(), failure.as_ref());
    (
        [
            repo.key.clone(),
            repo.common.display().to_string(),
            s.branch.clone().unwrap_or_else(|| "-".into()),
            n(s.behind),
            n(s.ahead),
            fetched,
            if failure.is_some() {
                "error"
            } else {
                s.label()
            }
            .to_string(),
        ],
        problem,
    )
}

/// The stderr line the bd shim prints in checkout `top` (canonical), if
/// any: the main checkout behind with its fast-forward skipped, or a
/// worktree behind the upstream.
pub fn hint(key: &str, s: &GitState, top: &Path, now: i64) -> Option<String> {
    let branch = s.branch.as_deref()?;
    let fetched = crate::ago(now - s.fetched);
    let same = |d: &Path| canonical(d) == top;
    if s.main_dir.as_deref().is_some_and(same) {
        let behind = s.behind.filter(|b| *b > 0)?;
        let why = s.skip.as_deref()?;
        return Some(format!(
            "vega: {branch} here is {behind} behind origin/{branch} (fetched {fetched}); vegad's fast-forward skipped: {why}. State: vega bd status"
        ));
    }
    let lag = s.worktrees.iter().find(|l| same(&l.dir))?;
    (lag.behind > 0).then(|| {
        format!(
            "vega: this worktree is {} commit(s) behind origin/{branch} (fetched {fetched}, {key}); vegad never rebases worktrees, the UAC decides. State: vega bd status",
            lag.behind
        )
    })
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// One entry of `git worktree list --porcelain`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub dir: PathBuf,
    pub head: Option<String>,
    /// The checked-out branch (`main`); `None` when detached.
    pub branch: Option<String>,
    pub bare: bool,
}

pub fn parse_worktrees(text: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for line in text.lines() {
        if let Some(dir) = line.strip_prefix("worktree ") {
            out.push(Worktree {
                dir: dir.into(),
                ..Worktree::default()
            });
            continue;
        }
        let Some(w) = out.last_mut() else { continue };
        if let Some(h) = line.strip_prefix("HEAD ") {
            w.head = Some(h.into());
        } else if let Some(b) = line.strip_prefix("branch ") {
            w.branch = Some(b.trim_start_matches("refs/heads/").into());
        } else if line == "bare" {
            w.bare = true;
        }
    }
    out
}

/// The git operations a step needs; [`Git`] is the real one.
pub trait Ops {
    /// Whether the repo has an `origin` remote.
    fn has_origin(&mut self, r: &Repo) -> bool;
    fn fetch(&mut self, r: &Repo) -> Result<(), String>;
    /// The default branch: `origin/HEAD`'s target, else `main`.
    fn default_branch(&mut self, r: &Repo) -> Result<String, String>;
    fn worktrees(&mut self, r: &Repo) -> Result<Vec<Worktree>, String>;
    /// `git rev-list --count <range>` in the common dir.
    fn count(&mut self, r: &Repo, range: &str) -> Result<u64, String>;
    /// Tracked files changed in checkout `dir`.
    fn dirty(&mut self, dir: &Path) -> Result<bool, String>;
    /// `git merge --ff-only <upstream>` in checkout `dir`.
    fn ff(&mut self, dir: &Path, upstream: &str) -> Result<(), String>;
}

/// Runs git with no prompt (a fetch that wants credentials fails) and the C
/// locale.
pub struct Git;

fn git(at: &[&str], args: &[&str], limit: Duration) -> Result<String, String> {
    let mut c = Command::new("git");
    c.args(at)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    let r = run(c, limit);
    if r.timed_out {
        Err(format!("git {} timed out", args[0]))
    } else if r.ok {
        Ok(r.out)
    } else {
        Err(last_line(&r.out))
    }
}

fn in_common(r: &Repo) -> String {
    format!("--git-dir={}", r.common.display())
}

impl Ops for Git {
    fn has_origin(&mut self, r: &Repo) -> bool {
        git(
            &[&in_common(r)],
            &["remote", "get-url", "origin"],
            FETCH_LIMIT,
        )
        .is_ok()
    }

    fn fetch(&mut self, r: &Repo) -> Result<(), String> {
        git(
            &[&in_common(r)],
            &["fetch", "--quiet", "--no-recurse-submodules", "origin"],
            FETCH_LIMIT,
        )
        .map(|_| ())
    }

    fn default_branch(&mut self, r: &Repo) -> Result<String, String> {
        let at = in_common(r);
        let head = git(
            &[&at],
            &["symbolic-ref", "--short", "-q", "refs/remotes/origin/HEAD"],
            GIT_LIMIT,
        );
        if let Some(b) = head.ok().and_then(|h| {
            h.lines()
                .next()
                .and_then(|l| l.strip_prefix("origin/"))
                .map(String::from)
        }) {
            return Ok(b);
        }
        git(
            &[&at],
            &["rev-parse", "-q", "--verify", "refs/remotes/origin/main"],
            GIT_LIMIT,
        )
        .map(|_| "main".to_string())
        .map_err(|_| "no origin/HEAD and no origin/main".to_string())
    }

    fn worktrees(&mut self, r: &Repo) -> Result<Vec<Worktree>, String> {
        git(
            &[&in_common(r)],
            &["worktree", "list", "--porcelain"],
            GIT_LIMIT,
        )
        .map(|t| parse_worktrees(&t))
    }

    fn count(&mut self, r: &Repo, range: &str) -> Result<u64, String> {
        let out = git(&[&in_common(r)], &["rev-list", "--count", range], GIT_LIMIT)?;
        out.lines()
            .next()
            .and_then(|l| l.trim().parse().ok())
            .ok_or_else(|| format!("rev-list {range}: {out}"))
    }

    fn dirty(&mut self, dir: &Path) -> Result<bool, String> {
        let d = dir.display().to_string();
        git(
            &["-C", &d],
            &["status", "--porcelain", "--untracked-files=no"],
            GIT_LIMIT,
        )
        .map(|o| !o.trim().is_empty())
    }

    fn ff(&mut self, dir: &Path, upstream: &str) -> Result<(), String> {
        let d = dir.display().to_string();
        git(
            &["-C", &d],
            &["merge", "--ff-only", "--quiet", upstream],
            GIT_LIMIT,
        )
        .map(|_| ())
    }
}

/// One pass over the repos; reads and writes their state files. Log lines.
pub fn cycle(now: i64, repos: &[Repo], ops: &mut dyn Ops) -> Vec<String> {
    let mut log = Vec::new();
    for r in repos {
        let mut s = state(&r.key, &r.common);
        let before = s.clone();
        step(now, r, &mut s, ops, &mut log);
        if s != before {
            save_state(&r.key, &r.common, &s);
        }
    }
    log
}

/// One repo. Pure apart from `ops`, for the tests. Logs a fast-forward and
/// every change of state or problem.
pub fn step(now: i64, r: &Repo, s: &mut GitState, ops: &mut dyn Ops, log: &mut Vec<String>) {
    if now - s.fetch_tried < FETCH_EVERY {
        return;
    }
    let said = (s.label(), s.problem());
    s.fetch_tried = now;
    if let Err(e) = refresh(now, r, s, ops, log) {
        s.error = Some(e);
    }
    if (s.label(), s.problem()) != said {
        let problem = s.problem().map(|p| format!(": {p}")).unwrap_or_default();
        log.push(format!("git sync: {} {}{problem}", r.key, s.label()));
    }
}

fn refresh(
    now: i64,
    r: &Repo,
    s: &mut GitState,
    ops: &mut dyn Ops,
    log: &mut Vec<String>,
) -> Result<(), String> {
    s.local = !ops.has_origin(r);
    if s.local {
        s.error = None;
        s.skip = None;
        return Ok(());
    }
    ops.fetch(r).map_err(|e| format!("fetch: {e}"))?;
    s.fetched = now;
    s.error = None;
    let branch = ops.default_branch(r)?;
    let up = format!("origin/{branch}");
    let trees = ops
        .worktrees(r)
        .map_err(|e| format!("worktree list: {e}"))?;
    s.worktrees = trees
        .iter()
        .filter(|w| !w.bare && w.branch.as_deref() != Some(&branch))
        .filter_map(|w| {
            let head = w.head.as_deref()?;
            Some(Lag {
                dir: w.dir.clone(),
                branch: w.branch.clone(),
                behind: ops.count(r, &format!("{head}..{up}")).ok()?,
            })
        })
        .collect();
    s.branch = Some(branch.clone());
    let Some(main) = trees
        .iter()
        .find(|w| !w.bare && w.branch.as_deref() == Some(&branch))
    else {
        s.main_dir = None;
        s.behind = None;
        s.ahead = None;
        s.skip = Some(format!("no checkout has {branch} checked out"));
        return Ok(());
    };
    s.main_dir = Some(main.dir.clone());
    let local = format!("refs/heads/{branch}");
    let behind = ops.count(r, &format!("{local}..{up}"))?;
    let ahead = ops.count(r, &format!("{up}..{local}"))?;
    s.behind = Some(behind);
    s.ahead = Some(ahead);
    if behind == 0 {
        s.skip = None;
        return Ok(());
    }
    if now - s.ff_tried < FF_EVERY {
        return Ok(());
    }
    s.ff_tried = now;
    s.skip = if ahead > 0 {
        Some(format!("diverged: {ahead} local commit(s) not on {up}"))
    } else {
        match ops.dirty(&main.dir) {
            Ok(true) => Some("tracked files changed".into()),
            Err(e) => Some(format!("status: {e}")),
            Ok(false) => match ops.ff(&main.dir, &up) {
                Ok(()) => {
                    s.fast_forwarded = now;
                    s.behind = Some(0);
                    log.push(format!(
                        "git sync: {} fast-forwarded {branch} by {behind} in {}",
                        r.key,
                        main.dir.display()
                    ));
                    None
                }
                Err(e) => Some(format!("fast-forward failed: {e}")),
            },
        }
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        fetch: Option<String>,
        trees: Vec<Worktree>,
        behind: u64,
        ahead: u64,
        lag: u64,
        dirty: bool,
        no_origin: bool,
        calls: Vec<String>,
    }

    impl Ops for Fake {
        fn has_origin(&mut self, _: &Repo) -> bool {
            !self.no_origin
        }
        fn fetch(&mut self, _: &Repo) -> Result<(), String> {
            self.calls.push("fetch".into());
            self.fetch.clone().map_or(Ok(()), Err)
        }
        fn default_branch(&mut self, _: &Repo) -> Result<String, String> {
            Ok("main".into())
        }
        fn worktrees(&mut self, _: &Repo) -> Result<Vec<Worktree>, String> {
            Ok(self.trees.clone())
        }
        fn count(&mut self, _: &Repo, range: &str) -> Result<u64, String> {
            Ok(match range {
                "refs/heads/main..origin/main" => self.behind,
                "origin/main..refs/heads/main" => self.ahead,
                _ => self.lag,
            })
        }
        fn dirty(&mut self, _: &Path) -> Result<bool, String> {
            Ok(self.dirty)
        }
        fn ff(&mut self, _: &Path, up: &str) -> Result<(), String> {
            self.calls.push(format!("ff {up}"));
            self.behind = 0;
            Ok(())
        }
    }

    fn repo() -> Repo {
        Repo {
            key: "~/src/x/.git".into(),
            common: "/x/.git".into(),
        }
    }

    fn tree(dir: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            dir: dir.into(),
            head: Some("abc".into()),
            branch: branch.map(String::from),
            bare: false,
        }
    }

    fn fake(behind: u64) -> Fake {
        Fake {
            trees: vec![tree("/x", Some("main")), tree("/x.fix", Some("fix"))],
            behind,
            lag: 3,
            ..Fake::default()
        }
    }

    #[test]
    fn a_repo_without_origin_is_local_not_an_error() {
        let mut s = GitState {
            error: Some("fetch: earlier".into()),
            ..GitState::default()
        };
        let mut f = Fake {
            no_origin: true,
            ..fake(4)
        };
        let mut log = Vec::new();
        step(1000, &repo(), &mut s, &mut f, &mut log);
        assert!(f.calls.is_empty());
        assert_eq!((s.label(), s.error.clone()), ("local", None));
    }

    #[test]
    fn fetches_on_the_interval_and_fast_forwards_a_clean_main() {
        let mut s = GitState::default();
        let mut f = fake(4);
        let mut log = Vec::new();
        step(1000, &repo(), &mut s, &mut f, &mut log);
        assert_eq!(f.calls, ["fetch", "ff origin/main"]);
        assert_eq!(
            (s.behind, s.fast_forwarded, s.label()),
            (Some(0), 1000, "ok")
        );
        assert_eq!(s.main_dir, Some("/x".into()));
        assert_eq!(
            s.worktrees,
            [Lag {
                dir: "/x.fix".into(),
                branch: Some("fix".into()),
                behind: 3
            }]
        );
        assert!(log[0].contains("fast-forwarded main by 4"), "{log:?}");
        let mut f = fake(4);
        step(1000 + FETCH_EVERY - 1, &repo(), &mut s, &mut f, &mut log);
        assert!(f.calls.is_empty(), "quiet between fetches");
    }

    #[test]
    fn dirty_or_diverged_skips_and_waits_for_the_ff_interval() {
        let mut s = GitState::default();
        let mut f = Fake {
            dirty: true,
            ..fake(2)
        };
        let mut log = Vec::new();
        step(1000, &repo(), &mut s, &mut f, &mut log);
        assert_eq!(s.label(), "skipped");
        assert_eq!(s.skip.as_deref(), Some("tracked files changed"));
        assert_eq!(log.len(), 1, "a change of state is logged");
        // Cleaned up, but the next fast-forward is not due yet: skip stays.
        let mut f = fake(2);
        step(1000 + FETCH_EVERY, &repo(), &mut s, &mut f, &mut log);
        assert_eq!(f.calls, ["fetch"]);
        assert!(s.skip.is_some());
        assert_eq!(log.len(), 1, "unchanged state is not logged again");
        step(1000 + FF_EVERY, &repo(), &mut s, &mut f, &mut log);
        assert_eq!(f.calls, ["fetch", "fetch", "ff origin/main"]);
        assert_eq!(s.label(), "ok");
        // Diverged: never merged.
        let mut s = GitState::default();
        let mut f = Fake {
            ahead: 1,
            ..fake(2)
        };
        step(1000, &repo(), &mut s, &mut f, &mut Vec::new());
        assert_eq!(f.calls, ["fetch"]);
        assert!(s.skip.unwrap().starts_with("diverged"));
    }

    #[test]
    fn no_main_checkout_and_fetch_errors_are_reported() {
        let mut s = GitState::default();
        let mut f = Fake {
            trees: vec![tree("/x", Some("feature"))],
            behind: 5,
            ..Fake::default()
        };
        step(1000, &repo(), &mut s, &mut f, &mut Vec::new());
        assert_eq!(f.calls, ["fetch"]);
        assert_eq!(s.skip.as_deref(), Some("no checkout has main checked out"));
        let mut f = Fake {
            fetch: Some("Could not resolve host".into()),
            ..fake(0)
        };
        step(2000, &repo(), &mut s, &mut f, &mut Vec::new());
        assert_eq!(s.label(), "error");
        assert_eq!(s.fetched, 1000);
        // The next fetch succeeds and clears it.
        step(
            2000 + FETCH_EVERY,
            &repo(),
            &mut s,
            &mut fake(0),
            &mut Vec::new(),
        );
        assert_eq!(s.label(), "ok");
    }

    #[test]
    fn parses_worktree_porcelain() {
        let text = "worktree /r/.bare\nbare\n\nworktree /r/main\nHEAD aaa\nbranch refs/heads/main\n\nworktree /r/x\nHEAD bbb\ndetached\nprunable gitdir file points to non-existent location\n";
        let t = parse_worktrees(text);
        assert_eq!(t.len(), 3);
        assert!(t[0].bare);
        assert_eq!(t[1].branch.as_deref(), Some("main"));
        assert_eq!(
            (t[2].head.as_deref(), t[2].branch.as_deref()),
            (Some("bbb"), None)
        );
    }

    #[test]
    fn hints_name_the_skip_or_the_lag() {
        let s = GitState {
            fetched: 940,
            branch: Some("main".into()),
            main_dir: Some("/nonexistent/x".into()),
            behind: Some(7),
            skip: Some("tracked files changed".into()),
            worktrees: vec![Lag {
                dir: "/nonexistent/x.fix".into(),
                branch: Some("fix".into()),
                behind: 12,
            }],
            ..GitState::default()
        };
        let main = hint("k", &s, Path::new("/nonexistent/x"), 1000).unwrap();
        assert!(main.contains("7 behind origin/main") && main.contains("tracked files changed"));
        let wt = hint("k", &s, Path::new("/nonexistent/x.fix"), 1000).unwrap();
        assert!(
            wt.contains("12 commit(s) behind") && wt.contains("1m ago"),
            "{wt}"
        );
        assert_eq!(hint("k", &s, Path::new("/elsewhere"), 1000), None);
        let level = GitState {
            skip: None,
            ..s.clone()
        };
        assert_eq!(hint("k", &level, Path::new("/nonexistent/x"), 1000), None);
    }

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// Real git against temp repos: an origin, a clone with a sentinel
    /// worktree, and a second clone that pushes.
    #[test]
    fn real_git_fetches_fast_forwards_and_counts_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        let origin = t.join("origin.git");
        sh(
            t,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        sh(t, &["clone", "-q", origin.to_str().unwrap(), "pusher"]);
        let pusher = t.join("pusher");
        sh(&pusher, &["commit", "-q", "--allow-empty", "-m", "one"]);
        sh(&pusher, &["push", "-q", "origin", "HEAD:main"]);
        sh(t, &["clone", "-q", origin.to_str().unwrap(), "main"]);
        let main = t.join("main");
        sh(
            &main,
            &["worktree", "add", "-q", "-b", "fix", "../main.fix"],
        );
        for m in ["two", "three"] {
            sh(&pusher, &["commit", "-q", "--allow-empty", "-m", m]);
        }
        sh(&pusher, &["push", "-q", "origin", "HEAD:main"]);
        let r = Repo {
            key: "~/main/.git".into(),
            common: main.join(".git"),
        };
        // Dirty: skipped, nothing moved.
        std::fs::write(main.join("f"), "x").unwrap();
        sh(&main, &["add", "f"]);
        let mut s = GitState::default();
        step(1000, &r, &mut s, &mut Git, &mut Vec::new());
        assert_eq!((s.behind, s.label()), (Some(2), "skipped"), "{s:?}");
        assert_eq!(s.worktrees.len(), 1);
        assert_eq!(s.worktrees[0].behind, 2);
        // Clean: fast-forwarded on the next ff interval.
        sh(&main, &["reset", "-q", "--hard"]);
        step(1000 + FF_EVERY, &r, &mut s, &mut Git, &mut Vec::new());
        assert_eq!((s.behind, s.label()), (Some(0), "ok"), "{s:?}");
        let log = Command::new("git")
            .args(["-C", main.to_str().unwrap(), "log", "--format=%s", "-1"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "three");
        // The worktree is untouched: still 2 behind.
        assert_eq!(s.worktrees[0].behind, 2);
    }
    #[test]
    fn local_discovery_keeps_clones_but_deduplicates_linked_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a", "b"] {
            let path = tmp.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            sh(&path, &["init", "-q"]);
            sh(
                &path,
                &[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/team/repo.git",
                ],
            );
        }
        let a = tmp.path().join("a");
        sh(&a, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        sh(&a, &["worktree", "add", "-q", "-b", "linked", "../linked"]);
        let paths = [
            a.clone(),
            a.join(".git"),
            tmp.path().join("linked"),
            tmp.path().join("b"),
        ];
        let discovered: Vec<_> = paths
            .iter()
            .map(|p| super::repo(p.to_str().unwrap()).unwrap())
            .collect();
        assert!(discovered.iter().all(|r| r.key == "github.com/team/repo"));
        assert_eq!(discovered[0].common, discovered[2].common);
        assert_ne!(discovered[0].common, discovered[3].common);
        let all = repos(discovered);
        assert_eq!(
            all.iter()
                .filter(|r| r.key == "github.com/team/repo")
                .count(),
            2
        );
    }
    #[test]
    fn a_stale_id_beside_its_re_registered_id_logs_once() {
        const MODE: &str = "VEGA_GIT_REPOS_STALE_FIXTURE";
        if std::env::var_os(MODE).is_some() {
            let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("a");
            std::fs::create_dir(&dir).unwrap();
            sh(&dir, &["init", "-q"]);
            let stale = super::repo(dir.to_str().unwrap()).unwrap();
            sh(
                &dir,
                &[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/team/repo.git",
                ],
            );
            let current = super::repo(dir.to_str().unwrap()).unwrap();
            assert_ne!(stale.key, current.key);
            for _ in 0..3 {
                assert_eq!(
                    repos([stale.clone(), current.clone()]),
                    vec![current.clone()]
                );
            }
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "git::tests::a_stale_id_beside_its_re_registered_id_logs_once",
                "--nocapture",
            ])
            .env(MODE, "1")
            .env("HOME", &root)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("VEGA_POST_HOST", "fixture")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{}{stderr}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(stderr.matches("git sync:").count(), 1, "{stderr}");
        assert!(stderr.contains("now github.com/team/repo"), "{stderr}");
        assert!(stderr.contains("vega bd init"), "{stderr}");
    }
    #[test]
    fn failed_claimed_and_provisioned_clones_are_dropped_once_without_hiding_siblings() {
        const MODE: &str = "VEGA_GIT_REPOS_GUARD_FIXTURE";
        if let Ok(mode) = std::env::var(MODE) {
            let root = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
            let mut discovered = Vec::new();
            for name in ["a", "b"] {
                let dir = root.join(name);
                std::fs::create_dir(&dir).unwrap();
                sh(&dir, &["init", "-q"]);
                sh(
                    &dir,
                    &[
                        "remote",
                        "add",
                        "origin",
                        "https://github.com/team/repo.git",
                    ],
                );
                discovered.push(super::repo(dir.to_str().unwrap()).unwrap());
            }
            let invalid = discovered[0].clone();
            let sibling = discovered[1].clone();
            let provisioned = mode.starts_with("provisioned");
            if provisioned {
                let dir = root.join("a");
                assert!(store::save_project(&store::Project {
                    key: invalid.key.clone(),
                    common: invalid.common.clone(),
                    dir: dir.clone(),
                    database: "DB".into(),
                    remote: "http://remote/DB".into(),
                    bd: "/nonexistent/bd".into(),
                    data: dir.join(".beads/dolt"),
                    since: 1,
                    resumed: 0
                }));
            }
            if mode.ends_with("config") {
                std::fs::write(invalid.common.join("config"), "[broken configuration").unwrap();
            } else {
                sh(
                    &root.join("a"),
                    &[
                        "remote",
                        "set-url",
                        "origin",
                        "https://github.com/team/fork.git",
                    ],
                );
            }
            let extra = if provisioned {
                vec![sibling.clone()]
            } else {
                discovered
            };
            assert_eq!(known_repos(extra.clone()).len(), 2);
            let prior = GitState {
                error: Some("previous fetch failure".into()),
                fetched: 17,
                ..GitState::default()
            };
            let (cells, problem) = row(&invalid, &prior, 1000);
            assert_eq!(cells[6], "error");
            let problem = problem.unwrap();
            assert!(problem.contains("previous fetch failure"));
            assert!(problem.contains(if mode.ends_with("config") {
                "project_git_failed"
            } else {
                "project_identity_changed"
            }));
            assert!(problem.contains(&invalid.validate_location().unwrap_err().hint));
            assert_eq!(repos(extra.clone()), vec![sibling.clone()]);
            assert_eq!(repos(extra), vec![sibling]);
            std::fs::write(root.join("returned"), "repos returned without exiting").unwrap();
            return;
        }
        for mode in [
            "claimed-config",
            "claimed-changed",
            "provisioned-config",
            "provisioned-changed",
        ] {
            let root = tempfile::tempdir().unwrap();
            let root = root.path().canonicalize().unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "git::tests::failed_claimed_and_provisioned_clones_are_dropped_once_without_hiding_siblings", "--nocapture"])
                .env(MODE, mode).env("HOME", &root).env("XDG_STATE_HOME", root.join("state"))
                .env("VEGA_POST_HOST", "fixture").env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                root.join("returned").is_file(),
                "repos must return rather than exiting the process"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(stderr.matches("git sync:").count(), 1, "{mode}: {stderr}");
            assert!(stderr.contains(if mode.ends_with("config") {
                "project_git_failed"
            } else {
                "project_identity_changed"
            }));
        }
    }
}
