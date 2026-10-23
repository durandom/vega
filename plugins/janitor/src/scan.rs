//! Finding checkouts and asking git about them (docs/design/janitor.md, Scan).
//!
//! Read-only: only read-only git commands, always with `GIT_OPTIONAL_LOCKS=0`
//! so `git status` does not rewrite the index; never a fetch.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Checkouts deeper than this below a root are not found (measured on
/// proteus: none are, and walking deeper costs tens of seconds).
pub const MAX_DEPTH: usize = 5;

/// Longer than this, `git status` is cut off and `dirty` is unknown.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(3);

/// Directories never entered: build output, dependencies, caches.
const SKIP: [&str; 16] = [
    "node_modules",
    "target",
    ".venv",
    "venv",
    ".terraform",
    ".build",
    "vendor",
    "dist",
    "build",
    "__pycache__",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".gradle",
    ".next",
    ".git",
];

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Clone,
    Worktree,
    Bare,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Clone => "clone",
            Kind::Worktree => "worktree",
            Kind::Bare => "bare",
        }
    }
}

/// One checkout (or bare repo) and what git said about it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
    /// This checkout's git dir (`.git`, `.git/worktrees/<id>`, the bare dir).
    pub gitdir: String,
    /// The repo's common git dir, shared by its worktrees.
    pub common: String,
    /// A worktree's main checkout (or bare repo).
    pub main: Option<String>,
    /// Normalised URL of `origin` (else the first remote).
    pub remote: Option<String>,
    /// Has an `upstream` remote: a fork (as the earlier Python janitor judged it).
    pub fork: bool,
    /// `None`: detached HEAD.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: u32,
    /// `git status` outlived `STATUS_TIMEOUT`: `dirty` is unknown. Usually a
    /// stale index: read-only status rehashes every file on each call.
    pub slow: bool,
    pub last_commit: Option<i64>,
    /// The commit HEAD points to: a new commit moves it even within the
    /// second `last_commit` resolves (History).
    #[serde(default)]
    pub head: Option<String>,
    /// Latest of last commit and the mtimes of HEAD and index.
    pub touched: i64,
    /// The branch the repo merges into (`main`, `master`, ...).
    pub default_branch: Option<String>,
    /// The current branch is merged into the default branch.
    pub merged: bool,
    /// The current branch's upstream is gone.
    pub gone: bool,
    /// For a clone or bare repo: local branches merged into the default
    /// branch, without the default branch and branches checked out anywhere.
    pub merged_branches: Vec<String>,
    /// A worktree whose directory is gone (`git worktree prune` removes it).
    pub prunable: bool,
    /// mtimes that change when git state changes (see `fingerprint`).
    pub fp: Vec<i64>,
    /// Agent skills installed in or owned by this checkout.
    #[serde(default)]
    pub skills: Vec<crate::skills::Skill>,
    /// Another host's checkout (a `peer` row, Cross-host); `None`: this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// What a directory is, if it is a checkout.
fn classify(dir: &Path) -> Option<(Kind, PathBuf)> {
    let dotgit = dir.join(".git");
    match fs::symlink_metadata(&dotgit) {
        Ok(m) if m.is_dir() => return Some((Kind::Clone, dotgit)),
        Ok(m) if m.is_file() => {
            let gitdir = read_gitdir_file(&dotgit)?;
            let s = gitdir.to_string_lossy();
            // Submodules (`.git/modules/...`) belong to their parent.
            return s
                .contains("/worktrees/")
                .then_some((Kind::Worktree, gitdir));
        }
        _ => {}
    }
    is_bare(dir).then(|| (Kind::Bare, dir.to_path_buf()))
}

fn is_bare(dir: &Path) -> bool {
    dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

/// The `gitdir:` of a worktree's `.git` file, absolute.
fn read_gitdir_file(dotgit: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(dotgit).ok()?;
    let rel = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let p = Path::new(rel);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        dotgit.parent()?.join(p)
    })
}

