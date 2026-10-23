//! The poll cycle against a fake GitHub that honours If-None-Match like the
//! real one (304 when the ETag matches; verified live, see gh.rs).

use std::cell::RefCell;
use std::collections::HashMap;

use vega_ci_watch::forge::Forge;
use vega_ci_watch::gh::Resp;
use vega_ci_watch::poll::{self, cycle, next_poll, paused, PENDING_EVERY, SETTLED_EVERY};
use vega_ci_watch::store::{self, Rate, Watch};
use vega_ci_watch::{Target, Verdict};

/// The store lives in XDG_STATE_HOME, which is process-wide: tests take turns.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
struct Fake {
    /// path -> (status, etag, body)
    pages: HashMap<String, (u16, String, String)>,
    requests: usize,
    full: usize,
}

impl Fake {
    fn set(&mut self, path: &str, body: &str) {
        let etag = format!(
            "\"{:x}\"",
            body.len() * 31 + body.bytes().map(|b| b as usize).sum::<usize>()
        );
        self.pages.insert(path.into(), (200, etag, body.into()));
    }

    fn get(&mut self, path: &str, etag: Option<&str>) -> Result<Resp, String> {
        self.requests += 1;
        let (status, tag, body) = self.pages.get(path).cloned().unwrap_or_else(|| {
            // Most tests exercise checks without Actions workflows.
            if path.contains("/actions/runs?head_sha=") {
                (
                    200,
                    "\"no-workflows\"".into(),
                    r#"{"total_count":0,"workflow_runs":[]}"#.into(),
                )
            } else {
                (404, String::new(), String::new())
            }
        });
        if status == 200 && etag == Some(tag.as_str()) {
            return Ok(Resp {
                status: 304,
                etag: Some(tag),
                ..Resp::default()
            });
        }
        self.full += 1;
        Ok(Resp {
            status,
            etag: Some(tag),
            body,
            ..Resp::default()
        })
    }
}

