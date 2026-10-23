//! The poll cycle vegad runs: each watched target once per due cycle, with
//! conditional requests, and the mails a new verdict causes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::checks::{
    parse_check_runs, parse_jobs, parse_status, parse_status_forgejo, Bucket, Check, Verdict,
};
use crate::forge::Forge;
use crate::gh::{escape, Resp};
use crate::store::{self, Rate, State, Watch};
use crate::target::Target;

/// Poll interval while checks run (or none exist yet), and once settled.
pub const PENDING_EVERY: i64 = 30;
pub const SETTLED_EVERY: i64 = 120;
/// Below this many remaining core requests, poll four times less often.
pub const SLOW_BELOW: i64 = 1000;
/// Below this, poll nothing until the limit resets.
pub const STOP_BELOW: i64 = 200;
/// A target without any change for this long ends.
pub const IDLE_END: i64 = 3 * 86400;
/// A workflow target takes the newest `workflow_dispatch` run created at
/// most this long before its first watch: `vega ci watch` usually follows
/// `gh workflow run`, sometimes a few minutes later.
pub const DISPATCH_SLACK: i64 = 600;
/// A workflow target that finds no run this long after its first watch ends.
pub const RESOLVE_WITHIN: i64 = 15 * 60;
/// A `queued` or `in_progress` run whose jobs all completed this long ago (by
/// our polls) is a zombie: it holds its concurrency group and never ends.
pub const ZOMBIE_AFTER: i64 = 15 * 60;
/// Run statuses of a run that has not ended (`waiting`, `pending` and
/// `requested` wait for an approval or a concurrency group: not zombies).
const OPEN_RUN: [&str; 2] = ["queued", "in_progress"];
/// At least this long after a 403/429.
const LIMITED_PAUSE: i64 = 60;
/// Failed check names listed in a mail.
const FAILED_SHOWN: usize = 5;

/// `(forge, host, path, etag) -> response`: `gh::get` or `tea::get` in vegad
/// (Forgejo has no ETags: the etag is always `None` there), a fake in tests.
pub type Fetch<'a> = dyn FnMut(Forge, &str, &str, Option<&str>) -> Result<Resp, String> + 'a;

#[derive(Debug, PartialEq)]
pub enum PollError {
    /// 403/429: wait this many seconds.
    Limited(i64),
    Other(String),
}

enum Got {
    Fresh(String),
    Same,
    Status(u16),
}

struct Req<'a, 'b> {
    forge: Forge,
    host: &'a str,
    old: &'a BTreeMap<String, String>,
    used: BTreeMap<String, String>,
    fetch: &'a mut Fetch<'b>,
    requests: usize,
}

impl Req<'_, '_> {
    fn get(&mut self, path: String) -> Result<Got, PollError> {
        let etag = self.old.get(&path).cloned();
        self.requests += 1;
        let r = (self.fetch)(self.forge, self.host, &path, etag.as_deref())
            .map_err(PollError::Other)?;
        match r.status {
            200 => {
                if let Some(e) = r.etag {
                    self.used.insert(path, e);
                }
                Ok(Got::Fresh(r.body))
            }
            304 => {
                if let Some(e) = etag {
                    self.used.insert(path, e);
                }
                Ok(Got::Same)
            }
            // GitHub says 403 for a spent limit; on Forgejo 403 is a token without
            // access, which waiting does not fix.
            429 => Err(PollError::Limited(
                r.retry_after.unwrap_or(0).max(LIMITED_PAUSE),
            )),
            403 if self.forge.is_github() => Err(PollError::Limited(
                r.retry_after.unwrap_or(0).max(LIMITED_PAUSE),
            )),
            s => Ok(Got::Status(s)),
        }
    }
}

