//! Check runs and commit statuses in gh-axi's buckets
//! (repos/gh-axi/src/commands/pr.ts), and the verdict of one head.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bucket {
    Pass,
    Fail,
    Pending,
    Skip,
    Cancel,
}

/// The order of `vega ci`'s columns.
pub const BUCKETS: [Bucket; 5] = [
    Bucket::Pass,
    Bucket::Fail,
    Bucket::Pending,
    Bucket::Skip,
    Bucket::Cancel,
];

impl Bucket {
    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Pass => "pass",
            Bucket::Fail => "fail",
            Bucket::Pending => "pending",
            Bucket::Skip => "skip",
            Bucket::Cancel => "cancel",
        }
    }

    /// A check run: `status` until completed, then its `conclusion`.
    pub fn of_run(status: &str, conclusion: Option<&str>) -> Bucket {
        if status != "completed" {
            return Bucket::Pending;
        }
        match conclusion.unwrap_or("") {
            "success" | "neutral" => Bucket::Pass,
            "failure" | "timed_out" | "action_required" | "startup_failure" => Bucket::Fail,
            "skipped" => Bucket::Skip,
            // A cancelled run says nothing about the code (gh-axi).
            "cancelled" => Bucket::Cancel,
            // stale and unknown: not a verdict for this head yet.
            _ => Bucket::Pending,
        }
    }

    /// A commit status `state`. Gitea's `warning` is not a failure: pass,
    /// like GitHub's `neutral`.
    pub fn of_status(state: &str) -> Bucket {
        match state {
            "success" | "warning" => Bucket::Pass,
            "failure" | "error" => Bucket::Fail,
            _ => Bucket::Pending,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub bucket: Bucket,
    /// The status description, kept for Forgejo (a Pipelines-as-Code run waiting for
    /// `/ok-to-test` says so there and nowhere else).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `GET commits/<ref>/check-runs`: the checks and the head sha they ran on.
pub fn parse_check_runs(body: &str) -> Result<(Vec<Check>, Option<String>), String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("check-runs: {e}"))?;
    let runs = v["check_runs"]
        .as_array()
        .ok_or("check-runs: no check_runs")?;
    let head = runs
        .first()
        .and_then(|r| r["head_sha"].as_str())
        .map(String::from);
    let checks = runs
        .iter()
        .map(|r| Check {
            name: r["name"].as_str().unwrap_or("?").to_string(),
            bucket: Bucket::of_run(r["status"].as_str().unwrap_or(""), r["conclusion"].as_str()),
            note: None,
        })
        .collect();
    Ok((checks, head))
}

/// `GET actions/runs/<id>/jobs`: the jobs of a run, in the check-run buckets.
pub fn parse_jobs(body: &str) -> Result<Vec<Check>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("jobs: {e}"))?;
    let jobs = v["jobs"].as_array().ok_or("jobs: no jobs")?;
    Ok(jobs
        .iter()
        .map(|j| Check {
            name: j["name"].as_str().unwrap_or("?").to_string(),
            bucket: Bucket::of_run(j["status"].as_str().unwrap_or(""), j["conclusion"].as_str()),
            note: None,
        })
        .collect())
}

/// `GET commits/<ref>/status`: the commit statuses and the ref's sha. The
/// combined `state` is ignored: it is `pending` when there are no statuses.
pub fn parse_status(body: &str) -> Result<(Vec<Check>, Option<String>), String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("status: {e}"))?;
    let statuses = v["statuses"].as_array().ok_or("status: no statuses")?;
    let checks = statuses
        .iter()
        .map(|s| Check {
            name: s["context"].as_str().unwrap_or("?").to_string(),
            bucket: Bucket::of_status(s["state"].as_str().unwrap_or("")),
            note: None,
        })
        .collect();
    Ok((checks, v["sha"].as_str().map(String::from)))
}

/// Forgejo/Gitea `GET repos/{o}/{r}/commits/{ref}/status`: the same shape as
/// GitHub's, but each status carries its state in `status` (not `state`), and
/// a commit without statuses answers `statuses: null` and `sha: ""`.
pub fn parse_status_forgejo(body: &str) -> Result<(Vec<Check>, Option<String>), String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("status: {e}"))?;
    let checks = v["statuses"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|s| Check {
            name: s["context"].as_str().unwrap_or("?").to_string(),
            bucket: Bucket::of_status(s["status"].as_str().unwrap_or("")),
            note: s["description"]
                .as_str()
                .filter(|d| !d.is_empty())
                .map(String::from),
        })
        .collect();
    let sha = v["sha"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok((checks, sha))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// All checks finished, none failed, at least one passed.
    Green,
    /// At least one check failed (even while others still run).
    Red,
    Pending,
    /// No checks, or only skipped and cancelled ones.
    None,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Green => "green",
            Verdict::Red => "red",
            Verdict::Pending => "pending",
            Verdict::None => "none",
        }
    }

    /// Green and red are worth a mail; pending and none are not.
    pub fn settled(self) -> bool {
        matches!(self, Verdict::Green | Verdict::Red)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub verdict: Verdict,
    /// Per bucket, in [`BUCKETS`] order.
    pub counts: [usize; 5],
    pub failed: Vec<String>,
}

impl Summary {
    pub fn count(&self, b: Bucket) -> usize {
        self.counts[BUCKETS.iter().position(|x| *x == b).unwrap_or(0)]
    }