fn pull(sha: &str, state: &str, merged: bool) -> String {
    format!(r#"{{"state":"{state}","merged":{merged},"head":{{"sha":"{sha}"}}}}"#)
}

fn runs(list: &[(&str, &str, Option<&str>)]) -> String {
    let runs: Vec<String> = list
        .iter()
        .map(|(n, s, c)| {
            let c = c.map(|c| format!("\"{c}\"")).unwrap_or("null".into());
            format!(r#"{{"name":"{n}","status":"{s}","conclusion":{c},"head_sha":"x"}}"#)
        })
        .collect();
    format!(
        r#"{{"total_count":{},"check_runs":[{}]}}"#,
        runs.len(),
        runs.join(",")
    )
}

const NO_STATUSES: &str = r#"{"state":"pending","sha":"x","statuses":[]}"#;

#[test]
fn two_watchers_one_poller_mails_on_verdicts() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    let t = Target::pr("github.com", "o/r", 42);
    for pane in ["w1:p2", "w1:p3"] {
        assert!(store::add_watch(&Watch {
            target: t.clone(),
            pane: pane.into(),
            since: 0
        }));
    }
    let fake = RefCell::new(Fake::default());
    let mut fetch = |_: Forge, _: &str, p: &str, e: Option<&str>| fake.borrow_mut().get(p, e);
    let live = |_: &str| true;
    {
        let mut f = fake.borrow_mut();
        f.set("repos/o/r/pulls/42", &pull("aaaaaaa1", "open", false));
        f.set("repos/o/r/commits/aaaaaaa1/status", NO_STATUSES);
        f.set(
            "repos/o/r/commits/aaaaaaa1/check-runs?per_page=100",
            &runs(&[
                ("test", "in_progress", None),
                ("lint", "completed", Some("success")),
            ]),
        );
    }

    // Pending: three requests for two watchers, no mail.
    let out = cycle(0, &live, &mut fetch);
    assert!(out.mails.is_empty());
    assert_eq!(fake.borrow().requests, 3);
    let s = store::load_state(&t).unwrap();
    assert_eq!(
        (s.summary().verdict, s.next),
        (Verdict::Pending, PENDING_EVERY)
    );

    // Not due yet: nothing is requested.
    cycle(5, &live, &mut fetch);
    assert_eq!(fake.borrow().requests, 3);

    // Due, nothing changed: three conditional requests, all 304.
    cycle(PENDING_EVERY, &live, &mut fetch);
    assert_eq!((fake.borrow().requests, fake.borrow().full), (6, 3));

    // test fails: one changed page, both watchers get red once.
    fake.borrow_mut().set(
        "repos/o/r/commits/aaaaaaa1/check-runs?per_page=100",
        &runs(&[
            ("test", "completed", Some("failure")),
            ("lint", "completed", Some("success")),
        ]),
    );
    let out = cycle(2 * PENDING_EVERY, &live, &mut fetch);
    assert_eq!((fake.borrow().requests, fake.borrow().full), (10, 5));
    assert_eq!(out.mails.len(), 2);
    assert_eq!(
        out.mails[0],
        (
            "w1:p2".into(),
            "ci: PR #42 (o/r) head aaaaaaa: red, failed: test. vega ci show o/r#42".into()
        )
    );
    let next = store::load_state(&t).unwrap().next;
    assert_eq!(next, 2 * PENDING_EVERY + SETTLED_EVERY);
    assert!(
        cycle(next, &live, &mut fetch).mails.is_empty(),
        "red is mailed once"
    );

    // A new push turns green; one pane is gone meanwhile.
    {
        let mut f = fake.borrow_mut();
        f.set("repos/o/r/pulls/42", &pull("bbbbbbb2", "open", false));
        f.set("repos/o/r/commits/bbbbbbb2/status", NO_STATUSES);
        f.set(
            "repos/o/r/commits/bbbbbbb2/check-runs?per_page=100",
            &runs(&[
                ("test", "completed", Some("success")),
                ("lint", "completed", Some("skipped")),
            ]),
        );
    }
    let only_p2 = |p: &str| p == "w1:p2";
    let out = cycle(1000, &only_p2, &mut fetch);
    assert_eq!(
        out.mails,
        vec![(
            "w1:p2".into(),
            "ci: PR #42 (o/r) head bbbbbbb: green (1 pass, 1 skip). vega ci show o/r#42".into()
        )]
    );
    assert_eq!(store::watches().len(), 1);

    // Merged: the last mail, and the watch and its cache are gone.
    fake.borrow_mut()
        .set("repos/o/r/pulls/42", &pull("bbbbbbb2", "closed", true));
    let out = cycle(2000, &live, &mut fetch);
    assert_eq!(
        out.mails,
        vec![(
            "w1:p2".into(),
            "ci: PR #42 (o/r) merged; watch ended".into()
        )]
    );
    assert!(store::watches().is_empty());
    assert!(store::load_state(&t).is_none());

    // A branch watch waits for the push, then ends when the branch is
    // deleted (404).
    let b = Target::branch("github.com", "o/r", "feat/x");
    store::add_watch(&Watch {
        target: b.clone(),
        pane: "w1:p2".into(),
        since: 0,
    });
    let out = cycle(2500, &live, &mut fetch);
    assert!(out.mails.is_empty(), "not pushed yet is no end");
    assert_eq!(
        store::load_state(&b).unwrap().error.as_deref(),
        Some("branch not on GitHub yet (push it)")
    );
    fake.borrow_mut().set(
        "repos/o/r/commits/feat%2Fx/status",
        r#"{"state":"success","sha":"ccccccc3","statuses":[{"context":"ci/x","state":"success"}]}"#,
    );
    fake.borrow_mut().set(
        "repos/o/r/commits/feat%2Fx/check-runs?per_page=100",
        &runs(&[]),
    );
    let out = cycle(3000, &live, &mut fetch);
    assert_eq!(
        out.mails[0].1,
        "ci: o/r@feat/x head ccccccc: green (1 pass). vega ci show o/r@feat/x"
    );
    fake.borrow_mut()
        .pages
        .remove("repos/o/r/commits/feat%2Fx/status");
    let out = cycle(4000, &live, &mut fetch);
    assert_eq!(out.mails[0].1, "ci: o/r@feat/x branch gone; watch ended");

    // 403: the target pauses, no mail.
    store::add_watch(&Watch {
        target: t.clone(),
        pane: "w1:p2".into(),
        since: 0,
    });
    let mut limited = |_: Forge, _: &str, _: &str, _: Option<&str>| {
        Ok(Resp {
            status: 403,
            retry_after: Some(120),
            ..Resp::default()
        })
    };
    let out = cycle(5000, &live, &mut limited);
    assert!(out.mails.is_empty());
    assert_eq!(store::load_state(&t).unwrap().next, 5120);
}

#[test]
fn backoff_follows_the_remaining_limit() {
    let _turn = serial();
    let rate = |remaining| Rate {
        limit: 5000,
        remaining,
        reset: 1000,
        at: 0,
        user_etag: None,
    };
    assert_eq!(next_poll(Verdict::Pending, None, 0), PENDING_EVERY);
    assert_eq!(
        next_poll(Verdict::Green, Some(&rate(4000)), 0),
        SETTLED_EVERY
    );
    assert_eq!(
        next_poll(Verdict::Pending, Some(&rate(900)), 0),
        4 * PENDING_EVERY
    );
    assert_eq!(next_poll(Verdict::Pending, Some(&rate(100)), 0), 1001);
    // After the reset the old numbers say nothing.
    assert_eq!(
        next_poll(Verdict::Pending, Some(&rate(100)), 2000),
        2000 + PENDING_EVERY
    );
    assert!(paused(Some(&rate(100)), 0));
    assert!(!paused(Some(&rate(100)), 1000));
    let _ = poll::IDLE_END;
}

/// Forgejo: no ETags (every request is a full answer), commit statuses only,
/// the head of a branch from `branches/b`, 429 as the only limit.
#[test]
fn forgejo_pr_and_branch_polls_by_statuses() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    let host = "git.example.org";
    let pr = Target::pr(host, "o/r", 7).on(Forge::Forgejo);
    let br = Target::branch(host, "o/r", "feat/x").on(Forge::Forgejo);
    for t in [&pr, &br] {
        assert!(store::add_watch(&Watch {
            target: t.clone(),
            pane: "w1:p2".into(),
            since: 0
        }));
    }
    let status = |list: &[(&str, &str)]| {
        let items: Vec<String> = list
            .iter()
            .map(|(c, s)| format!(r#"{{"context":"{c}","status":"{s}"}}"#))
            .collect();
        format!(
            r#"{{"total_count":{},"statuses":[{}]}}"#,
            items.len(),
            items.join(",")
        )
    };
    let fake = RefCell::new(Fake::default());
    let calls = RefCell::new(Vec::new());
    let mut fetch = |forge: Forge, h: &str, p: &str, e: Option<&str>| {
        assert_eq!((forge, h, e), (Forge::Forgejo, host, None), "{p}");
        calls.borrow_mut().push(p.to_string());
        fake.borrow_mut().get(p, e)
    };
    let live = |_: &str| true;
    {
        let mut f = fake.borrow_mut();
        f.set("repos/o/r/pulls/7", &pull("bbbbbbb1", "open", false));
        // Forgejo takes the escaped slash (codeberg.org, branches/v11.0%2Fforgejo: 200, 2026-10-12).
        f.set(
            "repos/o/r/branches/feat%2Fx",
            r#"{"commit":{"id":"ccccccc1"}}"#,
        );
        f.set(
            "repos/o/r/commits/bbbbbbb1/status?limit=50",
            &status(&[("pac / test", "pending")]),
        );
        // Gitea answers a commit without statuses like this.
        f.set(
            "repos/o/r/commits/ccccccc1/status?limit=50",
            r#"{"state":"","sha":"","total_count":0,"statuses":null}"#,
        );
    }

    // Two requests per PR, two per branch; nothing to mail yet.
    let out = cycle(0, &live, &mut fetch);
    assert!(out.mails.is_empty(), "{:?}", out.mails);
    assert_eq!(calls.borrow().len(), 4, "{:?}", calls.borrow());
    let s = store::load_state(&pr).unwrap();
    assert_eq!(
        (s.summary().verdict, s.next),
        (Verdict::Pending, PENDING_EVERY)
    );
    assert_eq!(
        store::load_state(&br).unwrap().head.as_deref(),
        Some("ccccccc1")
    );

    // The PR goes green: one mail, the branch (no statuses) stays silent.
    fake.borrow_mut().set(
        "repos/o/r/commits/bbbbbbb1/status?limit=50",
        &status(&[("pac / test", "success"), ("pac / lint", "warning")]),
    );
    let out = cycle(PENDING_EVERY, &live, &mut fetch);
    assert_eq!(out.mails.len(), 1, "{:?}", out.mails);
    let (pane, text) = &out.mails[0];
    assert_eq!(pane, "w1:p2");
    assert!(
        text.contains("PR #7 (o/r) head bbbbbbb: green (2 pass)"),
        "{text}"
    );
    assert_eq!(
        store::load_state(&br).unwrap().summary().verdict,
        Verdict::None
    );

    // A failed status mails red with its name.
    fake.borrow_mut().set(
        "repos/o/r/commits/ccccccc1/status?limit=50",
        &status(&[("pac / build", "failure")]),
    );
    let out = cycle(SETTLED_EVERY + PENDING_EVERY, &live, &mut fetch);
    let texts: Vec<_> = out.mails.iter().map(|m| m.1.as_str()).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("o/r@feat/x") && t.contains("red, failed: pac / build")),
        "{texts:?}"
    );

    // Merged: the watch ends with a mail; the branch is gone: it ends too.
    fake.borrow_mut()
        .set("repos/o/r/pulls/7", &pull("bbbbbbb1", "closed", true));
    fake.borrow_mut()
        .pages
        .remove("repos/o/r/branches/feat%2Fx");
    let out = cycle(10_000, &live, &mut fetch);
    let texts: Vec<_> = out.mails.iter().map(|m| m.1.as_str()).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("PR #7 (o/r) merged; watch ended")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("branch gone; watch ended")),
        "{texts:?}"
    );
    assert!(store::watches().is_empty());
}