/// Polls `s`'s target once; true if anything changed. Ends the target (sets
/// `ended`) when the PR is merged or closed or the branch is gone.
pub fn poll(s: &mut State, now: i64, fetch: &mut Fetch) -> Result<bool, PollError> {
    match s.target.as_ref().map(|t| (t.forge, t.is_run())) {
        Some((Forge::Forgejo, true)) => Err(PollError::Other(
            "run watches are GitHub Actions only; watch the PR or branch".into(),
        )),
        Some((Forge::Forgejo, false)) => poll_forgejo(s, now, fetch),
        Some((_, true)) => poll_run(s, now, fetch),
        _ => poll_github(s, now, fetch),
    }
}

/// `2026-10-16T07:08:09Z` for Unix seconds `t`: the format of GitHub's
/// `created_at`, so the two compare as strings.
pub fn iso(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86400), t.rem_euclid(86400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// A sha (40 hex digits) filters by `head_sha`, anything else by `branch`.
fn is_sha(r: &str) -> bool {
    r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit())
}

/// GitHub Actions: a run (`actions/runs/<id>` and its jobs), or a workflow's
/// newest `workflow_dispatch` run since the watch, resolved first. The run
/// itself counts as one check next to its jobs, so a failed run without a
/// failed job is red too. A completed run that neither passed nor failed
/// (cancelled, skipped) ends the target with its conclusion.
fn poll_run(s: &mut State, now: i64, fetch: &mut Fetch) -> Result<bool, PollError> {
    let t = s
        .target
        .clone()
        .ok_or(PollError::Other("state without target".into()))?;
    let repo = &t.repo;
    let old = s.etags.clone();
    let mut req = Req {
        forge: t.forge,
        host: &t.host,
        old: &old,
        used: BTreeMap::new(),
        fetch,
        requests: 0,
    };
    let mut changed = false;

    let id = match t.run.or(s.run_id) {
        Some(id) => id,
        None => {
            let w = t.workflow.as_deref().unwrap_or_default();
            let mut path = format!(
                "repos/{repo}/actions/workflows/{}/runs?event=workflow_dispatch&per_page=20",
                escape(w)
            );
            match t.git_ref.as_deref() {
                Some(r) if is_sha(r) => path.push_str(&format!("&head_sha={r}")),
                Some(r) => path.push_str(&format!("&branch={}", escape(r))),
                None => {}
            }
            let found = match req.get(path)? {
                Got::Fresh(body) => newest_run(&body, &iso(s.since - DISPATCH_SLACK))?,
                Got::Same => None,
                Got::Status(404) => {
                    s.ended = Some(format!("not found: no workflow {w} in {repo}"));
                    return Ok(true);
                }
                Got::Status(st) => return Err(PollError::Other(format!("runs: HTTP {st}"))),
            };
            match found {
                Some(id) => {
                    changed = true;
                    id
                }
                None => {
                    s.etags = req.used;
                    s.polled = now;
                    if now - s.since > RESOLVE_WITHIN {
                        s.ended = Some(format!(
                            "found no workflow_dispatch run in {} min",
                            RESOLVE_WITHIN / 60
                        ));
                        return Ok(true);
                    }
                    return Ok(false);
                }
            }
        }
    };
    s.run_id = Some(id);

    match req.get(format!("repos/{repo}/actions/runs/{id}"))? {
        Got::Fresh(body) => {
            let v: Value =
                serde_json::from_str(&body).map_err(|e| PollError::Other(format!("run: {e}")))?;
            let status = v["status"].as_str().map(String::from);
            let conclusion = v["conclusion"].as_str().map(String::from);
            let head = v["head_sha"].as_str().map(String::from);
            let me = vec![Check {
                name: v["name"].as_str().unwrap_or("run").to_string(),
                bucket: Bucket::of_run(status.as_deref().unwrap_or(""), conclusion.as_deref()),
                note: None,
            }];
            if (&status, &conclusion, &head, &me)
                != (&s.run_status, &s.run_conclusion, &s.head, &s.statuses)
            {
                (s.run_status, s.run_conclusion, s.head) = (status, conclusion, head);
                s.statuses = me;
                changed = true;
            }
        }
        Got::Same => {}
        Got::Status(404) => {
            s.ended = Some("not found".into());
            return Ok(true);
        }
        Got::Status(st) => return Err(PollError::Other(format!("run: HTTP {st}"))),
    }
    match req.get(format!("repos/{repo}/actions/runs/{id}/jobs?per_page=100"))? {
        Got::Fresh(body) => {
            let jobs = parse_jobs(&body).map_err(PollError::Other)?;
            if jobs != s.runs {
                s.runs = jobs;
                changed = true;
            }
        }
        Got::Same => {}
        Got::Status(st) => return Err(PollError::Other(format!("jobs: HTTP {st}"))),
    }
    let open = OPEN_RUN.contains(&s.run_status.as_deref().unwrap_or(""));
    s.open_runs = if open { vec![id] } else { Vec::new() };
    if open && s.runs.iter().all(|j| j.bucket != Bucket::Pending) {
        let since = *s.stuck.entry(id).or_insert(now);
        if now - since >= ZOMBIE_AFTER && !s.zombies.contains(&id) {
            s.zombies.push(id);
            changed = true;
        }
    } else {
        s.stuck.clear();
        s.zombies.clear();
    }
    s.etags = req.used;
    s.polled = now;
    if changed {
        s.changed = now;
    }
    if s.run_status.as_deref() == Some("completed") {
        let c = s.run_conclusion.as_deref();
        if !matches!(Bucket::of_run("completed", c), Bucket::Pass | Bucket::Fail) {
            s.ended = Some(format!("completed: {}", c.unwrap_or("no conclusion")));
        }
    }
    Ok(changed)
}

