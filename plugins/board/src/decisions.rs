//! The UAC's beads: the open `slayer-decision` children of the repo's one open
//! `uac-tracker` epic (the Inbox lists them), and the open beads the UAC's
//! lanes created (label `uac-lane`, stamped by `vega uac status`; the board
//! only reads it). The bead tabs show the lane or all beads; `D` toggles.

use crate::bd::types::Bead;

pub const TRACKER_LABEL: &str = "uac-tracker";
pub const DECISION_LABEL: &str = "slayer-decision";
pub const LANE_LABEL: &str = "uac-lane";

/// What the bead tabs are filtered to; `D` toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Lane,
    All,
}

impl Preset {
    pub fn next(self) -> Preset {
        match self {
            Preset::Lane => Preset::All,
            Preset::All => Preset::Lane,
        }
    }
}

/// The tracker epic the decisions hang off, found among the loaded beads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tracker {
    One(String),
    None,
    Several(Vec<String>),
}

impl Tracker {
    pub fn find(beads: &[Bead]) -> Tracker {
        let mut ids: Vec<String> = beads
            .iter()
            .filter(|b| {
                b.issue_type == "epic"
                    && !b.is_closed()
                    && b.labels.iter().any(|l| l == TRACKER_LABEL)
            })
            .map(|b| b.id.clone())
            .collect();
        match ids.len() {
            0 => Tracker::None,
            1 => Tracker::One(ids.remove(0)),
            _ => {
                ids.sort();
                Tracker::Several(ids)
            }
        }
    }

    pub fn id(&self) -> Option<&str> {
        match self {
            Tracker::One(id) => Some(id),
            _ => None,
        }
    }

    /// Why the Inbox lists no decisions, for the pane.
    pub fn problem(&self) -> Option<String> {
        match self {
            Tracker::One(_) => None,
            Tracker::None => Some(format!("no open {TRACKER_LABEL} epic: no decisions")),
            Tracker::Several(ids) => Some(format!(
                "{} {TRACKER_LABEL} epics ({}): no decisions",
                ids.len(),
                ids.join(", ")
            )),
        }
    }
}

/// An open decision for the Slayer under `tracker`.
pub fn is_decision(b: &Bead, tracker: &str) -> bool {
    !b.is_closed()
        && b.parent.as_deref() == Some(tracker)
        && b.labels.iter().any(|l| l == DECISION_LABEL)
}

/// An open bead one of the UAC's lanes created.
pub fn is_lane(b: &Bead) -> bool {
    !b.is_closed() && b.labels.iter().any(|l| l == LANE_LABEL)
}

/// The lane backlog as `vega uac status` counts it: open lane beads and the
/// oldest one with its age, e.g. `3 open, oldest v-7 4d`.
pub fn lane_summary(beads: &[Bead], now: i64) -> String {
    let open: Vec<&Bead> = beads.iter().filter(|b| is_lane(b)).collect();
    let oldest = open
        .iter()
        .filter_map(|b| Some((parse_rfc3339(b.created_at.as_deref()?)?, &b.id)))
        .min()
        .map(|(t, id)| format!(", oldest {id} {}", age(now - t)))
        .unwrap_or_default();
    format!("{} open{oldest}", open.len())
}

/// How long ago the bead was created, `?` when bd gave no time.
pub fn bead_age(b: &Bead, now: i64) -> String {
    b.created_at
        .as_deref()
        .and_then(parse_rfc3339)
        .map_or_else(|| "?".into(), |t| age(now - t))
}

/// The buckets of `vega uac status`: 45s, 12m, 3h05m, 4d.
pub fn age(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..172800 => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d", secs / 86400),
    }
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Seconds since the epoch of an RFC 3339 time as bd writes it.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let rest = s
        .get(19..)?
        .trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "Z" | "" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let h = o.get(1..3)?.parse::<i64>().ok()?;
            let mi = o.get(4..6)?.parse::<i64>().ok()?;
            sign * (h * 3600 + mi * 60)
        }
    };
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_summary_counts_open_lane_beads_and_the_oldest() {
        let mut a = bead("v-1", "task", "open", &[LANE_LABEL], None);
        a.created_at = Some("2026-10-05T10:00:00Z".into());
        let mut b = bead("v-2", "task", "in_progress", &[LANE_LABEL], None);
        b.created_at = Some("2026-10-09T09:00:00+02:00".into());
        let closed = bead("v-3", "task", "closed", &[LANE_LABEL], None);
        let other = bead("v-4", "task", "open", &[], None);
        let now = parse_rfc3339("2026-10-09T10:00:00Z").unwrap();
        assert_eq!(bead_age(&b, now), "3h00m");
        assert_eq!(
            lane_summary(&[a, b, closed, other], now),
            "2 open, oldest v-1 4d"
        );
        assert_eq!(lane_summary(&[], now), "0 open");
    }

    fn bead(id: &str, kind: &str, status: &str, labels: &[&str], parent: Option<&str>) -> Bead {
        Bead {
            id: id.into(),
            issue_type: kind.into(),
            status: status.into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            parent: parent.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn finds_exactly_one_open_tracker_epic() {
        let beads = vec![
            bead("t-1", "epic", "open", &[TRACKER_LABEL], None),
            bead("t-2", "epic", "closed", &[TRACKER_LABEL], None),
            // A decision may carry the tracker label too; only epics count.
            bead(
                "t-1.1",
                "decision",
                "open",
                &[TRACKER_LABEL, DECISION_LABEL],
                Some("t-1"),
            ),
        ];
        assert_eq!(Tracker::find(&beads), Tracker::One("t-1".into()));
    }

    #[test]
    fn none_or_several_trackers_are_a_problem() {
        assert_eq!(Tracker::find(&[]), Tracker::None);
        assert!(Tracker::None.problem().unwrap().contains("no open"));
        let beads = vec![
            bead("b", "epic", "open", &[TRACKER_LABEL], None),
            bead("a", "epic", "in_progress", &[TRACKER_LABEL], None),
        ];
        let t = Tracker::find(&beads);
        assert_eq!(t, Tracker::Several(vec!["a".into(), "b".into()]));
        assert!(t.problem().unwrap().contains("a, b"));
        assert_eq!(t.id(), None);
    }

    #[test]
    fn decisions_are_open_labelled_children_of_the_tracker() {
        assert!(is_decision(
            &bead("x", "decision", "open", &[DECISION_LABEL], Some("t")),
            "t"
        ));
        assert!(!is_decision(
            &bead("x", "decision", "closed", &[DECISION_LABEL], Some("t")),
            "t"
        ));
        assert!(!is_decision(
            &bead("x", "decision", "open", &[], Some("t")),
            "t"
        ));
        assert!(!is_decision(
            &bead("x", "decision", "open", &[DECISION_LABEL], Some("u")),
            "t"
        ));
        assert!(!is_decision(
            &bead("x", "decision", "open", &[DECISION_LABEL], None),
            "t"
        ));
    }
}