#[test]
fn forgejo_429_pauses_but_403_is_an_error_and_a_missing_branch_waits() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    let t = Target::pr("git.example.org", "o/r", 9).on(Forge::Forgejo);
    assert!(store::add_watch(&Watch {
        target: t.clone(),
        pane: "w1:p2".into(),
        since: 0
    }));
    let live = |_: &str| true;
    let answer = |status: u16, retry: Option<i64>| {
        move |_: Forge, _: &str, _: &str, _: Option<&str>| {
            Ok(Resp {
                status,
                retry_after: retry,
                ..Resp::default()
            })
        }
    };
    cycle(0, &live, &mut answer(429, Some(300)));
    let s = store::load_state(&t).unwrap();
    assert_eq!(s.next, 300);
    assert!(s.error.as_deref().unwrap().contains("rate limited"));

    // 403 is a token problem, not a limit: the usual retry, with the HTTP status shown.
    cycle(300, &live, &mut answer(403, None));
    let s = store::load_state(&t).unwrap();
    assert_eq!(s.next, 300 + 2 * PENDING_EVERY);
    assert_eq!(s.error.as_deref(), Some("pull: HTTP 403"));

    // A branch that is not pushed yet: waits, with a hint.
    let b = Target::branch("git.example.org", "o/r", "new").on(Forge::Forgejo);
    assert!(store::add_watch(&Watch {
        target: b.clone(),
        pane: "w1:p2".into(),
        since: 0
    }));
    cycle(1000, &live, &mut answer(404, None));
    let s = store::load_state(&b).unwrap();
    assert_eq!(
        s.error.as_deref(),
        Some("branch not on git.example.org yet (push it)")
    );
    assert!(s.ended.is_none());
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// `run.json` with another status and conclusion.
fn run_at(status: &str, conclusion: Option<&str>) -> String {
    let c = conclusion.map_or("null".to_string(), |c| format!("\"{c}\""));
    fixture("run.json")
        .replace(
            r#""status":"in_progress""#,
            &format!(r#""status":"{status}""#),
        )
        .replace(r#""conclusion":null"#, &format!(r#""conclusion":{c}"#))
}

fn jobs(list: &[(&str, &str, Option<&str>)]) -> String {
    runs(list).replace("check_runs", "jobs")
}

const RUN: u64 = 38721967731;
const RUN_PATH: &str = "repos/o/r/actions/runs/38721967731";
const JOBS_PATH: &str = "repos/o/r/actions/runs/38721967731/jobs?per_page=100";
const RUNS_PATH: &str =
    "repos/o/r/actions/workflows/spire.yml/runs?event=workflow_dispatch&per_page=20&branch=main";
/// 2026-10-15T10:38:10Z: eight seconds after the dispatched run was created.
const DISPATCHED: i64 = 1792060690;

#[test]
fn iso_matches_github_timestamps() {
    assert_eq!(poll::iso(DISPATCHED), "2026-10-15T10:38:10Z");
    assert_eq!(poll::iso(951868799), "2000-02-29T23:59:59Z");
    assert_eq!(poll::iso(0), "1970-01-01T00:00:00Z");
}

#[test]
fn a_dispatched_workflow_resolves_and_mails_its_verdict_last() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    let t = Target::workflow("github.com", "o/r", "spire.yml", Some("main"));
    assert!(store::add_watch(&Watch {
        target: t.clone(),
        pane: "w1:p2".into(),
        since: DISPATCHED,
    }));
    let fake = RefCell::new(Fake::default());
    let mut fetch = |_: Forge, _: &str, p: &str, e: Option<&str>| fake.borrow_mut().get(p, e);
    let live = |_: &str| true;
    {
        let mut f = fake.borrow_mut();
        f.set(RUNS_PATH, &fixture("workflow-runs.json"));
        f.set(RUN_PATH, &fixture("run.json"));
        f.set(JOBS_PATH, &fixture("jobs.json"));
    }

    // The newest run since the slack wins over an older one inside it; in
    // progress: no mail, the pane waits.
    let now = DISPATCHED + 5;
    assert!(cycle(now, &live, &mut fetch).mails.is_empty());
    assert_eq!(fake.borrow().requests, 3);
    let s = store::load_state(&t).unwrap();
    assert_eq!(s.run_id, Some(RUN));
    assert_eq!(s.run_status.as_deref(), Some("in_progress"));
    assert_eq!(s.summary().verdict, Verdict::Pending);
    assert!(store::waiting(now).contains_key("w1:p2"));

    // Resolved once: the list is not asked again; unchanged run and jobs are 304s.
    cycle(now + PENDING_EVERY, &live, &mut fetch);
    assert_eq!((fake.borrow().requests, fake.borrow().full), (5, 3));

    // Completed green: the verdict mail, and the watch ends with it.
    {
        let mut f = fake.borrow_mut();
        f.set(RUN_PATH, &run_at("completed", Some("success")));
        f.set(
            JOBS_PATH,
            &jobs(&[
                ("build", "completed", Some("success")),
                ("deploy (prod)", "completed", Some("success")),
            ]),
        );
    }
    let out = cycle(now + 2 * PENDING_EVERY, &live, &mut fetch);
    assert_eq!(
        out.mails,
        vec![(
            "w1:p2".into(),
            "ci: spire.yml run 38721967731 (o/r@main) head 9f2c4e1: green (3 pass). vega ci show o/r/workflow/spire.yml@main".into()
        )]
    );
    assert!(store::watches().is_empty());
    assert!(store::load_state(&t).is_none());
}

#[test]
fn a_run_mails_red_early_once_and_a_cancelled_run_its_conclusion() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    let t = Target::run("github.com", "o/r", RUN);
    store::add_watch(&Watch {
        target: t.clone(),
        pane: "w1:p2".into(),
        since: 0,
    });
    let fake = RefCell::new(Fake::default());
    let mut fetch = |_: Forge, _: &str, p: &str, e: Option<&str>| fake.borrow_mut().get(p, e);
    let live = |_: &str| true;
    {
        let mut f = fake.borrow_mut();
        f.set(RUN_PATH, &fixture("run.json"));
        f.set(
            JOBS_PATH,
            &jobs(&[
                ("build", "completed", Some("failure")),
                ("deploy (prod)", "queued", None),
            ]),
        );
    }
    // A failed job while the run goes on: red at once, the run still pending.
    let out = cycle(0, &live, &mut fetch);
    assert_eq!(
        out.mails,
        vec![(
            "w1:p2".into(),
            "ci: run 38721967731 (o/r) head 9f2c4e1: red, failed: build; 2 pending. vega ci show o/r/run/38721967731".into()
        )]
    );
    assert_eq!(fake.borrow().requests, 2, "a run id needs no lookup");
    // Completed as failed: red again is no new mail; the watch ends.
    fake.borrow_mut()
        .set(RUN_PATH, &run_at("completed", Some("failure")));
    fake.borrow_mut().set(
        JOBS_PATH,
        &jobs(&[
            ("build", "completed", Some("failure")),
            ("deploy (prod)", "completed", Some("skipped")),
        ]),
    );
    let next = store::load_state(&t).unwrap().next;
    assert!(cycle(next, &live, &mut fetch).mails.is_empty());
    assert!(store::watches().is_empty());

    // A cancelled run with passed jobs is not green: its conclusion is mailed.
    store::add_watch(&Watch {
        target: t.clone(),
        pane: "w1:p2".into(),
        since: 0,
    });
    fake.borrow_mut()
        .set(RUN_PATH, &run_at("completed", Some("cancelled")));
    fake.borrow_mut().set(
        JOBS_PATH,
        &jobs(&[
            ("build", "completed", Some("success")),
            ("deploy (prod)", "completed", Some("cancelled")),
        ]),
    );
    let out = cycle(next + 1000, &live, &mut fetch);
    assert_eq!(
        out.mails,
        vec![(
            "w1:p2".into(),
            "ci: run 38721967731 (o/r) completed: cancelled; watch ended".into()
        )]
    );
    assert!(store::watches().is_empty());
}