/// Directories found under `roots`: (path, kind, gitdir).
pub fn walk(roots: &[PathBuf]) -> Vec<Found> {
    // Expand the first levels serially, then spread the subtrees over threads.
    let found = Mutex::new(Vec::new());
    let mut frontier: Vec<(PathBuf, usize)> = Vec::new();
    for root in roots {
        if let Some((k, g)) = classify(root) {
            found.lock().unwrap().push((root.clone(), k, g));
            if k == Kind::Bare {
                continue;
            }
        }
        frontier.push((root.clone(), 0));
    }
    for _ in 0..2 {
        let mut next = Vec::new();
        for (dir, depth) in frontier {
            next.extend(children(&dir, depth, &found));
        }
        frontier = next;
    }
    let idx = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads() {
            s.spawn(|| loop {
                let i = idx.fetch_add(1, Ordering::Relaxed);
                let Some((dir, depth)) = frontier.get(i) else {
                    break;
                };
                walk_dir(dir, *depth, &found);
            });
        }
    });
    let mut out = found.into_inner().unwrap();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn walk_dir(dir: &Path, depth: usize, found: &Mutex<Vec<(PathBuf, Kind, PathBuf)>>) {
    for (child, d) in children(dir, depth, found) {
        walk_dir(&child, d, found);
    }
}

/// Classifies the subdirectories of `dir` (at `depth`) and returns those to enter.
fn children(
    dir: &Path,
    depth: usize,
    found: &Mutex<Vec<(PathBuf, Kind, PathBuf)>>,
) -> Vec<(PathBuf, usize)> {
    if depth >= MAX_DEPTH {
        return Vec::new();
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut enter = Vec::new();
    for e in rd.flatten() {
        // file_type does not follow symlinks: symlinked dirs are not entered.
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = e.file_name();
        let name = name.to_string_lossy();
        if SKIP.contains(&name.as_ref()) {
            continue;
        }
        let path = e.path();
        let kind = classify(&path);
        if let Some((k, g)) = &kind {
            found.lock().unwrap().push((path.clone(), *k, g.clone()));
        }
        let bare = matches!(kind, Some((Kind::Bare, _)));
        if !bare && !name.starts_with('.') {
            enter.push((path, depth + 1));
        }
    }
    enter
}

pub fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16)
}

/// The common dir of a gitdir (a worktree's `commondir` file, else itself).
pub fn common_dir(gitdir: &Path) -> PathBuf {
    match fs::read_to_string(gitdir.join("commondir")) {
        Ok(rel) => {
            let p = gitdir.join(rel.trim());
            fs::canonicalize(&p).unwrap_or(p)
        }
        Err(_) => gitdir.to_path_buf(),
    }
}

/// The checkout a common dir belongs to: `<repo>/.git` -> `<repo>`, a bare dir itself.
pub fn main_of(common: &Path) -> PathBuf {
    if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().unwrap_or(common).to_path_buf()
    } else {
        common.to_path_buf()
    }
}

/// Worktrees a repo knows (`<common>/worktrees/*/gitdir`): (path, admin dir, exists).
pub fn registered_worktrees(common: &Path) -> Vec<(PathBuf, PathBuf, bool)> {
    let Ok(rd) = fs::read_dir(common.join("worktrees")) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let admin = e.path();
            let dotgit = fs::read_to_string(admin.join("gitdir")).ok()?;
            let dotgit = PathBuf::from(dotgit.trim());
            let path = dotgit.parent()?.to_path_buf();
            let exists = dotgit.is_file();
            Some((path, admin, exists))
        })
        .collect()
}

pub fn mtime(p: &Path) -> i64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64)
}

/// mtimes that move on commit, checkout, add, fetch, pull and branch changes.
pub fn fingerprint(gitdir: &Path, common: &Path) -> Vec<i64> {
    vec![
        mtime(&gitdir.join("HEAD")),
        mtime(&gitdir.join("index")),
        mtime(&common.join("FETCH_HEAD")),
        mtime(&common.join("packed-refs")),
        mtime(&common.join("refs/heads")),
    ]
}

