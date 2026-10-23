//! Local files under `$XDG_STATE_HOME/vega/ci/`:
//!
//! - `watch/<target key>__<pane>.json`: one per watcher, written by `vega ci
//!   watch`, removed by `unwatch` and by vegad when the watch ends.
//! - `state/<target key>.json`: vegad's cache of a target (only vegad writes).
//! - `rate.json`: the last rate-limit headers any request saw.
//! - `forges.json`: the forge of each probed host (see [`crate::forge`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::checks::{summarize, Bucket, Check, Summary};
use crate::forge::Forge;
use crate::gh::Resp;
use crate::target::Target;

pub fn dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))
        .map(|s| s.join("vega").join("ci"))
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn read<T: DeserializeOwned>(file: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(file).ok()?).ok()
}

/// Write to a temp file, then rename: a reader never sees half a file.
fn write<T: Serialize>(file: &Path, value: &T) -> bool {
    let Some(parent) = file.parent() else {
        return false;
    };
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    fs::create_dir_all(parent).is_ok()
        && serde_json::to_vec_pretty(value)
            .ok()
            .is_some_and(|b| fs::write(&tmp, b).is_ok())
        && fs::rename(&tmp, file).is_ok()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub target: Target,
    pub pane: String,
    pub since: i64,
}

fn watch_file(t: &Target, pane: &str) -> Option<PathBuf> {
    dir().map(|d| {
        d.join("watch")
            .join(format!("{}__{}.json", t.key(), safe(pane)))
    })
}

/// Registers `pane` for `t`; false if the file could not be written.
pub fn add_watch(w: &Watch) -> bool {
    watch_file(&w.target, &w.pane).is_some_and(|f| write(&f, w))
}

/// Removes `pane`'s watch of `t`; false if there was none.
pub fn remove_watch(t: &Target, pane: &str) -> bool {
    watch_file(t, pane).is_some_and(|f| fs::remove_file(f).is_ok())
}

pub fn watches() -> Vec<Watch> {
    let Some(entries) = dir().and_then(|d| fs::read_dir(d.join("watch")).ok()) else {
        return Vec::new();
    };
    let mut out: Vec<Watch> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read(&e.path()))
        .collect();
    out.sort_by(|a, b| {
        (a.target.label(), a.since, &a.pane).cmp(&(b.target.label(), b.since, &b.pane))
    });
    out
}

/// A target without checks this long after its last change has no CI to
/// wait for.
const NO_CHECKS_GRACE: i64 = 15 * 60;

/// Whether `pane` still waits for a verdict mail of the target `s` caches
/// (`None`: not polled yet): no verdict yet, or a new one not mailed. A
/// mailed verdict or an ended target owes nothing; the watch stays until the
/// PR merges, but the pane is no longer waiting.
pub fn owed(pane: &str, s: Option<&State>, now: i64) -> bool {
    let Some(s) = s else { return true };
    if s.ended.is_some() {
        return false;
    }
    // A conflicting PR gets no CI until the rebase, a zombie run never
    // completes: once told, the pane has its next step.
    let run = s.target.as_ref().is_some_and(Target::is_run);
    let told = |k: &String| s.signals.get(pane).is_some_and(|sent| sent.contains(k));
    if s.signal_keys()
        .iter()
        .any(|k| told(k) && (k.starts_with("merge:") || run))
    {
        return false;
    }
    // A run owes its verdict until it completes, however long it queues.
    if run && s.run_status.as_deref() != Some("completed") {
        return true;
    }
    let sum = s.summary();
    match sum.verdict {
        crate::Verdict::Pending => true,
        crate::Verdict::None => now - s.changed < NO_CHECKS_GRACE,
        v => {
            let key = format!("{}:{}", s.head.as_deref().unwrap_or(""), v.as_str());
            s.notified.get(pane) != Some(&key)
        }
    }
}

/// The panes a watch still owes a verdict mail (see [`owed`]), each with
/// the labels of those targets.
pub fn waiting(now: i64) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for w in watches() {
        if owed(&w.pane, load_state(&w.target).as_ref(), now) {
            out.entry(w.pane).or_default().push(w.target.label());
        }
    }
    out
}