#[test]
fn a_workflow_without_a_dispatched_run_ends_after_a_while() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    let t = Target::workflow("github.com", "o/r", "spire.yml", Some("main"));
    // A day after the fixture's runs: none of them is the dispatched one.
    let since = DISPATCHED + 86400;
    store::add_watch(&Watch {
        target: t.clone(),
        pane: "w1:p2".into(),
        since,
    });
    let fake = RefCell::new(Fake::default());
    fake.borrow_mut()
        .set(RUNS_PATH, &fixture("workflow-runs.json"));
    let mut fetch = |_: Forge, _: &str, p: &str, e: Option<&str>| fake.borrow_mut().get(p, e);
    let live = |_: &str| true;
    assert!(cycle(since, &live, &mut fetch).mails.is_empty());
    let s = store::load_state(&t).unwrap();
    assert_eq!((s.run_id, s.error.as_deref()), (None, None));
    assert!(store::waiting(since).contains_key("w1:p2"));
    // The list is unchanged: a 304, free.
    cycle(since + PENDING_EVERY, &live, &mut fetch);
    assert_eq!((fake.borrow().requests, fake.borrow().full), (2, 1));
    let out = cycle(since + poll::RESOLVE_WITHIN + 1, &live, &mut fetch);
    assert_eq!(
        out.mails,
        vec![(
            "w1:p2".into(),
            "ci: spire.yml run (o/r@main) found no workflow_dispatch run in 15 min; watch ended"
                .into()
        )]
    );
    assert!(store::watches().is_empty());
}