/// The id of the newest run in a `workflows/<w>/runs` body created at or
/// after `cutoff` (ISO 8601, UTC).
fn newest_run(body: &str, cutoff: &str) -> Result<Option<u64>, PollError> {
    let v: Value =
        serde_json::from_str(body).map_err(|e| PollError::Other(format!("runs: {e}")))?;
    let runs = v["workflow_runs"]
        .as_array()
        .ok_or(PollError::Other("runs: no workflow_runs".into()))?;
    Ok(runs
        .iter()
        .filter_map(|r| Some((r["created_at"].as_str()?, r["id"].as_u64()?)))
        .filter(|(c, _)| *c >= cutoff)
        .max()
        .map(|(_, id)| id))
}

/// The PR's head sha and state (`open`, `closed`, `merged`) from a `pulls/n`
/// body, which is the same on GitHub and Forgejo, and on GitHub its merge
/// state and head branch. True if any changed.
fn apply_pull(s: &mut State, body: &str, github: bool) -> Result<bool, PollError> {
    let v: Value =
        serde_json::from_str(body).map_err(|e| PollError::Other(format!("pull: {e}")))?;
    let head = v["head"]["sha"].as_str().map(String::from);
    let state = if v["merged"] == true {
        "merged"
    } else {
        v["state"].as_str().unwrap_or("open")
    };
    let mut changed = false;
    if head != s.head {
        s.head = head;
        s.runs.clear();
        s.statuses.clear();
        s.head_workflows = None;
        changed = true;
    }
    if s.pr_state.as_deref() != Some(state) {
        s.pr_state = Some(state.to_string());
        changed = true;
    }
    if github {
        // `mergeable` is null and `mergeable_state` `unknown` while GitHub
        // computes them, after a push to the head or the base.
        let merge = match (v["mergeable"].as_bool(), v["mergeable_state"].as_str()) {
            (Some(false), _) => Some("dirty"),
            (_, m) => m,
        }
        .map(String::from);
        let head_ref = v["head"]["ref"].as_str().map(String::from);
        if (&merge, &head_ref) != (&s.merge_state, &s.head_ref) {
            (s.merge_state, s.head_ref) = (merge, head_ref);
            changed = true;
        }
    }
    Ok(changed)
}

