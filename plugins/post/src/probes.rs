//! vegad's per-path git probes for the sidebar, cached (docs/design/vegad.md,
//! Sidebar). A project's identity (four git calls per path) is resolved again
//! only when its repository config changes or the entry ages out; `git
//! status` runs on its own thread, so a metadata refresh never waits on git.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use vega_herdr::project::Project;
use vega_herdr::Failure;

/// A resolved project is trusted this long while its config is unchanged.
const RESOLVED_FOR: Duration = Duration::from_secs(600);

/// A path that did not resolve is tried again after this long, or at once
/// when the path itself changes (e.g. `git init`).
const FAILED_FOR: Duration = Duration::from_secs(60);

/// How often each checkout's `$git` is re-read.
const GIT_EVERY: Duration = Duration::from_secs(30);

/// A checkout no refresh asked about for this long is no longer probed.
const GIT_UNWANTED_AFTER: Duration = Duration::from_secs(120);

struct Resolved {
    at: Instant,
    /// mtime of the common dir's config (Ok) or of the path (Err).
    stamp: Option<SystemTime>,
    result: Result<Project, (String, String, String)>,
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl Resolved {
    fn fresh(&self, path: &Path, now: Instant) -> bool {
        let age = now.duration_since(self.at);
        match &self.result {
            Ok(p) => {
                age < RESOLVED_FOR && mtime(&p.location.common_dir.join("config")) == self.stamp
            }
            Err(_) => age < FAILED_FOR && mtime(path) == self.stamp,
        }
    }
}

/// `vega_herdr::project::resolve`, cached per path and host.
pub fn resolve(path: &Path, host: &str) -> Result<Project, Failure> {
    type Cache = HashMap<(PathBuf, String), Resolved>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = (path.to_path_buf(), host.to_owned());
    let now = Instant::now();
    let hit = cache.get(&key).filter(|r| r.fresh(path, now));
    let result = match hit {
        Some(r) => r.result.clone(),
        None => {
            let result = vega_herdr::project::resolve(path, host);
            let stamp = match &result {
                Ok(p) => mtime(&p.location.common_dir.join("config")),
                Err(_) => mtime(path),
            };
            let result = result.map_err(|f| (f.code, f.message, f.hint));
            cache.insert(
                key,
                Resolved {
                    at: now,
                    stamp,
                    result: result.clone(),
                },
            );
            result
        }
    };
    result.map_err(|(code, message, hint)| Failure {
        code,
        message,
        hint,
    })
}

#[derive(Default)]
struct Git {
    /// Each checkout a refresh asked about, and when.
    asked: HashMap<String, Instant>,
    /// Its last `$git` (None: git failed), and when it was read.
    read: HashMap<String, (Option<String>, Instant)>,
}

fn git_store() -> &'static Mutex<Git> {
    static GIT: OnceLock<Mutex<Git>> = OnceLock::new();
    GIT.get_or_init(|| {
        std::thread::spawn(git_worker);
        Default::default()
    })
}

/// The `$git` token of the checkout at `path` as last read off the loop;
/// `None` until the first read or when git fails. Asking keeps it probed.
pub fn git(path: &str) -> Option<String> {
    let mut g = git_store().lock().unwrap_or_else(|e| e.into_inner());
    g.asked.insert(path.to_string(), Instant::now());
    g.read.get(path).and_then(|(t, _)| t.clone())
}

/// The next checkout to read: never read first, then the oldest past GIT_EVERY.
fn next_git(g: &mut Git, now: Instant) -> Option<String> {
    g.asked
        .retain(|_, at| now.duration_since(*at) < GIT_UNWANTED_AFTER);
    let asked = &g.asked;
    g.read.retain(|p, _| asked.contains_key(p));
    g.asked
        .keys()
        .filter_map(|p| match g.read.get(p) {
            None => Some((None, p)),
            Some((_, at)) if now.duration_since(*at) >= GIT_EVERY => Some((Some(*at), p)),
            Some(_) => None,
        })
        .min()
        .map(|(_, p)| p.clone())
}

fn git_worker() {
    loop {
        let next = {
            let mut g = git_store().lock().unwrap_or_else(|e| e.into_inner());
            next_git(&mut g, Instant::now())
        };
        let Some(path) = next else {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };
        let token = crate::git_state(&path);
        let mut g = git_store().lock().unwrap_or_else(|e| e.into_inner());
        g.read.insert(path, (token, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_reads_new_checkouts_first_then_the_oldest_due_one() {
        let t0 = Instant::now();
        let mut g = Git::default();
        for p in ["a", "b", "c"] {
            g.asked.insert(p.into(), t0);
        }
        g.read.insert("a".into(), (Some("✓".into()), t0));
        g.read
            .insert("b".into(), (None, t0 + Duration::from_secs(1)));
        let next = next_git(&mut g, t0 + Duration::from_secs(2));
        assert_eq!(next.as_deref(), Some("c"));
        g.read
            .insert("c".into(), (None, t0 + Duration::from_secs(2)));
        assert_eq!(next_git(&mut g, t0 + Duration::from_secs(3)), None);
        let next = next_git(&mut g, t0 + GIT_EVERY + Duration::from_secs(1));
        assert_eq!(next.as_deref(), Some("a"));
    }

    #[test]
    fn git_forgets_checkouts_no_refresh_asks_about() {
        let t0 = Instant::now();
        let mut g = Git::default();
        g.asked.insert("gone".into(), t0);
        g.read.insert("gone".into(), (Some("✓".into()), t0));
        assert_eq!(next_git(&mut g, t0 + GIT_UNWANTED_AFTER), None);
        assert!(g.read.is_empty());
    }

    #[test]
    fn resolve_is_cached_until_the_config_changes() {
        let dir = std::env::temp_dir().join(format!("vega-probes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Not a repository yet: the failure is cached until the path changes.
        assert!(resolve(&dir, "testhost").is_err());
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "https://github.com/team/one.git"]);
        let first = resolve(&dir, "testhost").unwrap();
        assert_eq!(first.id.as_str(), "github.com/team/one");
        // Same config: the cached answer.
        assert_eq!(resolve(&dir, "testhost").unwrap(), first);
        // mtime granularity: make sure the rewrite lands on a later stamp.
        std::thread::sleep(Duration::from_millis(20));
        git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/team/two.git",
        ]);
        let second = resolve(&dir, "testhost").unwrap();
        assert_eq!(second.id.as_str(), "github.com/team/two");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