const SK: &str = "b4arena/spellkave";
/// `pull-dirty.json`: b4arena/spellkave#4350, recorded 2026-10-19.
const DIRTY_PULL: &str = "repos/b4arena/spellkave/pulls/4350";
const DIRTY_HEAD: &str = "db3650d64707d856f7ed1da242f768c0b8e49280";

#[test]
fn a_conflicting_pr_mails_its_pane_once_per_head_and_state() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    let t = Target::pr("github.com", SK, 4350);
    store::add_watch(&Watch {
        target: t.clone(),
        pane: "w1:p2".into(),
        since: 0,
    });
    let fake = RefCell::new(Fake::default());
    let mut fetch = |_: Forge, _: &str, p: &str, e: Option<&str>| fake.borrow_mut().get(p, e);
    let live = |_: &str| true;
    let dirty = fixture("pull-dirty.json");
    let checks_of = |f: &mut Fake, head: &str| {
        f.set(&format!("repos/{SK}/commits/{head}/status"), NO_STATUSES);
        f.set(
            &format!("repos/{SK}/commits/{head}/check-runs?per_page=100"),
            &runs(&[]),
        );
    };
    {
        let mut f = fake.borrow_mut();
        f.set(DIRTY_PULL, &dirty);
        checks_of(&mut f, DIRTY_HEAD);
    }

    // CONFLICTING: no CI runs on it; the pane gets the next step and is no
    // longer waiting. The branch's run list is a 404 (as without Actions): no
    // zombies, no error.
    let out = cycle(0, &live, &mut fetch);
    let conflict = "ci: PR #4350 (b4arena/spellkave) head db3650d: conflicts with its base \
                    (CONFLICTING); rebase onto the base and resolve them before you push again, \
                    CI on this head cannot merge. vega ci show b4arena/spellkave#4350";
    assert_eq!(out.mails, vec![("w1:p2".into(), conflict.into())]);
    let s = store::load_state(&t).unwrap();
    assert_eq!(s.merge_state.as_deref(), Some("dirty"));
    assert_eq!(s.head_ref.as_deref(), Some("ci/merged-pr-cost"));
    assert_eq!(s.error, None);
    assert!(!store::waiting(0).contains_key("w1:p2"));
    // Pull, status, check-runs, head workflows, branch runs.
    assert_eq!(fake.borrow().requests, 5);

    // A push to the base: GitHub recomputes (`unknown`), then dirty again.
    // Same head and state: no second mail.
    let unknown = dirty
        .replace(r#""mergeable":false"#, r#""mergeable":null"#)
        .replace(
            r#""mergeable_state":"dirty""#,
            r#""mergeable_state":"unknown""#,
        );
    fake.borrow_mut().set(DIRTY_PULL, &unknown);
    assert!(cycle(1000, &live, &mut fetch).mails.is_empty());
    fake.borrow_mut().set(DIRTY_PULL, &dirty);
    assert!(cycle(2000, &live, &mut fetch).mails.is_empty());

    // Behind a base that requires up-to-date branches: another state, a mail.
    let behind = dirty
        .replace(r#""mergeable":false"#, r#""mergeable":true"#)
        .replace(
            r#""mergeable_state":"dirty""#,
            r#""mergeable_state":"behind""#,
        );
    fake.borrow_mut().set(DIRTY_PULL, &behind);
    let out = cycle(3000, &live, &mut fetch);
    assert_eq!(
        out.mails[0].1,
        "ci: PR #4350 (b4arena/spellkave) head db3650d: behind its base, which requires \
         up-to-date branches; rebase onto the base before the merge. vega ci show \
         b4arena/spellkave#4350"
    );

    // Rebased: a new head, mergeable but checks failing (`unstable`): no signal.
    let new_head = "1111111111111111111111111111111111111111";
    {
        let mut f = fake.borrow_mut();
        f.set(
            DIRTY_PULL,
            &fixture("pull-unstable.json")
                .replace("78d20895f3d592b813469030514012becaced583", new_head),
        );
        checks_of(&mut f, new_head);
    }
    assert!(cycle(4000, &live, &mut fetch).mails.is_empty());
    let s = store::load_state(&t).unwrap();
    assert_eq!(s.error, None);
    assert_eq!(s.merge_state.as_deref(), Some("unstable"));
    assert!(s.signals.is_empty(), "the old head's signal is dropped");

    // Conflicting again on the new head: mailed again.
    fake.borrow_mut()
        .set(DIRTY_PULL, &dirty.replace(DIRTY_HEAD, new_head));
    let out = cycle(5000, &live, &mut fetch);
    assert_eq!(out.mails.len(), 1);
    assert!(out.mails[0].1.contains("head 1111111: conflicts"));
}

#[test]
fn a_zombie_run_on_the_prs_branch_mails_the_force_cancel_command_once() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    // `pull-unstable.json` and `runs-branch.json`: b4arena/spellkave#4353 and its
    // branch's runs, recorded 2026-10-19; run 39310882417 is `queued`.
    let head = "78d20895f3d592b813469030514012becaced583";
    let runs_path =
        "repos/b4arena/spellkave/actions/runs?branch=feat%2Ftriton-step-log-build&per_page=20";
    let jobs_path = "repos/b4arena/spellkave/actions/runs/39310882417/jobs?per_page=100";
    let t = Target::pr("github.com", SK, 4353);
    store::add_watch(&Watch {
        target: t.clone(),
        pane: "w1:p2".into(),
        since: 0,
    });
    let fake = RefCell::new(Fake::default());
    let mut fetch = |_: Forge, _: &str, p: &str, e: Option<&str>| fake.borrow_mut().get(p, e);
    let live = |_: &str| true;
    {
        let mut f = fake.borrow_mut();
        f.set(
            "repos/b4arena/spellkave/pulls/4353",
            &fixture("pull-unstable.json"),
        );
        f.set(&format!("repos/{SK}/commits/{head}/status"), NO_STATUSES);
        f.set(
            &format!("repos/{SK}/commits/{head}/check-runs?per_page=100"),
            &runs(&[("test", "in_progress", None)]),
        );
        f.set(runs_path, &fixture("runs-branch.json"));
        f.set(
            &format!("repos/{SK}/actions/runs?head_sha={head}&per_page=100"),
            &fixture("runs-branch.json"),
        );
        f.set(
            jobs_path,
            &jobs(&[
                ("test", "completed", Some("success")),
                ("lint", "queued", None),
            ]),
        );
    }

    // Checks pending: the runs are not asked (three requests).
    assert!(cycle(0, &live, &mut fetch).mails.is_empty());
    assert_eq!(fake.borrow().requests, 3);

    // Checks pass but the head workflow is queued: no verdict mail, jobs not asked yet.
    fake.borrow_mut().set(
        &format!("repos/{SK}/commits/{head}/check-runs?per_page=100"),
        &runs(&[("test", "completed", Some("success"))]),
    );
    let out = cycle(100, &live, &mut fetch);
    assert!(out.mails.is_empty());
    assert_eq!(fake.borrow().requests, 8);
    assert_eq!(store::load_state(&t).unwrap().open_runs, vec![39310882417]);

    // Open for ZOMBIE_AFTER: its jobs are asked; one is still queued (waiting
    // for a runner), so no zombie, and they are not asked again before
    // another ZOMBIE_AFTER.
    let at = 100 + poll::ZOMBIE_AFTER;
    assert!(cycle(at, &live, &mut fetch).mails.is_empty());
    assert_eq!(fake.borrow().requests, 14);
    assert!(cycle(at + SETTLED_EVERY, &live, &mut fetch)
        .mails
        .is_empty());
    assert_eq!(fake.borrow().requests, 19);

    // A partial jobs page (total_count above the listed jobs) is no proof.
    fake.borrow_mut().set(
        jobs_path,
        &jobs(&[("test", "completed", Some("success"))])
            .replace(r#""total_count":1"#, r#""total_count":101"#),
    );
    assert!(cycle(at + poll::ZOMBIE_AFTER, &live, &mut fetch)
        .mails
        .is_empty());

    // All jobs completed, the run still queued: one mail with the command.
    fake.borrow_mut().set(
        jobs_path,
        &jobs(&[
            ("test", "completed", Some("success")),
            ("lint", "completed", Some("success")),
        ]),
    );
    let out = cycle(at + 2 * poll::ZOMBIE_AFTER, &live, &mut fetch);
    assert_eq!(
        out.mails,
        vec![(
            "w1:p2".into(),
            "ci: PR #4353 (b4arena/spellkave): run 39310882417 is still queued or in progress \
             though all its jobs completed (15 min and more); it holds its concurrency group. \
             vega does not cancel it; to force-cancel: gh api -X POST \
             repos/b4arena/spellkave/actions/runs/39310882417/force-cancel"
                .into()
        )]
    );
    let next = store::load_state(&t).unwrap().next;
    assert!(
        cycle(next, &live, &mut fetch).mails.is_empty(),
        "mailed once"
    );

    // Force-cancelled by hand: the run leaves the open list, the zombie goes.
    fake.borrow_mut().set(
        runs_path,
        &fixture("runs-branch.json").replace("queued", "completed"),
    );
    cycle(next + SETTLED_EVERY, &live, &mut fetch);
    let s = store::load_state(&t).unwrap();
    assert!(s.zombies.is_empty() && s.stuck.is_empty() && s.signals.is_empty());
}

#[test]
fn a_watched_run_queued_with_all_jobs_done_is_a_zombie() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    // `run-zombie.json`, `jobs-none.json`: b4arena/spellkave run 31117423266,
    // recorded 2026-10-19: `queued` since 2026-08-06, no jobs.
    let id = 31117423266;
    let t = Target::run("github.com", SK, id);
    store::add_watch(&Watch {
        target: t.clone(),
        pane: "w1:p2".into(),
        since: 0,
    });
    let fake = RefCell::new(Fake::default());
    let mut fetch = |_: Forge, _: &str, p: &str, e: Option<&str>| fake.borrow_mut().get(p, e);
    let live = |_: &str| true;
    {
        let mut f = fake.borrow_mut();
        f.set(
            &format!("repos/{SK}/actions/runs/{id}"),
            &fixture("run-zombie.json"),
        );
        f.set(
            &format!("repos/{SK}/actions/runs/{id}/jobs?per_page=100"),
            &fixture("jobs-none.json"),
        );
    }
    // Seen done once is not enough: a run's status lags its last job.
    assert!(cycle(0, &live, &mut fetch).mails.is_empty());
    assert!(store::waiting(0).contains_key("w1:p2"));
    let out = cycle(poll::ZOMBIE_AFTER, &live, &mut fetch);
    assert_eq!(out.mails.len(), 1);
    assert!(out.mails[0]
        .1
        .starts_with("ci: run 31117423266 (b4arena/spellkave): run 31117423266 is still queued"));
    assert!(out.mails[0]
        .1
        .ends_with("gh api -X POST repos/b4arena/spellkave/actions/runs/31117423266/force-cancel"));
    // Told: the pane no longer waits for a run that never completes; no extra
    // request was made for it (the run and its jobs are polled anyway).
    assert!(!store::waiting(poll::ZOMBIE_AFTER).contains_key("w1:p2"));
    assert_eq!(fake.borrow().requests, 4);
    let next = store::load_state(&t).unwrap().next;
    assert!(cycle(next, &live, &mut fetch).mails.is_empty());
}