fn poll_github(s: &mut State, now: i64, fetch: &mut Fetch) -> Result<bool, PollError> {
    let t = s
        .target
        .clone()
        .ok_or(PollError::Other("state without target".into()))?;
    let repo = &t.repo;
    let mut old = s.etags.clone();
    if s.head_workflows.is_none() {
        // An old cache or a failed probe has no body to reuse on a 304.
        old.retain(|path, _| !path.contains("/actions/runs?head_sha="));
    }
    let mut req = Req {
        forge: t.forge,
        host: &t.host,
        old: &old,
        used: BTreeMap::new(),
        fetch,
        requests: 0,
    };
    let mut changed = false;

    let head_ref = match (t.pr, &t.branch) {
        (Some(n), _) => {
            match req.get(format!("repos/{repo}/pulls/{n}"))? {
                Got::Fresh(body) => changed |= apply_pull(s, &body, true)?,
                Got::Same => {}
                Got::Status(404) => {
                    s.ended = Some("not found".into());
                    return Ok(true);
                }
                Got::Status(st) => return Err(PollError::Other(format!("pull: HTTP {st}"))),
            }
            if let Some(state @ ("merged" | "closed")) = s.pr_state.as_deref() {
                s.ended = Some(state.to_string());
                s.etags = req.used;
                return Ok(true);
            }
            s.head
                .clone()
                .ok_or(PollError::Other("pull: no head sha".into()))?
        }
        (None, Some(b)) => escape(b),
        (None, None) => return Err(PollError::Other("target without PR or branch".into())),
    };

    match req.get(format!("repos/{repo}/commits/{head_ref}/status"))? {
        Got::Fresh(body) => {
            let (statuses, sha) = parse_status(&body).map_err(PollError::Other)?;
            if t.pr.is_none() && sha.is_some() && sha != s.head {
                s.head = sha;
                s.head_workflows = None;
                changed = true;
            }
            if statuses != s.statuses {
                s.statuses = statuses;
                changed = true;
            }
        }
        Got::Same => {}
        // Gone only once seen: a watch may start before the push.
        Got::Status(404 | 422) if t.pr.is_none() && s.head.is_some() => {
            s.ended = Some("branch gone".into());
            return Ok(true);
        }
        Got::Status(404 | 422) if t.pr.is_none() => {
            return Err(PollError::Other(
                "branch not on GitHub yet (push it)".into(),
            ));
        }
        Got::Status(st) => return Err(PollError::Other(format!("status: HTTP {st}"))),
    }
    match req.get(format!(
        "repos/{repo}/commits/{head_ref}/check-runs?per_page=100"
    ))? {
        Got::Fresh(body) => {
            let (runs, _) = parse_check_runs(&body).map_err(PollError::Other)?;
            if runs != s.runs {
                s.runs = runs;
                changed = true;
            }
        }
        Got::Same => {}
        Got::Status(st) => return Err(PollError::Other(format!("check-runs: HTTP {st}"))),
    }
    // Pending job checks already prevent green. Otherwise ask the head's
    // workflows too: a queued run may not have registered any job checks yet.
    // Keep the branch-wide zombie probe independent of these run buckets.
    let checks_pending = s
        .runs
        .iter()
        .chain(&s.statuses)
        .any(|c| c.bucket == Bucket::Pending);
    if !checks_pending {
        let head = s
            .head
            .as_deref()
            .ok_or(PollError::Other("no head sha".into()))?;
        match req.get(format!(
            "repos/{repo}/actions/runs?head_sha={}&per_page=100",
            escape(head)
        )) {
            Ok(Got::Fresh(body)) => {
                let previous = s.head_workflows.take();
                let runs = head_workflows(&body, head)?;
                if previous.as_ref() != Some(&runs) {
                    changed = true;
                }
                s.head_workflows = Some(runs);
            }
            Ok(Got::Same) if s.head_workflows.is_some() => {}
            other => {
                s.head_workflows = None;
                return Err(match other {
                    Err(e) => e,
                    Ok(Got::Status(st)) => PollError::Other(format!("workflow runs: HTTP {st}")),
                    _ => PollError::Other("workflow runs unavailable".into()),
                });
            }
        }
        if let Some(b) = t.branch.clone().or_else(|| s.head_ref.clone()) {
            changed |= find_zombies(s, &mut req, &b, now)?;
        }
    }
    s.etags = req.used;
    s.polled = now;
    if changed {
        s.changed = now;
    }
    Ok(changed)
}