/// vegad's cache of one target.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub target: Option<Target>,
    /// Head sha the checks belong to.
    #[serde(default)]
    pub head: Option<String>,
    /// PR state: `open`, `merged`, `closed`.
    #[serde(default)]
    pub pr_state: Option<String>,
    #[serde(default)]
    pub runs: Vec<Check>,
    #[serde(default)]
    pub statuses: Vec<Check>,
    /// GitHub PR/branch: workflow runs for this head. None until fetched,
    /// including caches written before workflow runs were checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_workflows: Option<Vec<Check>>,
    /// ETag per request path of the current head.
    #[serde(default)]
    pub etags: BTreeMap<String, String>,
    /// Last poll and last change (Unix seconds).
    #[serde(default)]
    pub polled: i64,
    #[serde(default)]
    pub changed: i64,
    /// Earliest next poll.
    #[serde(default)]
    pub next: i64,
    /// Per pane: the `<head>:<verdict>` it got a mail for.
    #[serde(default)]
    pub notified: BTreeMap<String, String>,
    /// Why the watch ended (merged, closed, branch gone, no change).
    #[serde(default)]
    pub ended: Option<String>,
    /// The last poll's error, if it failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Run and workflow targets: the run (resolved for a workflow), its
    /// `status` (`queued`, `in_progress`, `completed`, …) and `conclusion`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_conclusion: Option<String>,
    /// Workflow targets: the earliest watch (Unix seconds); the dispatched run
    /// is the newest one created at most [`crate::poll::DISPATCH_SLACK`] before it.
    #[serde(default)]
    pub since: i64,
    /// PR: `mergeable_state` (`clean`, `dirty`, `behind`, `blocked`,
    /// `unstable`, `unknown` while GitHub computes it, …); `dirty` also when
    /// `mergeable` is false (a draft says `draft` even with conflicts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_state: Option<String>,
    /// PR: the head branch, whose workflow runs the zombie check lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    /// Workflow runs of the branch (or the run target) still `queued` or
    /// `in_progress`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_runs: Vec<u64>,
    /// Open runs that look done, and since when (the next jobs check is
    /// [`crate::poll::ZOMBIE_AFTER`] later).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stuck: BTreeMap<u64, i64>,
    /// Open runs whose jobs all completed: zombies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zombies: Vec<u64>,
    /// Per pane: the signal keys (see [`State::signal_keys`]) it got a mail for.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub signals: BTreeMap<String, BTreeSet<String>>,
}

impl State {
    pub fn new(t: &Target, now: i64) -> State {
        State {
            target: Some(t.clone()),
            changed: now,
            ..State::default()
        }
    }

    pub fn checks(&self) -> Vec<Check> {
        let mut checks: Vec<_> = self.runs.iter().chain(&self.statuses).cloned().collect();
        if self
            .target
            .as_ref()
            .is_some_and(|t| t.forge.is_github() && !t.is_run())
        {
            match &self.head_workflows {
                Some(runs) => checks.extend(runs.iter().cloned()),
                None => checks.push(Check {
                    name: "workflow runs (not fetched yet)".into(),
                    bucket: Bucket::Pending,
                    note: None,
                }),
            }
        }
        checks
    }

    pub fn summary(&self) -> Summary {
        summarize(&self.checks())
    }

    /// True when the PR cannot merge as it is: conflicts (`dirty`), or behind a
    /// base that requires up-to-date branches (`behind`).
    pub fn needs_rebase(&self) -> bool {
        self.pr_state.as_deref() == Some("open")
            && matches!(self.merge_state.as_deref(), Some("dirty" | "behind"))
    }

    /// The signals that are true now, each mailed once per pane:
    /// `merge:<head>:<state>` and `zombie:<run id>`.
    pub fn signal_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        if self.needs_rebase() {
            keys.push(format!(
                "merge:{}:{}",
                self.head.as_deref().unwrap_or(""),
                self.merge_state.as_deref().unwrap_or("")
            ));
        }
        keys.extend(self.zombies.iter().map(|id| format!("zombie:{id}")));
        keys
    }
}

fn state_file(t: &Target) -> Option<PathBuf> {
    dir().map(|d| d.join("state").join(format!("{}.json", t.key())))
}

pub fn load_state(t: &Target) -> Option<State> {
    read(&state_file(t)?)
}

pub fn save_state(t: &Target, s: &State) -> bool {
    state_file(t).is_some_and(|f| write(&f, s))
}

pub fn remove_state(t: &Target) {
    if let Some(f) = state_file(t) {
        let _ = fs::remove_file(f);
    }
}

fn forges_file() -> Option<PathBuf> {
    dir().map(|d| d.join("forges.json"))
}

/// The cached forge per host.
pub fn load_forges() -> BTreeMap<String, Forge> {
    forges_file().and_then(|f| read(&f)).unwrap_or_default()
}

pub fn save_forges(forges: &BTreeMap<String, Forge>) -> bool {
    forges_file().is_some_and(|f| write(&f, forges))
}

/// The last rate-limit headers seen (core), and the ETag of `GET /user`,
/// which refreshes them for free.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    pub limit: i64,
    pub remaining: i64,
    pub reset: i64,
    /// When these were seen.
    pub at: i64,
    #[serde(default)]
    pub user_etag: Option<String>,
}