#[test]
fn external_passes_wait_for_all_head_workflows() {
    let _turn = serial();
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    // Recorded 2026-10-21 from spellkave#4498: only two Macroscope checks,
    // with CI selection run 39643594803 still pending. Other transitions below
    // are derived from that recording; runs-branch.json records a queued run.
    let head = "653070ea2b7b3a903693fa6d10b130956b78e1b8";
    for t in [
        Target::pr("github.com", SK, 4498),
        Target::branch("github.com", SK, "feature"),
    ] {
        let fake = RefCell::new(Fake::default());
        let mut fetch = |_: Forge, _: &str, p: &str, e: Option<&str>| fake.borrow_mut().get(p, e);
        for pane in ["w1:p2", "w1:p3"] {
            store::add_watch(&Watch {
                target: t.clone(),
                pane: pane.into(),
                since: 0,
            });
        }
        let head_ref = if t.pr.is_some() { head } else { "feature" };
        let path = format!("repos/{SK}/actions/runs?head_sha={head}&per_page=100");
        {
            let mut f = fake.borrow_mut();
            f.set(
                &format!("repos/{SK}/pulls/4498"),
                &pull(head, "open", false),
            );
            f.set(
                &format!("repos/{SK}/commits/{head_ref}/status"),
                &NO_STATUSES.replace('x', head),
            );
            f.set(
                &format!("repos/{SK}/commits/{head_ref}/check-runs?per_page=100"),
                &fixture("check-runs-external.json"),
            );
            f.set(&path, &fixture("runs-head.json"));
        }
        let baseline = 3; // branch also probes zombies
        for (i, status) in ["queued", "in_progress", "waiting", "requested", "pending"]
            .iter()
            .enumerate()
        {
            fake.borrow_mut()
                .set(&path, &fixture("runs-head.json").replace("pending", status));
            let now = i as i64 * PENDING_EVERY;
            let before = fake.borrow().requests;
            assert!(cycle(now, &|_| true, &mut fetch).mails.is_empty());
            assert_eq!(fake.borrow().requests - before, baseline + 1);
            let s = store::load_state(&t).unwrap();
            assert_eq!(s.error, None);
            assert_eq!(s.summary().verdict, Verdict::Pending);
            assert_eq!(s.next, now + PENDING_EVERY);
            assert!(store::owed("w1:p2", Some(&s), now));
        }
        // All 304s keep the queued verdict, at the same shared request cost.
        let before = fake.borrow().full;
        assert!(cycle(150, &|_| true, &mut fetch).mails.is_empty());
        // The branch zombie probe is a 404 in this fixture; other endpoints 304.
        assert_eq!(fake.borrow().full - before, usize::from(t.pr.is_none()));
        let completed = fixture("runs-head.json")
            .replace("pending", "completed")
            .replace("\"conclusion\":null", "\"conclusion\":\"success\"");
        // A partial response cannot prove green, even with a successful run.
        fake.borrow_mut().set(
            &path,
            &completed.replace("\"total_count\":1", "\"total_count\":101"),
        );
        assert!(cycle(180, &|_| true, &mut fetch).mails.is_empty());
        assert_eq!(
            store::load_state(&t).unwrap().summary().verdict,
            Verdict::Pending
        );
        fake.borrow_mut().set(&path, &completed);
        let out = cycle(210, &|_| true, &mut fetch);
        assert_eq!(out.mails.len(), 2);
        assert!(out
            .mails
            .iter()
            .all(|(_, m)| m.contains(": green (3 pass)")));
        assert!(cycle(330, &|_| true, &mut fetch).mails.is_empty());
        // A failed workflow without job checks must still report red.
        fake.borrow_mut()
            .set(&path, &completed.replace("success", "failure"));
        let out = cycle(450, &|_| true, &mut fetch);
        assert_eq!(out.mails.len(), 2);
        assert!(out.mails[0]
            .1
            .contains("red, failed: workflow: CI selection"));
        for pane in ["w1:p2", "w1:p3"] {
            store::remove_watch(&t, pane);
        }
        store::remove_state(&t);
    }
}

