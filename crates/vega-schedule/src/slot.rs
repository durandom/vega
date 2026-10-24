//! Catch-up state of one task (a job's `state.json`, a reminder's
//! `reminders/state/<id>.json`) and the decision a tick takes.
//!
//! A tick: [`SlotState::step`]; on `Fire` the caller (1) writes
//! [`SlotState::begin`] durably, (2) spawns the runner or publishes, (3)
//! writes [`SlotState::finish`]. A crash after (1) leaves `pending`, and the
//! next step is `Resume` of that same slot: it is neither repeated as a new
//! firing nor dropped. A crash after (3) is done. Slots between two firings
//! are skipped for good: only the latest slot at or before now ever fires.

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::when::When;

/// The vegad tick.
pub const TICK: SignedDuration = SignedDuration::from_secs(30);

/// A slot fired this much after its time counts as a catch-up (`late: due
/// <slot>` on a reminder): more than one missed tick.
pub const LATE_AFTER: SignedDuration = SignedDuration::from_secs(60);

/// `catchup = false` skips a slot missed by more than this.
pub const CATCHUP_GRACE: SignedDuration = SignedDuration::from_mins(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotState {
    /// No slot before this fires: the reminder's creation, the job's first
    /// approval on this host.
    pub since: Timestamp,
    /// The latest slot handled (fired or skipped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_slot: Option<Timestamp>,
    /// A slot begun but not yet moved to `last_slot`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing due.
    Idle,
    /// A begun slot is unfinished (crash or failed publish): carry it on.
    /// Jobs ask [`recover`]; reminders publish again under the same dedup id.
    Resume(Timestamp),
    /// Fire this slot; `late` when it is a catch-up.
    Fire { slot: Timestamp, late: bool },
    /// `catchup = false` and the slot was missed: record it, do not run it.
    Skip(Timestamp),
}

impl SlotState {
    pub fn new(since: Timestamp) -> Self {
        SlotState {
            since,
            last_slot: None,
            pending: None,
        }
    }

    pub fn step(&self, when: &When, tz: &TimeZone, now: Timestamp, catchup: bool) -> Step {
        if let Some(p) = self.pending {
            return Step::Resume(p);
        }
        let Some(slot) = when.latest(tz, now) else {
            return Step::Idle;
        };
        if slot < self.since || self.last_slot.is_some_and(|l| l >= slot) {
            return Step::Idle;
        }
        let behind = now.duration_since(slot);
        if !catchup && behind > CATCHUP_GRACE {
            return Step::Skip(slot);
        }
        Step::Fire {
            slot,
            late: behind > LATE_AFTER,
        }
    }

    /// Step (1): the slot is taken.
    pub fn begin(&mut self, slot: Timestamp) {
        self.pending = Some(slot);
    }

    /// Step (3), or the record of a skip: the slot is handled.
    pub fn finish(&mut self, slot: Timestamp) {
        self.last_slot = Some(self.last_slot.map_or(slot, |l| l.max(slot)));
        if self.pending.is_some_and(|p| p <= slot) {
            self.pending = None;
        }
    }

    /// A one-shot task whose slot is handled (or lies before `since`): the
    /// owner deletes it.
    pub fn is_done(&self, when: &When) -> bool {
        match *when {
            When::At(t) => {
                self.pending.is_none() && (t < self.since || self.last_slot.is_some_and(|l| l >= t))
            }
            _ => false,
        }
    }
}

/// What a job's `pending` slot needs after a restart, from its run file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// No run file: the runner never started; spawn it again (it is
    /// idempotent on the run file).
    Spawn,
    /// The run file exists: move the slot to `last_slot`. `lost`: it still
    /// says `started` but nobody holds the job's lock, so the run died; mark
    /// it lost and mail it like a failure, never run it again.
    Finish { lost: bool },
}