/// One page per head at most. Never call a truncated list green: the unseen
/// runs may still be queued. Filter defensively so an older head cannot vote.
fn head_workflows(body: &str, head: &str) -> Result<Vec<Check>, PollError> {
    let v: Value =
        serde_json::from_str(body).map_err(|e| PollError::Other(format!("runs: {e}")))?;
    let runs = v["workflow_runs"]
        .as_array()
        .ok_or(PollError::Other("runs: no workflow_runs".into()))?;
    let mut checks: Vec<_> = runs
        .iter()
        .filter(|r| r["head_sha"].as_str() == Some(head))
        .map(|r| Check {
            name: format!(
                "workflow: {} (run {})",
                r["name"].as_str().unwrap_or("?"),
                r["id"]
            ),
            bucket: Bucket::of_run(r["status"].as_str().unwrap_or(""), r["conclusion"].as_str()),
            note: None,
        })
        .collect();
    if v["total_count"].as_u64() != Some(runs.len() as u64) {
        checks.push(Check {
            name: "workflow runs (incomplete list)".into(),
            bucket: Bucket::Pending,
            note: None,
        });
    }
    Ok(checks)
}

/// The zombie check of a PR or branch, asked only once its checks are no
/// longer pending (a zombie's jobs all completed): the branch's newest runs
/// (one conditional request), then for an open run that stayed open
/// [`ZOMBIE_AFTER`] its jobs, at most once per [`ZOMBIE_AFTER`]. The runs of
/// the branch, not of the head: a zombie of an older head holds the
/// concurrency group of the new one. True if the zombies changed.
fn find_zombies(s: &mut State, req: &mut Req, branch: &str, now: i64) -> Result<bool, PollError> {
    let repo = s
        .target
        .as_ref()
        .map(|t| t.repo.clone())
        .unwrap_or_default();
    match req.get(format!(
        "repos/{repo}/actions/runs?branch={}&per_page=20",
        escape(branch)
    ))? {
        Got::Fresh(body) => s.open_runs = open_runs(&body)?,
        Got::Same => {}
        // No run list for this repo (not verified when GitHub says so): no zombies.
        Got::Status(404) => s.open_runs.clear(),
        Got::Status(st) => return Err(PollError::Other(format!("runs: HTTP {st}"))),
    }
    let was = s.zombies.clone();
    s.stuck.retain(|id, _| s.open_runs.contains(id));
    s.zombies.retain(|id| s.open_runs.contains(id));
    for id in s.open_runs.clone() {
        if s.zombies.contains(&id) {
            continue;
        }
        let since = *s.stuck.entry(id).or_insert(now);
        if now - since < ZOMBIE_AFTER {
            continue;
        }
        match req.get(format!("repos/{repo}/actions/runs/{id}/jobs?per_page=100"))? {
            Got::Fresh(body) if jobs_done(&body)? => s.zombies.push(id),
            // Still running (or unchanged since the last check): ask again later.
            Got::Fresh(_) | Got::Same | Got::Status(404) => {
                s.stuck.insert(id, now);
            }
            Got::Status(st) => return Err(PollError::Other(format!("jobs: HTTP {st}"))),
        }
    }
    Ok(s.zombies != was)
}

/// The ids of the `queued` and `in_progress` runs in an `actions/runs` body.
fn open_runs(body: &str) -> Result<Vec<u64>, PollError> {
    let v: Value =
        serde_json::from_str(body).map_err(|e| PollError::Other(format!("runs: {e}")))?;
    let runs = v["workflow_runs"]
        .as_array()
        .ok_or(PollError::Other("runs: no workflow_runs".into()))?;
    Ok(runs
        .iter()
        .filter(|r| OPEN_RUN.contains(&r["status"].as_str().unwrap_or("")))
        .filter_map(|r| r["id"].as_u64())
        .collect())
}