/// Runs a read-only git command; stdout on success.
pub fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A fresh entry for one checkout, without the repo-level fields.
pub fn query(path: &Path, kind: Kind, gitdir: &Path) -> Entry {
    let common = common_dir(gitdir);
    let mut e = Entry {
        path: path.display().to_string(),
        kind,
        gitdir: gitdir.display().to_string(),
        common: common.display().to_string(),
        main: (kind == Kind::Worktree).then(|| main_of(&common).display().to_string()),
        remote: None,
        fork: false,
        branch: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        dirty: 0,
        slow: false,
        last_commit: None,
        head: None,
        touched: 0,
        default_branch: None,
        merged: false,
        gone: false,
        merged_branches: Vec::new(),
        prunable: false,
        fp: fingerprint(gitdir, &common),
        skills: Vec::new(),
        host: None,
    };
    if kind == Kind::Bare {
        e.branch = git(path, &["symbolic-ref", "-q", "--short", "HEAD"])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    } else {
        match status(path) {
            Some(out) => parse_status(&out, &mut e),
            None => {
                e.slow = true;
                cheap_branch(path, &mut e);
            }
        }
    }
    let log = git(path, &["log", "-1", "--format=%H %ct"]).unwrap_or_default();
    let mut log = log.split_whitespace();
    e.head = log.next().map(str::to_string);
    e.last_commit = log.next().and_then(|s| s.parse().ok());
    let remotes = git(path, &["config", "--get-regexp", r"^remote\..*\.url$"]).unwrap_or_default();
    e.fork = remotes
        .lines()
        .any(|l| l.starts_with("remote.upstream.url "));
    e.remote = pick_remote(&remotes).map(|u| crate::remote::normalise(&u));
    e.touched = touched(&e, gitdir);
    if kind != Kind::Bare {
        e.skills = crate::skills::collect(path);
    }
    e
}