pub fn recover(run: Option<&crate::run::RunFile>, lock_free: bool) -> Recovery {
    match run {
        None => Recovery::Spawn,
        Some(r) => Recovery::Finish {
            lost: r.outcome == crate::run::Outcome::Started && lock_free,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Outcome, RunFile};
    use crate::when::tests::{berlin, ts};

    fn daily() -> When {
        When::daily("07:00").unwrap()
    }

    /// Fires every due slot like vegad would, returning the fired slots.
    fn tick_all(
        state: &mut SlotState,
        when: &When,
        ticks: &[&str],
        catchup: bool,
    ) -> Vec<Timestamp> {
        let mut fired = vec![];
        for t in ticks {
            match state.step(when, &berlin(), ts(t), catchup) {
                Step::Fire { slot, .. } => {
                    state.begin(slot);
                    fired.push(slot);
                    state.finish(slot);
                }
                Step::Skip(slot) => state.finish(slot),
                Step::Resume(_) => panic!("nothing pending"),
                Step::Idle => {}
            }
        }
        fired
    }

    #[test]
    fn fires_once_per_slot() {
        let mut s = SlotState::new(ts("2026-10-21T00:00:00Z"));
        let fired = tick_all(
            &mut s,
            &daily(),
            &[
                "2026-10-21T04:59:30Z",
                "2026-10-21T05:00:10Z",
                "2026-10-21T05:00:40Z",
                "2026-10-21T12:00:00Z",
            ],
            true,
        );
        assert_eq!(fired, vec![ts("2026-10-21T05:00:00Z")]);
    }

    #[test]
    fn a_week_asleep_fires_once_and_skipped_slots_never_run_later() {
        let mut s = SlotState::new(ts("2026-10-01T00:00:00Z"));
        let when = daily();
        assert_eq!(
            tick_all(&mut s, &when, &["2026-10-10T05:00:05Z"], true),
            vec![ts("2026-10-10T05:00:00Z")]
        );
        // Asleep 10-11 .. 10-17; wakes 10-17 at 09:00 local.
        let now = ts("2026-10-17T07:00:00Z");
        assert_eq!(
            s.step(&when, &berlin(), now, true),
            Step::Fire {
                slot: ts("2026-10-17T05:00:00Z"),
                late: true
            }
        );
        assert_eq!(
            tick_all(&mut s, &when, &["2026-10-17T07:00:00Z"], true).len(),
            1
        );
        // No later tick ever returns one of the skipped 10-11..10-16 slots.
        let later: Vec<String> = (0..48)
            .map(|h| {
                ts("2026-10-17T07:30:00Z")
                    .checked_add(SignedDuration::from_hours(h))
                    .unwrap()
                    .to_string()
            })
            .collect();
        let later: Vec<&str> = later.iter().map(String::as_str).collect();
        let fired = tick_all(&mut s, &when, &later, true);
        assert_eq!(
            fired,
            vec![ts("2026-10-18T05:00:00Z"), ts("2026-10-19T05:00:00Z")]
        );
    }

    #[test]
    fn nothing_fires_for_slots_before_since() {
        // Approved at 07:30 local: today's 07:00 slot is not owed.
        let s = SlotState::new(ts("2026-10-21T05:30:00Z"));
        assert_eq!(
            s.step(&daily(), &berlin(), ts("2026-10-21T06:00:00Z"), true),
            Step::Idle
        );
        assert!(matches!(
            s.step(&daily(), &berlin(), ts("2026-10-22T05:00:00Z"), true),
            Step::Fire { late: false, .. }
        ));
    }

    #[test]
    fn catchup_false_skips_after_grace() {
        let s = SlotState::new(ts("2026-10-01T00:00:00Z"));
        let w = daily();
        assert!(matches!(
            s.step(&w, &berlin(), ts("2026-10-21T05:14:00Z"), false),
            Step::Fire { .. }
        ));
        assert_eq!(
            s.step(&w, &berlin(), ts("2026-10-21T05:16:00Z"), false),
            Step::Skip(ts("2026-10-21T05:00:00Z"))
        );
        // The skipped slot is recorded and never fires, also not with catch-up.
        let mut s = s;
        assert!(tick_all(&mut s, &w, &["2026-10-21T05:16:00Z"], false).is_empty());
        assert_eq!(
            s.step(&w, &berlin(), ts("2026-10-21T05:17:00Z"), true),
            Step::Idle
        );
    }

    #[test]
    fn crash_between_steps_resumes_the_same_slot_once() {
        let when = daily();
        let tz = berlin();
        let mut s = SlotState::new(ts("2026-10-01T00:00:00Z"));
        let Step::Fire { slot, .. } = s.step(&when, &tz, ts("2026-10-21T05:00:10Z"), true) else {
            panic!()
        };
        s.begin(slot);
        // Crash after (1): the persisted state comes back from disk.
        let mut s: SlotState = serde_json::from_slice(&crate::to_json(&s)).unwrap();
        // Even a day later the pending slot comes first, not a new firing.
        assert_eq!(
            s.step(&when, &tz, ts("2026-10-22T06:00:00Z"), true),
            Step::Resume(slot)
        );
        assert_eq!(
            s.step(&when, &tz, ts("2026-10-21T05:00:40Z"), true),
            Step::Resume(slot)
        );
        s.finish(slot);
        assert_eq!(
            s.step(&when, &tz, ts("2026-10-21T05:00:40Z"), true),
            Step::Idle
        );
        // Crash after (3) leaves nothing to do for that slot.
        let s: SlotState = serde_json::from_slice(&crate::to_json(&s)).unwrap();
        assert_eq!(s.last_slot, Some(slot));
        assert_eq!(
            s.step(&when, &tz, ts("2026-10-21T23:00:00Z"), true),
            Step::Idle
        );
    }

    #[test]
    fn recovery_never_reruns_a_started_job() {
        let slot = ts("2026-10-21T05:00:00Z");
        let mut run = RunFile::started(slot, slot);
        assert_eq!(recover(None, true), Recovery::Spawn);
        assert_eq!(recover(Some(&run), true), Recovery::Finish { lost: true });
        assert_eq!(recover(Some(&run), false), Recovery::Finish { lost: false });
        run.outcome = Outcome::Succeeded;
        assert_eq!(recover(Some(&run), true), Recovery::Finish { lost: false });
    }

    #[test]
    fn one_shot_is_done_after_its_slot() {
        let t = ts("2026-10-21T05:00:00Z");
        let w = When::At(t);
        let mut s = SlotState::new(ts("2026-10-20T00:00:00Z"));
        assert!(!s.is_done(&w));
        s.begin(t);
        assert!(!s.is_done(&w), "pending is not done");
        s.finish(t);
        assert!(s.is_done(&w));
        assert_eq!(
            s.step(&w, &berlin(), ts("2026-10-22T00:00:00Z"), true),
            Step::Idle
        );
    }

    #[test]
    fn state_json_rejects_unknown_keys() {
        let ok = r#"{"since":"2026-10-20T00:00:00Z","last_slot":"2026-10-21T05:00:00Z"}"#;
        assert!(serde_json::from_str::<SlotState>(ok).is_ok());
        let bad = r#"{"since":"2026-10-20T00:00:00Z","last":"2026-10-21T05:00:00Z"}"#;
        assert!(serde_json::from_str::<SlotState>(bad).is_err());
    }
}