fn rate_file() -> Option<PathBuf> {
    dir().map(|d| d.join("rate.json"))
}

pub fn load_rate() -> Option<Rate> {
    read(&rate_file()?)
}

/// Records the rate headers of `r`, if it has them.
pub fn note_rate(r: &Resp, now: i64) {
    let (Some(limit), Some(remaining), Some(reset)) = (r.limit, r.remaining, r.reset) else {
        return;
    };
    let old = load_rate().unwrap_or_default();
    let rate = Rate {
        limit,
        remaining,
        reset,
        at: now,
        user_etag: old.user_etag,
    };
    if let Some(f) = rate_file() {
        write(&f, &rate);
    }
}

/// The current rate: the cached one when younger than `max_age` seconds, else
/// a conditional `GET /user` (a 304 costs nothing; the first call costs 1).
pub fn rate(max_age: i64, now: i64) -> Result<Rate, String> {
    let cached = load_rate();
    if let Some(r) = cached.as_ref().filter(|r| now - r.at < max_age) {
        return Ok(r.clone());
    }
    let etag = cached.and_then(|r| r.user_etag);
    let resp = crate::gh::get(crate::target::GITHUB, "user", etag.as_deref())?;
    note_rate(&resp, now);
    let mut r =
        load_rate().ok_or_else(|| format!("GET /user: HTTP {}, no rate headers", resp.status))?;
    if resp.status == 200 && resp.etag.is_some() {
        r.user_etag = resp.etag;
        if let Some(f) = rate_file() {
            write(&f, &r);
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::{Bucket, Check};

    fn state(head: &str, buckets: &[Bucket]) -> State {
        let t = Target::pr(crate::target::GITHUB, "o/r", 42);
        let mut s = State::new(&t, 1000);
        s.head_workflows = Some(Vec::new());
        s.head = Some(head.into());
        s.runs = buckets
            .iter()
            .map(|b| Check {
                name: "test".into(),
                bucket: *b,
                note: None,
            })
            .collect();
        s
    }

    #[test]
    fn a_watch_file_from_before_forgejo_support_loads() {
        let old = include_str!("../tests/fixtures/watch-github-v1.json");
        let w: Watch = serde_json::from_str(old).unwrap();
        assert_eq!(w.target, Target::pr(crate::target::GITHUB, "o/r", 42));
        assert_eq!(w.target.forge, Forge::GitHub);
        // Written back unchanged: no forge field for GitHub.
        let back: serde_json::Value = serde_json::to_value(&w).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(old).unwrap()
        );
    }

    #[test]
    fn a_pane_waits_until_its_verdict_is_mailed() {
        // Not polled yet, or checks still run: waiting.
        assert!(owed("w1:p1", None, 1000));
        let mut s = state("abc", &[Bucket::Pending]);
        assert!(owed("w1:p1", Some(&s), 1000));
        // Green, mail not sent yet: still waiting; once sent: not.
        s = state("abc", &[Bucket::Pass]);
        assert!(owed("w1:p1", Some(&s), 1000));
        s.notified.insert("w1:p1".into(), "abc:green".into());
        assert!(!owed("w1:p1", Some(&s), 1000));
        // Another pane on the same target was not mailed yet.
        assert!(owed("w2:p1", Some(&s), 1000));
        // A new head: a new verdict is owed.
        s.head = Some("def".into());
        assert!(owed("w1:p1", Some(&s), 1000));
        // Ended (merged): the last mail goes out, nothing is owed.
        s.ended = Some("merged".into());
        assert!(!owed("w1:p1", Some(&s), 1000));
        // No checks: waits a while for CI to start, then owes nothing.
        let none = state("abc", &[]);
        assert!(owed("w1:p1", Some(&none), 1000 + 60));
        assert!(!owed("w1:p1", Some(&none), 1000 + NO_CHECKS_GRACE));
    }

    #[test]
    fn a_run_is_owed_until_it_completes() {
        let t = Target::workflow(crate::target::GITHUB, "o/r", "deploy.yml", Some("main"));
        let mut s = State::new(&t, 1000);
        // Queued for an hour without jobs: still waiting.
        s.run_status = Some("queued".into());
        assert!(owed("w1:p1", Some(&s), 1000 + 3600));
        s.run_status = Some("completed".into());
        s.head = Some("abc".into());
        s.runs = vec![Check {
            name: "deploy".into(),
            bucket: Bucket::Pass,
            note: None,
        }];
        assert!(owed("w1:p1", Some(&s), 1000));
        s.notified.insert("w1:p1".into(), "abc:green".into());
        assert!(!owed("w1:p1", Some(&s), 1000));
    }
}