#[test]
fn workflow_probe_errors_and_new_heads_cannot_reuse_green() {
    let _turn = serial();
    let t = Target::pr("github.com", "o/r", 42);
    let mut s = store::State::new(&t, 0);
    let mut f = Fake::default();
    for head in ["old", "new"] {
        f.set(&format!("repos/o/r/commits/{head}/status"), NO_STATUSES);
        f.set(
            &format!("repos/o/r/commits/{head}/check-runs?per_page=100"),
            &runs(&[("external", "completed", Some("success"))]),
        );
    }
    f.set("repos/o/r/pulls/42", &pull("old", "open", false));
    poll::poll(&mut s, 0, &mut |_, _, p, e| f.get(p, e)).unwrap();
    assert_eq!(s.summary().verdict, Verdict::Green);
    f.set("repos/o/r/pulls/42", &pull("new", "open", false));
    let path = "repos/o/r/actions/runs?head_sha=new&per_page=100";
    for (status, body) in [
        (404, ""),
        (500, ""),
        (403, ""),
        (200, "{}"),
        (200, "invalid"),
    ] {
        f.pages.insert(
            path.into(),
            (status, format!("tag{status}{body}"), body.into()),
        );
        assert!(poll::poll(&mut s, 30, &mut |_, _, p, e| f.get(p, e)).is_err());
        assert_eq!(s.summary().verdict, Verdict::Pending);
        assert!(poll::mails(&mut s, &["w1:p2".into()]).is_empty());
    }
    // A pending run for a different head must not affect this head.
    f.set(path, &fixture("runs-head.json"));
    poll::poll(&mut s, 60, &mut |_, _, p, e| f.get(p, e)).unwrap();
    assert_eq!(s.summary().verdict, Verdict::Green);
    // After a failed probe of the same head, don't accept a 304 without its
    // cached workflow body. Fetch it again even if its ETag did not change.
    f.pages
        .insert(path.into(), (500, String::new(), String::new()));
    assert!(poll::poll(&mut s, 90, &mut |_, _, p, e| f.get(p, e)).is_err());
    assert_eq!(s.summary().verdict, Verdict::Pending);
    f.set(path, &fixture("runs-head.json"));
    poll::poll(&mut s, 120, &mut |_, _, p, e| f.get(p, e)).unwrap();
    assert_eq!(s.summary().verdict, Verdict::Green);
    // Old caches lack the workflow field and must wait for a probe too.
    let mut cache = serde_json::to_value(&s).unwrap();
    cache.as_object_mut().unwrap().remove("head_workflows");
    let old: store::State = serde_json::from_value(cache).unwrap();
    assert_eq!(old.summary().verdict, Verdict::Pending);
}