    /// `12 pass, 2 skip`: the non-zero buckets.
    pub fn counts_text(&self) -> String {
        BUCKETS
            .iter()
            .zip(self.counts)
            .filter(|(_, n)| *n > 0)
            .map(|(b, n)| format!("{n} {}", b.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn summarize(checks: &[Check]) -> Summary {
    let mut counts = [0; 5];
    for c in checks {
        counts[BUCKETS.iter().position(|b| *b == c.bucket).unwrap_or(0)] += 1;
    }
    let mut failed: Vec<String> = checks
        .iter()
        .filter(|c| c.bucket == Bucket::Fail)
        .map(|c| c.name.clone())
        .collect();
    failed.dedup();
    let [pass, fail, pending, ..] = counts;
    let verdict = if fail > 0 {
        Verdict::Red
    } else if pending > 0 {
        Verdict::Pending
    } else if pass > 0 {
        Verdict::Green
    } else {
        Verdict::None
    };
    Summary {
        verdict,
        counts,
        failed,
    }
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

    fn c(name: &str, bucket: Bucket) -> Check {
        Check {
            name: name.into(),
            bucket,
            note: None,
        }
    }

    #[test]
    fn recorded_check_runs_and_status_parse() {
        // Recorded 2026-10-10 from cli/cli trunk, trimmed.
        let (runs, head) = parse_check_runs(&fixture("check-runs.json")).unwrap();
        assert_eq!(runs.len(), 8);
        assert_eq!(
            head.as_deref(),
            Some("ec5b512045db67e5a2a4ff4a1b02660b2fb24390")
        );
        let s = summarize(&runs);
        assert_eq!(s.verdict, Verdict::Green);
        assert_eq!(s.counts_text(), "6 pass, 2 skip");
        // No statuses: the combined state says pending, which must not count.
        let (statuses, sha) = parse_status(&fixture("status.json")).unwrap();
        assert!(statuses.is_empty());
        assert_eq!(sha, head);
    }

    #[test]
    fn run_jobs_parse() {
        let jobs = parse_jobs(&fixture("jobs.json")).unwrap();
        assert_eq!(
            jobs,
            vec![
                c("build", Bucket::Pass),
                c("deploy (prod)", Bucket::Pending)
            ]
        );
        assert!(parse_jobs("{}").is_err());
    }

    #[test]
    fn recorded_forgejo_statuses_parse() {
        // Recorded 2026-10-12 from git.b4mad.industries (goern/rs.cms PR 103,
        // Tekton Pipelines-as-Code), trimmed.
        let (checks, sha) = parse_status_forgejo(&fixture("tea-status-pac.stdout")).unwrap();
        assert_eq!(
            checks.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            [
                "op1st Pipelines as Code / cms-on-pr",
                "op1st Pipelines as Code / cms-on-pr-gate"
            ]
        );
        assert_eq!(summarize(&checks).verdict, Verdict::Green);
        assert_eq!(
            sha.as_deref(),
            Some("8173ddcb042096b2471eeb953580a633f1e9d32e")
        );
        // No statuses: `statuses: null`, `sha: ""`.
        let (none, sha) = parse_status_forgejo(&fixture("tea-status-none.stdout")).unwrap();
        assert!(none.is_empty());
        assert_eq!(sha, None);
        let pending = r#"{"statuses":[{"context":"pac","status":"pending","description":"Pending approval, waiting for an /ok-to-test"}]}"#;
        let (p, _) = parse_status_forgejo(pending).unwrap();
        assert_eq!(
            p[0].note.as_deref(),
            Some("Pending approval, waiting for an /ok-to-test")
        );
        assert_eq!(summarize(&p).verdict, Verdict::Pending);
    }

    #[test]
    fn buckets_follow_gh_axi() {
        assert_eq!(Bucket::of_run("in_progress", None), Bucket::Pending);
        assert_eq!(Bucket::of_run("completed", Some("neutral")), Bucket::Pass);
        assert_eq!(Bucket::of_run("completed", Some("timed_out")), Bucket::Fail);
        assert_eq!(
            Bucket::of_run("completed", Some("cancelled")),
            Bucket::Cancel
        );
        assert_eq!(Bucket::of_run("completed", Some("stale")), Bucket::Pending);
        assert_eq!(Bucket::of_status("error"), Bucket::Fail);
        assert_eq!(Bucket::of_status("warning"), Bucket::Pass);
        assert_eq!(Bucket::of_status("pending"), Bucket::Pending);
    }

    #[test]
    fn verdicts() {
        use Bucket::*;
        let red = summarize(&[c("test", Fail), c("lint", Pending), c("build", Pass)]);
        assert_eq!(
            (red.verdict, red.failed.clone()),
            (Verdict::Red, vec!["test".to_string()])
        );
        assert_eq!(red.count(Pending), 1);
        assert_eq!(
            summarize(&[c("a", Pass), c("b", Pending)]).verdict,
            Verdict::Pending
        );
        assert_eq!(
            summarize(&[c("a", Pass), c("b", Cancel)]).verdict,
            Verdict::Green
        );
        assert_eq!(
            summarize(&[c("a", Skip), c("b", Cancel)]).verdict,
            Verdict::None
        );
        assert_eq!(summarize(&[]).verdict, Verdict::None);
    }
}
