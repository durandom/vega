//! The sync cycle vegad runs every few seconds (docs/design/beads-sync.md,
//! Sync rules):
//!
//! - Every [`PULL_EVERY`]: pull, then push if ahead.
//! - When the chunk journal changed since the last look (a local write):
//!   count; if ahead, pull first, then push. Never force.
//! - Conflict, or a push rejected for another reason than being behind:
//!   the project stops until `vega bd resume`, and the caller raises an
//!   attention. Unreachable remote: an error, retried on the next interval.
//! - A transient pull failure ([`Outcome::Retry`]): pulled again at once
//!   (fast-forwarded when bd says the remote tip is fast-forwardable), then
//!   an error retried on the next interval; never a stop. A stop recorded
//!   for one (by an older vegad) lifts itself.

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::bd::{self, Counts, Outcome};
use crate::store::{self, Project, State};

pub const PULL_EVERY: i64 = 60;

/// pull and push over the tailnet; a large first push may take a while, and
/// bd's pre-push fsck gets up to [`bd::FSCK_TIMEOUT`] before that.
const NET_LIMIT: Duration = Duration::from_secs(420);
const SQL_LIMIT: Duration = Duration::from_secs(30);

/// The operations a cycle needs; [`Bd`] is the real one.
pub trait Ops {
    fn pull(&mut self, p: &Project) -> Outcome;
    fn push(&mut self, p: &Project) -> Outcome;
    fn counts(&mut self, p: &Project) -> Result<Counts, String>;
    /// [`bd::FAST_FORWARD_SQL`]: heals a [`bd::fast_forwardable`] pull.
    fn fast_forward(&mut self, p: &Project) -> Outcome;
    /// The newest mtime (ns) in the project's noms dir; 0 if unreadable.
    fn last_write(&mut self, p: &Project) -> u128;
}

/// Runs bd as `actor` (`<host>/vegad`).
pub struct Bd {
    pub actor: String,
}

impl Ops for Bd {
    fn pull(&mut self, p: &Project) -> Outcome {
        let mut c = bd::bd(p, &self.actor);
        c.args(["dolt", "pull"]);
        bd::classify(&bd::run(c, NET_LIMIT))
    }

    fn push(&mut self, p: &Project) -> Outcome {
        let mut c = bd::bd(p, &self.actor);
        c.args(["dolt", "push"]);
        bd::classify(&bd::run(c, NET_LIMIT))
    }

    fn fast_forward(&mut self, p: &Project) -> Outcome {
        let mut c = bd::bd(p, &self.actor);
        c.args(["sql", bd::FAST_FORWARD_SQL]);
        bd::classify(&bd::run(c, NET_LIMIT))
    }

    fn counts(&mut self, p: &Project) -> Result<Counts, String> {
        let mut c = bd::bd(p, &self.actor);
        c.args(["sql", "--json", bd::COUNTS_SQL]);
        let r = bd::run(c, SQL_LIMIT);
        bd::parse_counts(&r.out).ok_or_else(|| bd::last_line(&r.out))
    }

    fn last_write(&mut self, p: &Project) -> u128 {
        newest_mtime(&p.noms())
    }
}

pub fn newest_mtime(dir: &Path) -> u128 {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .max()
        .unwrap_or(0)
}

/// What a cycle asks the caller to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Log(String),
    /// A project stopped syncing: tell its UAC (key, database, reason).
    Stopped {
        key: String,
        common: std::path::PathBuf,
        database: String,
        reason: String,
    },
}

/// One pass over the projects; reads and writes their state files.
pub fn cycle(now: i64, ops: &mut dyn Ops) -> Vec<Event> {
    let mut events = Vec::new();
    for p in store::projects() {
        if let Err(e) = p.validate_location() {
            if let Some(message) =
                vega_herdr::project::registered_diagnostic("bd sync", &p.key, &p.common, Some(&e))
            {
                events.push(Event::Log(format!("bd sync: {message}")));
            }
            continue;
        }
        vega_herdr::project::registered_diagnostic("bd sync", &p.key, &p.common, None);
        let mut s = store::state(&p.key, &p.common);
        let before = s.clone();
        step(now, &p, &mut s, ops, &mut events);
        if s != before {
            store::save_state(&p.key, &p.common, &s);
        }
    }
    events
}