/// True when an `actions/runs/<id>/jobs` body lists all the run's jobs
/// (`total_count`) and each one completed. No jobs at all counts too.
fn jobs_done(body: &str) -> Result<bool, PollError> {
    let v: Value =
        serde_json::from_str(body).map_err(|e| PollError::Other(format!("jobs: {e}")))?;
    let jobs = v["jobs"]
        .as_array()
        .ok_or(PollError::Other("jobs: no jobs".into()))?;
    Ok(v["total_count"].as_u64() == Some(jobs.len() as u64)
        && jobs.iter().all(|j| j["status"] == "completed"))
}

/// Forgejo/Gitea: no check runs, no ETags, no rate headers. A PR is
/// `pulls/n` (head, state) and a branch `branches/b` (head), then the commit
/// statuses of the head (Pipelines-as-Code and Forgejo Actions report there).
fn poll_forgejo(s: &mut State, now: i64, fetch: &mut Fetch) -> Result<bool, PollError> {
    let t = s
        .target
        .clone()
        .ok_or(PollError::Other("state without target".into()))?;
    let repo = &t.repo;
    let old = BTreeMap::new();
    let mut req = Req {
        forge: t.forge,
        host: &t.host,
        old: &old,
        used: BTreeMap::new(),
        fetch,
        requests: 0,
    };
    let mut changed = false;

    match (t.pr, &t.branch) {
        (Some(n), _) => {
            match req.get(format!("repos/{repo}/pulls/{n}"))? {
                Got::Fresh(body) => changed |= apply_pull(s, &body, false)?,
                Got::Same => {}
                Got::Status(404) => {
                    s.ended = Some("not found".into());
                    return Ok(true);
                }
                Got::Status(st) => return Err(PollError::Other(format!("pull: HTTP {st}"))),
            }
            if let Some(state @ ("merged" | "closed")) = s.pr_state.as_deref() {
                s.ended = Some(state.to_string());
                return Ok(true);
            }
        }
        (None, Some(b)) => match req.get(format!("repos/{repo}/branches/{}", escape(b)))? {
            Got::Fresh(body) => {
                let v: Value = serde_json::from_str(&body)
                    .map_err(|e| PollError::Other(format!("branch: {e}")))?;
                let head = v["commit"]["id"].as_str().map(String::from);
                if head.is_some() && head != s.head {
                    s.head = head;
                    s.runs.clear();
                    s.statuses.clear();
                    changed = true;
                }
            }
            Got::Same => {}
            // Gone only once seen: a watch may start before the push.
            Got::Status(404) if s.head.is_some() => {
                s.ended = Some("branch gone".into());
                return Ok(true);
            }
            Got::Status(404) => {
                return Err(PollError::Other(format!(
                    "branch not on {} yet (push it)",
                    t.host
                )))
            }
            Got::Status(st) => return Err(PollError::Other(format!("branch: HTTP {st}"))),
        },
        (None, None) => return Err(PollError::Other("target without PR or branch".into())),
    }
    let head = s
        .head
        .clone()
        .ok_or(PollError::Other("no head sha".into()))?;
    match req.get(format!("repos/{repo}/commits/{head}/status?limit=50"))? {
        Got::Fresh(body) => {
            let (statuses, _) = parse_status_forgejo(&body).map_err(PollError::Other)?;
            if statuses != s.statuses {
                s.statuses = statuses;
                changed = true;
            }
        }
        Got::Same => {}
        Got::Status(st) => return Err(PollError::Other(format!("status: HTTP {st}"))),
    }
    s.polled = now;
    if changed {
        s.changed = now;
    }
    Ok(changed)
}