/// `git status` bounded by `STATUS_TIMEOUT`; `None` when it took longer. Killing
/// it is safe: with `GIT_OPTIONAL_LOCKS=0` it holds no lock and writes nothing.
fn status(path: &Path) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .args([
            "status",
            "--porcelain=v2",
            "--branch",
            "--ignore-submodules=all",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    let out = vega_herdr::output_within(&mut cmd, STATUS_TIMEOUT).ok()??;
    // A failed status (not a repo any more) counts as clean, like a missing one.
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Branch, upstream and ahead/behind without `git status` (no work tree scan).
fn cheap_branch(path: &Path, e: &mut Entry) {
    let trimmed = |s: String| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    e.branch = git(path, &["symbolic-ref", "-q", "--short", "HEAD"]).and_then(trimmed);
    e.upstream = git(path, &["rev-parse", "--abbrev-ref", "@{u}"]).and_then(trimmed);
    if e.upstream.is_some() {
        if let Some(ab) = git(
            path,
            &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
        ) {
            let mut n = ab.split_whitespace().map(|x| x.parse().unwrap_or(0));
            e.ahead = n.next().unwrap_or(0);
            e.behind = n.next().unwrap_or(0);
        }
    }
}

fn touched(e: &Entry, gitdir: &Path) -> i64 {
    let secs = |ns: i64| ns / 1_000_000_000;
    e.last_commit
        .unwrap_or(0)
        .max(secs(mtime(&gitdir.join("HEAD"))))
        .max(secs(mtime(&gitdir.join("index"))))
}

/// `origin`'s URL, else the first remote's.
fn pick_remote(config: &str) -> Option<String> {
    let remotes: Vec<(&str, &str)> = config.lines().filter_map(|l| l.split_once(' ')).collect();
    remotes
        .iter()
        .find(|(k, _)| *k == "remote.origin.url")
        .or(remotes.first())
        .map(|(_, v)| v.trim().to_string())
}

/// Fills branch, upstream, ahead/behind and dirty from `git status --porcelain=v2 --branch`.
pub fn parse_status(status: &str, e: &mut Entry) {
    for line in status.lines() {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            e.branch = (h != "(detached)").then(|| h.to_string());
        } else if let Some(u) = line.strip_prefix("# branch.upstream ") {
            e.upstream = Some(u.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for t in ab.split_whitespace() {
                if let Some(n) = t.strip_prefix('+') {
                    e.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = t.strip_prefix('-') {
                    e.behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            e.dirty += 1;
        }
    }
}

/// Repo-level facts shared by a repo's checkouts.
#[derive(Default)]
pub struct RepoInfo {
    pub default_branch: Option<String>,
    pub merged: BTreeSet<String>,
    pub gone: BTreeSet<String>,
}

/// Default branch, merged and gone local branches of the repo at `common`.
pub fn repo_info(common: &Path) -> RepoInfo {
    let dir = main_of(common);
    // Merge target: the remote's default (`origin/HEAD`), else a local main/master.
    let origin_head = git(&dir, &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])
        .map(|s| s.trim().trim_start_matches("refs/remotes/").to_string())
        .filter(|s| !s.is_empty());
    let local = |b: &str| {
        git(
            &dir,
            &["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")],
        )
        .is_some()
    };
    let (target, default) = match origin_head {
        Some(t) => {
            let name = t.split_once('/').map_or(t.clone(), |(_, b)| b.to_string());
            (t, name)
        }
        None => match ["main", "master"].into_iter().find(|b| local(b)) {
            Some(b) => (b.to_string(), b.to_string()),
            None => return RepoInfo::default(),
        },
    };
    let merged = git(
        &dir,
        &[
            "for-each-ref",
            &format!("--merged={target}"),
            "--format=%(refname:short)",
            "refs/heads",
        ],
    )
    .unwrap_or_default()
    .lines()
    .map(str::to_string)
    .filter(|b| *b != default)
    .collect();
    let gone = git(
        &dir,
        &[
            "for-each-ref",
            "--format=%(refname:short)%09%(upstream:track)",
            "refs/heads",
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| l.split_once('\t'))
    .filter(|(_, t)| *t == "[gone]")
    .map(|(b, _)| b.to_string())
    .collect();
    RepoInfo {
        default_branch: Some(default),
        merged,
        gone,
    }
}

/// Sets the repo-level fields of every entry in `entries` that shares a common dir with `commons`.
pub fn apply_repo_info(entries: &mut [Entry], commons: &BTreeSet<String>) {
    let infos: BTreeMap<String, RepoInfo> = par_map(commons.iter().cloned().collect(), |c| {
        let info = repo_info(Path::new(&c));
        (c, info)
    })
    .into_iter()
    .collect();
    // Branches checked out somewhere are not cleanup candidates as branches.
    let mut checked_out: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for e in entries.iter() {
        if let Some(b) = &e.branch {
            checked_out
                .entry(e.common.as_str())
                .or_default()
                .insert(b.clone());
        }
    }
    let checked_out: BTreeMap<String, BTreeSet<String>> = checked_out
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    for e in entries.iter_mut() {
        let Some(info) = infos.get(&e.common) else {
            continue;
        };
        e.default_branch = info.default_branch.clone();
        e.merged = e.branch.as_ref().is_some_and(|b| info.merged.contains(b));
        e.gone = e.branch.as_ref().is_some_and(|b| info.gone.contains(b));
        e.merged_branches = if e.kind == Kind::Worktree {
            Vec::new()
        } else {
            let busy = checked_out.get(&e.common);
            info.merged
                .iter()
                .filter(|b| !busy.is_some_and(|s| s.contains(*b)))
                .cloned()
                .collect()
        };
    }
}

/// Maps `f` over `items` on all cores, keeping the order.
pub fn par_map<T: Send + Sync + Clone, R: Send>(
    items: Vec<T>,
    f: impl Fn(T) -> R + Sync,
) -> Vec<R> {
    let idx = AtomicUsize::new(0);
    let out: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..threads().min(items.len().max(1)) {
            s.spawn(|| loop {
                let i = idx.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(i) else { break };
                let r = f(item.clone());
                out.lock().unwrap().push((i, r));
            });
        }
    });
    let mut out = out.into_inner().unwrap();
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

/// Every checkout under `roots`, plus worktrees their repos know elsewhere
/// (and prunable ones), fully queried.
pub fn scan(roots: &[PathBuf]) -> Vec<Entry> {
    let t = std::time::Instant::now();
    let (found, prunable) = discover(roots);
    trace("discover", t);
    let mut entries = par_map(found, |(p, k, g)| query(&p, k, &g));
    trace("query", t);
    entries.extend(prunable);
    finish(&mut entries);
    trace("repo info", t);
    entries
}

/// Phase timings on stderr with `VEGA_JANITOR_TRACE=1`.
fn trace(phase: &str, since: std::time::Instant) {
    if std::env::var_os("VEGA_JANITOR_TRACE").is_some() {
        eprintln!("trace: {phase} done at {} ms", since.elapsed().as_millis());
    }
}

/// Repo-level fields for all, sorted by path, one entry per path.
pub fn finish(entries: &mut Vec<Entry>) {
    let commons: BTreeSet<String> = entries.iter().map(|e| e.common.clone()).collect();
    apply_repo_info(entries, &commons);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries.dedup_by(|a, b| a.path == b.path);
}

/// A found checkout: (path, kind, gitdir).
pub type Found = (PathBuf, Kind, PathBuf);

/// The walk plus the worktrees the found repos know elsewhere; prunable ones
/// come back as finished entries (there is nothing to ask git).
pub fn discover(roots: &[PathBuf]) -> (Vec<Found>, Vec<Entry>) {
    let mut found = walk(roots);
    let known: BTreeSet<PathBuf> = found.iter().map(|(p, _, _)| p.clone()).collect();
    let commons: BTreeSet<PathBuf> = found
        .iter()
        .filter(|(_, k, _)| *k != Kind::Worktree)
        .map(|(_, _, g)| g.clone())
        .chain(
            found
                .iter()
                .filter(|(_, k, _)| *k == Kind::Worktree)
                .map(|(_, _, g)| common_dir(g)),
        )
        .collect();
    let mut prunable = Vec::new();
    for c in &commons {
        for (path, admin, exists) in registered_worktrees(c) {
            if known.contains(&path) {
                continue;
            }
            if exists {
                found.push((path, Kind::Worktree, admin));
            } else {
                prunable.push(prunable_entry(&path, &admin, c));
            }
        }
    }
    (found, prunable)
}

fn prunable_entry(path: &Path, admin: &Path, common: &Path) -> Entry {
    let head = fs::read_to_string(admin.join("HEAD")).unwrap_or_default();
    Entry {
        path: path.display().to_string(),
        kind: Kind::Worktree,
        gitdir: admin.display().to_string(),
        common: common.display().to_string(),
        main: Some(main_of(common).display().to_string()),
        remote: None,
        fork: false,
        branch: head
            .trim()
            .strip_prefix("ref: refs/heads/")
            .map(str::to_string),
        upstream: None,
        ahead: 0,
        behind: 0,
        dirty: 0,
        slow: false,
        last_commit: None,
        head: None,
        touched: mtime(&admin.join("HEAD")) / 1_000_000_000,
        default_branch: None,
        merged: false,
        gone: false,
        merged_branches: Vec::new(),
        prunable: true,
        fp: Vec::new(),
        skills: Vec::new(),
        host: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let mut e = prunable_entry(Path::new("/x"), Path::new("/x/a"), Path::new("/x/.git"));
        parse_status(
            "# branch.oid abc\n# branch.head feat\n# branch.upstream origin/feat\n# branch.ab +2 -5\n1 .M N... 100644 100644 100644 a b f\n? new.txt\n",
            &mut e,
        );
        assert_eq!(e.branch.as_deref(), Some("feat"));
        assert_eq!(e.upstream.as_deref(), Some("origin/feat"));
        assert_eq!((e.ahead, e.behind, e.dirty), (2, 5, 2));
        let mut d = e.clone();
        parse_status("# branch.oid abc\n# branch.head (detached)\n", &mut d);
        assert_eq!(d.branch, None);
    }

    #[test]
    fn picks_origin_first() {
        let c =
            "remote.fork.url git@github.com:me/r.git\nremote.origin.url https://github.com/o/r\n";
        assert_eq!(pick_remote(c).as_deref(), Some("https://github.com/o/r"));
        assert_eq!(
            pick_remote("remote.up.url u\n").as_deref(),
            Some("u"),
            "no origin: the first remote"
        );
    }
}