/// One project. Pure apart from `ops`, for the tests.
pub fn step(now: i64, p: &Project, s: &mut State, ops: &mut dyn Ops, ev: &mut Vec<Event>) {
    let heals = s.stop_reason.as_deref().is_some_and(heals_itself);
    if s.is_stopped(p) && !heals {
        return;
    }
    if s.stopped.is_some() {
        // Resumed, or stopped without a conflict (vegad before vega-lhc.130
        // stopped on transient pull failures): forget the stop, sync now.
        let how = if s.is_stopped(p) {
            "self-resumed"
        } else {
            "resumed"
        };
        s.stopped = None;
        s.stop_reason = None;
        s.pull_tried = 0;
        ev.push(Event::Log(format!("bd sync: {} {how}", p.key)));
    }
    let write = ops.last_write(p);
    let wrote = write != s.seen_write;
    let due = now - s.pull_tried >= PULL_EVERY;
    if !due && !wrote {
        return;
    }
    // Recorded before our own pull or push touch the journal: a write in
    // between is never missed, ours costs one extra count.
    s.seen_write = write;
    if !due {
        match ops.counts(p) {
            Ok(c) if c.ahead == 0 => {
                set_counts(s, &c);
                return;
            }
            Ok(_) => {}
            Err(e) => return count_failed(p, s, now, e, ev),
        }
    }
    s.pull_tried = now;
    let mut pulled = ops.pull(p);
    let mut retried = false;
    if let Outcome::Retry(m) = &pulled {
        pulled = if bd::fast_forwardable(m) {
            ev.push(Event::Log(format!("bd sync: {} fast-forward: {m}", p.key)));
            ops.fast_forward(p)
        } else {
            ev.push(Event::Log(format!("bd sync: {} pull again: {m}", p.key)));
            ops.pull(p)
        };
        retried = true;
    }
    match pulled {
        Outcome::Ok => s.pulled = now,
        Outcome::Conflict(m) => return stop(p, s, now, format!("pull conflict: {m}"), ev),
        // The first try failed without a conflict: whatever the second says,
        // the next interval tries again.
        Outcome::Unreachable(m) | Outcome::Retry(m) => {
            s.error = Some(format!("pull: {m}"));
            return;
        }
        Outcome::Behind(m) | Outcome::Failed(m) if retried => {
            s.error = Some(format!("pull: {m}"));
            return;
        }
        Outcome::Behind(m) | Outcome::Failed(m) => {
            return stop(p, s, now, format!("pull failed: {m}"), ev)
        }
    }
    let c = match ops.counts(p) {
        Ok(c) => c,
        Err(e) => return count_failed(p, s, now, e, ev),
    };
    set_counts(s, &c);
    if c.ahead > 0 {
        match ops.push(p) {
            Outcome::Ok => {
                s.pushed = now;
                s.ahead = Some(0);
                ev.push(Event::Log(format!(
                    "bd sync: {} pushed {} commit(s)",
                    p.key, c.ahead
                )));
            }
            Outcome::Behind(m) => {
                // Another machine pushed between our pull and push: pull
                // again on the next tick.
                s.pull_tried = 0;
                s.error = Some(format!("push: {m}"));
                return;
            }
            Outcome::Unreachable(m) | Outcome::Retry(m) => {
                s.error = Some(format!("push: {m}"));
                return;
            }
            Outcome::Conflict(m) | Outcome::Failed(m) => {
                return stop(p, s, now, format!("push failed: {m}"), ev)
            }
        }
    }
    s.error = None;
    s.synced = now;
}

/// A stop whose reason is a transient failure, not a conflict.
fn heals_itself(reason: &str) -> bool {
    let r = bd::Run {
        ok: false,
        out: reason.to_string(),
        timed_out: false,
    };
    matches!(bd::classify(&r), Outcome::Retry(_))
}

/// A failed count is an error retried next time, except bd's identity
/// check: the database now holds another project's identity (two clones
/// seeded an empty remote at once), which no retry fixes.
fn count_failed(p: &Project, s: &mut State, now: i64, e: String, ev: &mut Vec<Event>) {
    if e.contains("IDENTITY MISMATCH") || e.contains("Do NOT run 'bd init'") {
        stop(p, s, now, format!("project identity mismatch: {e}"), ev);
    } else {
        s.error = Some(format!("count: {e}"));
    }
}