/// The text of a signal key (see [`State::signal_keys`]).
fn signal_text(s: &State, t: &Target, title: &str, key: &str) -> String {
    let short: String = s.head.as_deref().unwrap_or("").chars().take(7).collect();
    let show = format!("vega ci show {}", t.label());
    if let Some(id) = key.strip_prefix("zombie:") {
        return format!(
            "ci: {title}: run {id} is still queued or in progress though all its jobs completed \
             ({} min and more); it holds its concurrency group. vega does not cancel it; \
             to force-cancel: gh api -X POST repos/{}/actions/runs/{id}/force-cancel",
            ZOMBIE_AFTER / 60,
            t.repo
        );
    }
    if s.merge_state.as_deref() == Some("behind") {
        return format!(
            "ci: {title} head {short}: behind its base, which requires up-to-date branches; \
             rebase onto the base before the merge. {show}"
        );
    }
    format!(
        "ci: {title} head {short}: conflicts with its base (CONFLICTING); rebase onto the base \
         and resolve them before you push again, CI on this head cannot merge. {show}"
    )
}

/// The mails `s` owes its watchers: one per new (head, verdict) when green or
/// red, one per signal (rebase needed, zombie run), one when the target
/// ended. Marks them sent in `s.notified` and `s.signals`.
pub fn mails(s: &mut State, panes: &[String]) -> Vec<(String, String)> {
    let Some(t) = s.target.clone() else {
        return Vec::new();
    };
    let title = t.title_of(s.run_id);
    if let Some(why) = &s.ended {
        let text = format!("ci: {title} {why}; watch ended");
        return panes.iter().map(|p| (p.clone(), text.clone())).collect();
    }
    let mut out = Vec::new();
    let signals: Vec<(String, String)> = s
        .signal_keys()
        .into_iter()
        .map(|k| {
            let text = signal_text(s, &t, &title, &k);
            (k, text)
        })
        .collect();
    // A merge state of this head stays sent though GitHub says `unknown` for a
    // while after each push to the base; an older head or an ended zombie
    // goes, so its return mails again.
    let this_head = format!("merge:{}:", s.head.as_deref().unwrap_or(""));
    for p in panes {
        let sent = s.signals.entry(p.clone()).or_default();
        sent.retain(|k| k.starts_with(&this_head) || signals.iter().any(|(key, _)| key == k));
        for (k, text) in &signals {
            if sent.insert(k.clone()) {
                out.push((p.clone(), text.clone()));
            }
        }
    }
    s.signals.retain(|_, sent| !sent.is_empty());
    let sum = s.summary();
    if sum.verdict == Verdict::Green && s.error.is_some() {
        return out;
    }
    if !sum.verdict.settled() {
        return out;
    }
    let head = s.head.clone().unwrap_or_default();
    let short: String = head.chars().take(7).collect();
    let key = format!("{head}:{}", sum.verdict.as_str());
    let text = match sum.verdict {
        Verdict::Red => {
            let mut names = sum
                .failed
                .iter()
                .take(FAILED_SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if sum.failed.len() > FAILED_SHOWN {
                names.push_str(&format!(" +{} more", sum.failed.len() - FAILED_SHOWN));
            }
            let pending = sum.count(crate::Bucket::Pending);
            let pending = if pending > 0 {
                format!("; {pending} pending")
            } else {
                String::new()
            };
            format!(
                "ci: {title} head {short}: red, failed: {names}{pending}. vega ci show {}",
                t.label()
            )
        }
        _ => format!(
            "ci: {title} head {short}: green ({}). vega ci show {}",
            sum.counts_text(),
            t.label()
        ),
    };
    for p in panes {
        if s.notified.get(p) != Some(&key) {
            s.notified.insert(p.clone(), key.clone());
            out.push((p.clone(), text.clone()));
        }
    }
    out
}

/// When to poll next: by the verdict, slower on a low rate limit, not before
/// the reset on a very low one.
pub fn next_poll(verdict: Verdict, rate: Option<&Rate>, now: i64) -> i64 {
    let base = if verdict.settled() {
        SETTLED_EVERY
    } else {
        PENDING_EVERY
    };
    match rate.filter(|r| r.reset > now) {
        Some(r) if r.remaining < STOP_BELOW => r.reset + 1,
        Some(r) if r.remaining < SLOW_BELOW => now + base * 4,
        _ => now + base,
    }
}

/// True while the rate limit is too low to poll at all.
pub fn paused(rate: Option<&Rate>, now: i64) -> bool {
    rate.is_some_and(|r| r.reset > now && r.remaining < STOP_BELOW)
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// (pane, text) to mail.
    pub mails: Vec<(String, String)>,
    /// Lines for vegad's log.
    pub log: Vec<String>,
}

/// One cycle over all watches: drops watches of panes `live` says are gone,
/// polls due targets once each however many panes watch them, saves the
/// cache and returns the mails.
pub fn cycle(now: i64, live: &dyn Fn(&str) -> bool, fetch: &mut Fetch) -> Outcome {
    let mut out = Outcome::default();
    // Per target: its watching panes and the earliest watch.
    let mut groups: BTreeMap<String, (Target, Vec<String>, i64)> = BTreeMap::new();
    for Watch {
        target,
        pane,
        since,
    } in store::watches()
    {
        if !live(&pane) {
            store::remove_watch(&target, &pane);
            out.log.push(format!(
                "ci: {} unwatched, pane {pane} gone",
                target.label()
            ));
            continue;
        }
        let g = groups
            .entry(target.key())
            .or_insert_with(|| (target, Vec::new(), since));
        g.1.push(pane);
        g.2 = g.2.min(since);
    }
    for (t, panes, since) in groups.values() {
        let mut s = store::load_state(t)
            .filter(|s| s.target.as_ref() == Some(t))
            .unwrap_or_else(|| State {
                since: *since,
                ..State::new(t, now)
            });
        // The rate headers are GitHub's: Forgejo sends none.
        let rate = t.forge.is_github().then(store::load_rate).flatten();
        if s.ended.is_none() && now >= s.next && !paused(rate.as_ref(), now) {
            let was = s.summary().verdict;
            match poll(&mut s, now, fetch) {
                Ok(changed) => {
                    s.error = None;
                    let verdict = s.summary().verdict;
                    let rate = t.forge.is_github().then(store::load_rate).flatten();
                    s.next = next_poll(verdict, rate.as_ref(), now);
                    if changed && verdict != was {
                        out.log
                            .push(format!("ci: {} {}", t.label(), verdict.as_str()));
                    }
                }
                Err(PollError::Limited(secs)) => {
                    s.error = Some(format!("rate limited, next poll in {secs}s"));
                    s.next = now + secs;
                    out.log.push(format!(
                        "ci: {} {}",
                        t.label(),
                        s.error.as_deref().unwrap_or("")
                    ));
                }
                Err(PollError::Other(e)) => {
                    if s.error.as_deref() != Some(e.as_str()) {
                        out.log.push(format!("ci: {} poll failed: {e}", t.label()));
                    }
                    s.error = Some(e);
                    s.next = now + 2 * PENDING_EVERY;
                }
            }
        }
        if s.ended.is_none() && now - s.changed > IDLE_END {
            s.ended = Some("no change for 3 days".into());
        }
        let mails = mails(&mut s, panes);
        for (p, text) in &mails {
            out.log.push(format!("ci: mail to {p}: {text}"));
        }
        out.mails.extend(mails);
        // A completed run has nothing more to say once its verdict is mailed
        // (a cancelled one ended in the poll, with its own mail).
        if s.ended.is_none()
            && t.is_run()
            && s.run_status.as_deref() == Some("completed")
            && !panes.iter().any(|p| store::owed(p, Some(&s), now))
        {
            out.log
                .push(format!("ci: {} completed; watch ended", t.label()));
            s.ended = Some("completed".into());
        }
        if s.ended.is_some() {
            for p in panes {
                store::remove_watch(t, p);
            }
            store::remove_state(t);
        } else {
            store::save_state(t, &s);
        }
    }
    // Caches of targets nobody watches any more.
    let keys: BTreeSet<&String> = groups.keys().collect();
    if let Some(entries) = store::dir().and_then(|d| std::fs::read_dir(d.join("state")).ok()) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(k) = name.strip_suffix(".json") {
                if !keys.contains(&k.to_string()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
    out
}