fn set_counts(s: &mut State, c: &Counts) {
    s.ahead = Some(c.ahead);
    s.behind = Some(c.behind);
    s.head = Some(c.head.clone());
}

fn stop(p: &Project, s: &mut State, now: i64, reason: String, ev: &mut Vec<Event>) {
    s.stopped = Some(now);
    s.stop_reason = Some(reason.clone());
    s.error = None;
    ev.push(Event::Log(format!("bd sync: {} stopped: {reason}", p.key)));
    ev.push(Event::Stopped {
        key: p.key.clone(),
        common: p.common.clone(),
        database: p.database.clone(),
        reason,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        pull: Vec<Outcome>,
        push: Vec<Outcome>,
        ahead: u64,
        write: u128,
        calls: Vec<&'static str>,
    }

    impl Ops for Fake {
        fn pull(&mut self, _: &Project) -> Outcome {
            self.calls.push("pull");
            self.pull.pop().unwrap_or(Outcome::Ok)
        }
        fn push(&mut self, _: &Project) -> Outcome {
            self.calls.push("push");
            let o = self.push.pop().unwrap_or(Outcome::Ok);
            if o == Outcome::Ok {
                self.ahead = 0;
            }
            o
        }
        fn fast_forward(&mut self, _: &Project) -> Outcome {
            self.calls.push("ff");
            self.pull.pop().unwrap_or(Outcome::Ok)
        }
        fn counts(&mut self, _: &Project) -> Result<Counts, String> {
            self.calls.push("count");
            Ok(Counts {
                ahead: self.ahead,
                behind: 0,
                head: "h".into(),
            })
        }
        fn last_write(&mut self, _: &Project) -> u128 {
            self.write
        }
    }

    fn project() -> Project {
        Project {
            key: "k".into(),
            common: "/x/.git".into(),
            dir: "/x".into(),
            database: "X".into(),
            remote: "http://r/X".into(),
            bd: "/bin/bd".into(),
            data: "/x/.beads/dolt".into(),
            since: 0,
            resumed: 0,
        }
    }

    #[test]
    fn interval_pulls_and_pushes_when_ahead() {
        let p = project();
        let mut s = State::default();
        let mut f = Fake {
            ahead: 2,
            ..Fake::default()
        };
        let mut ev = Vec::new();
        step(1000, &p, &mut s, &mut f, &mut ev);
        assert_eq!(f.calls, ["pull", "count", "push"]);
        assert_eq!(
            (s.pulled, s.pushed, s.synced, s.ahead),
            (1000, 1000, 1000, Some(0))
        );
    }

    #[test]
    fn quiet_between_intervals() {
        let p = project();
        let mut s = State {
            pull_tried: 1000,
            seen_write: 5,
            ..State::default()
        };
        let mut f = Fake {
            write: 5,
            ..Fake::default()
        };
        step(1030, &p, &mut s, &mut f, &mut Vec::new());
        assert!(f.calls.is_empty());
    }

    #[test]
    fn a_write_counts_and_pushes_after_a_pull() {
        let p = project();
        let mut s = State {
            pull_tried: 1000,
            seen_write: 5,
            ..State::default()
        };
        let mut f = Fake {
            write: 6,
            ahead: 1,
            ..Fake::default()
        };
        step(1010, &p, &mut s, &mut f, &mut Vec::new());
        assert_eq!(f.calls, ["count", "pull", "count", "push"]);
        assert_eq!(s.seen_write, 6);
        // A write that is not ahead (a read touched the journal): count only.
        let mut f = Fake {
            write: 7,
            ..Fake::default()
        };
        step(1020, &p, &mut s, &mut f, &mut Vec::new());
        assert_eq!(f.calls, ["count"]);
    }

    #[test]
    fn conflict_stops_until_resumed() {
        let mut p = project();
        let mut s = State::default();
        let mut f = Fake {
            pull: vec![Outcome::Conflict("merge conflicts in issues".into())],
            ..Fake::default()
        };
        let mut ev = Vec::new();
        step(1000, &p, &mut s, &mut f, &mut ev);
        assert!(s.is_stopped(&p));
        assert!(ev.iter().any(|e| matches!(e, Event::Stopped { .. })));
        let mut f = Fake::default();
        step(2000, &p, &mut s, &mut f, &mut Vec::new());
        assert!(f.calls.is_empty(), "a stopped project does not sync");
        p.resumed = 2001;
        step(2002, &p, &mut s, &mut f, &mut Vec::new());
        assert_eq!(f.calls, ["pull", "count"]);
        assert!(s.stopped.is_none() && s.synced == 2002);
    }

    #[test]
    fn unreachable_is_an_error_not_a_stop() {
        let p = project();
        let mut s = State::default();
        let mut f = Fake {
            pull: vec![Outcome::Unreachable("code = Unavailable".into())],
            ..Fake::default()
        };
        step(1000, &p, &mut s, &mut f, &mut Vec::new());
        assert!(!s.is_stopped(&p));
        assert!(s.error.is_some());
        // Next interval succeeds and clears it.
        let mut f = Fake::default();
        step(1000 + PULL_EVERY, &p, &mut s, &mut f, &mut Vec::new());
        assert!(s.error.is_none());
    }

    /// vega-lhc.111: bd's auto-commit raced an agent's write (`cannot merge
    /// with uncommitted changes`): pulled again at once, no stop.
    #[test]
    fn a_transient_pull_failure_pulls_again() {
        let p = project();
        let mut s = State::default();
        let dirty = || Outcome::Retry("cannot merge with uncommitted changes".into());
        let mut f = Fake {
            pull: vec![dirty()],
            ..Fake::default()
        };
        let mut ev = Vec::new();
        step(1000, &p, &mut s, &mut f, &mut ev);
        assert_eq!(f.calls, ["pull", "pull", "count"]);
        assert!(!s.is_stopped(&p) && s.error.is_none() && s.synced == 1000);
        // Failing twice: an error, retried on the next interval; never a stop.
        let mut f = Fake {
            pull: vec![dirty(), dirty()],
            ..Fake::default()
        };
        step(1000 + PULL_EVERY, &p, &mut s, &mut f, &mut ev);
        assert_eq!(f.calls, ["pull", "pull"]);
        assert!(!s.is_stopped(&p) && s.error.is_some());
        assert!(!ev.iter().any(|e| matches!(e, Event::Stopped { .. })));
    }

    /// vega-lhc.130: ganymede stopped 22 h on a peer's push after bd's
    /// fetch; it fast-forwards instead. proteus stopped on a restarted
    /// local server; it pulls again.
    #[test]
    fn a_fast_forwardable_tip_heals_without_a_stop() {
        let p = project();
        let mut s = State::default();
        let mut f = Fake {
            pull: vec![Outcome::Retry(
                "pull merged nothing but the local branch is behind a fast-forwardable remote tip"
                    .into(),
            )],
            ..Fake::default()
        };
        let mut ev = Vec::new();
        step(1000, &p, &mut s, &mut f, &mut ev);
        assert_eq!(f.calls, ["pull", "ff", "count"]);
        assert!(!s.is_stopped(&p) && s.synced == 1000);
        let mut f = Fake {
            pull: vec![Outcome::Retry("invalid connection".into())],
            ..Fake::default()
        };
        step(1000 + PULL_EVERY, &p, &mut s, &mut f, &mut ev);
        assert_eq!(f.calls, ["pull", "pull", "count"]);
        assert!(!ev.iter().any(|e| matches!(e, Event::Stopped { .. })));
    }

    /// A stop an older vegad recorded for a transient failure lifts itself;
    /// a conflict's stop does not.
    #[test]
    fn a_stop_without_a_conflict_lifts_itself() {
        let p = project();
        let mut s = State {
            stopped: Some(900),
            stop_reason: Some("pull failed: invalid connection".into()),
            ..State::default()
        };
        let mut f = Fake::default();
        let mut ev = Vec::new();
        step(1000, &p, &mut s, &mut f, &mut ev);
        assert_eq!(f.calls, ["pull", "count"]);
        assert!(s.stopped.is_none() && s.synced == 1000);
        assert!(ev.contains(&Event::Log("bd sync: k self-resumed".into())));
        let mut s = State {
            stopped: Some(900),
            stop_reason: Some(
                "pull conflict: merge conflicts in issues require operator resolution".into(),
            ),
            ..State::default()
        };
        let mut f = Fake::default();
        step(1000, &p, &mut s, &mut f, &mut Vec::new());
        assert!(f.calls.is_empty() && s.is_stopped(&p));
    }

    #[test]
    fn rejected_push_stops_but_behind_retries() {
        let p = project();
        let mut s = State::default();
        let mut f = Fake {
            ahead: 1,
            push: vec![Outcome::Behind(
                "tip of your current branch is behind".into(),
            )],
            ..Fake::default()
        };
        step(1000, &p, &mut s, &mut f, &mut Vec::new());
        assert!(!s.is_stopped(&p));
        assert_eq!(s.pull_tried, 0, "pulls again next tick");
        let mut f = Fake {
            ahead: 1,
            push: vec![Outcome::Failed("permission denied".into())],
            ..Fake::default()
        };
        let mut ev = Vec::new();
        step(1001, &p, &mut s, &mut f, &mut ev);
        assert!(s.is_stopped(&p));
    }
    #[test]
    fn cycle_skips_failed_or_changed_provisioned_locations_without_saving() {
        const MODE: &str = "VEGA_BD_CYCLE_GUARD_FIXTURE";
        if let Ok(mode) = std::env::var(MODE) {
            let root = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
            let dir = root.join("repo");
            std::fs::create_dir(&dir).unwrap();
            let git = |args: &[&str]| {
                assert!(std::process::Command::new("git")
                    .arg("-C")
                    .arg(&dir)
                    .args(args)
                    .status()
                    .unwrap()
                    .success());
            };
            git(&["init", "-q"]);
            git(&[
                "remote",
                "add",
                "origin",
                "https://github.com/team/repo.git",
            ]);
            let resolved = vega_herdr::project::resolve(&dir, "fixture").unwrap();
            let p = Project {
                key: resolved.id.to_string(),
                common: resolved.location.common_dir,
                dir,
                ..project()
            };
            assert!(store::save_project(&p));
            let state = State {
                synced: 17,
                head: Some("preserve".into()),
                ..State::default()
            };
            assert!(store::save_state(&p.key, &p.common, &state));
            let state_file = store::file("state", &p.key, &p.common).unwrap();
            let project_file = store::project_path(&p.key, &p.common).unwrap();
            let state_bytes = std::fs::read(&state_file).unwrap();
            let project_bytes = std::fs::read(&project_file).unwrap();
            if mode == "config" {
                std::fs::write(p.common.join("config"), "[broken configuration").unwrap();
            } else {
                assert!(std::process::Command::new("git")
                    .arg("-C")
                    .arg(&p.dir)
                    .args([
                        "remote",
                        "set-url",
                        "origin",
                        "https://github.com/team/fork.git"
                    ])
                    .status()
                    .unwrap()
                    .success());
            }
            let mut ops = Fake::default();
            let events = cycle(1000, &mut ops);
            assert_eq!(events.len(), 1);
            assert!(
                matches!(&events[0], Event::Log(message) if message.contains(if mode == "config" { "project_git_failed" } else { "project_identity_changed" }))
            );
            assert!(
                cycle(1001, &mut ops).is_empty(),
                "the same failure is logged once"
            );
            let (cells, problem) = store::row(&p, &state, 1000);
            assert_eq!(cells[6], "error");
            let problem = problem.unwrap();
            let failure = p.validate_location().unwrap_err();
            assert!(problem.contains(&failure.code));
            assert!(problem.contains(&failure.hint));
            let stopped = State {
                stopped: Some(99),
                stop_reason: Some("keep stopped".into()),
                ..state.clone()
            };
            let (cells, problem) = store::row(&p, &stopped, 1000);
            assert_eq!(cells[6], "stopped");
            assert!(problem.unwrap().contains("keep stopped"));
            assert!(ops.calls.is_empty(), "invalid locations never enter step");
            assert_eq!(std::fs::read(state_file).unwrap(), state_bytes);
            assert_eq!(std::fs::read(project_file).unwrap(), project_bytes);
            std::fs::write(root.join("returned"), "cycle returned without exiting").unwrap();
            return;
        }
        for mode in ["config", "changed"] {
            let root = tempfile::tempdir().unwrap();
            let root = root.path().canonicalize().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "sync::tests::cycle_skips_failed_or_changed_provisioned_locations_without_saving", "--nocapture"])
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
                "cycle must return rather than exiting the process"
            );
        }
    }
}
