//! When a task fires: once at an instant, `daily = "HH:MM"` or
//! `weekly = "<day> HH:MM"` in the host's local time zone.
//!
//! DST: a local time that does not exist (spring forward) resolves to the
//! first valid minute after it; one that occurs twice (fall back) to its
//! first occurrence. [`When::latest`] is the latest slot at or before now
//! (catch-up), [`When::next`] the first one after an instant (display).

use std::fmt;
use std::str::FromStr;

use jiff::civil::{Date, DateTime, Time, Weekday};
use jiff::tz::{AmbiguousOffset, TimeZone};
use jiff::{SignedDuration, Span, Timestamp};

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Once (`--at`, `--in`).
    At(Timestamp),
    Daily(Time),
    Weekly(Weekday, Time),
}

const DAYS: [(&str, Weekday); 7] = [
    ("mon", Weekday::Monday),
    ("tue", Weekday::Tuesday),
    ("wed", Weekday::Wednesday),
    ("thu", Weekday::Thursday),
    ("fri", Weekday::Friday),
    ("sat", Weekday::Saturday),
    ("sun", Weekday::Sunday),
];

fn invalid(msg: impl Into<String>) -> Error {
    Error::new("when_invalid", msg)
}

/// `HH:MM`, 24 h clock, two digits each.
pub fn parse_hhmm(s: &str) -> Result<Time, Error> {
    let bad = || invalid(format!("{s:?}: want HH:MM (24 h, e.g. 07:30)"));
    let b = s.as_bytes();
    if b.len() != 5
        || b[2] != b':'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 2 || c.is_ascii_digit())
    {
        return Err(bad());
    }
    let h: i8 = s[..2].parse().map_err(|_| bad())?;
    let m: i8 = s[3..].parse().map_err(|_| bad())?;
    Time::new(h, m, 0, 0).map_err(|_| bad())
}

/// `<n>m`, `<n>h` or `<n>d` (a day is 24 h), n > 0.
pub fn parse_duration(s: &str) -> Result<SignedDuration, Error> {
    let bad = || invalid(format!("{s:?}: want a duration like 90m, 2h or 3d"));
    if !s.is_ascii() {
        return Err(bad());
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    if num.is_empty() || !num.bytes().all(|c| c.is_ascii_digit()) {
        return Err(bad());
    }
    let n: i64 = num.parse().map_err(|_| bad())?;
    let mins = match unit {
        "m" => Some(n),
        "h" => n.checked_mul(60),
        "d" => n.checked_mul(24 * 60),
        _ => None,
    };
    match mins {
        Some(m) if m > 0 && m <= 3650 * 24 * 60 => Ok(SignedDuration::from_mins(m)),
        _ => Err(bad()),
    }
}

impl When {
    /// `daily = "HH:MM"`.
    pub fn daily(s: &str) -> Result<When, Error> {
        parse_hhmm(s).map(When::Daily)
    }

    /// `weekly = "mon HH:MM"`.
    pub fn weekly(s: &str) -> Result<When, Error> {
        let mut parts = s.split(' ');
        let (Some(day), Some(hm), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(invalid(format!("{s:?}: want \"<mon..sun> HH:MM\"")));
        };
        let wd = DAYS
            .iter()
            .find(|(n, _)| *n == day)
            .map(|(_, w)| *w)
            .ok_or_else(|| invalid(format!("{day:?}: want one of mon tue wed thu fri sat sun")))?;
        Ok(When::Weekly(wd, parse_hhmm(hm)?))
    }

    /// `--in 90m|2h|3d`: once, that long after `now`.
    pub fn after(s: &str, now: Timestamp) -> Result<When, Error> {
        let d = parse_duration(s)?;
        now.checked_add(d)
            .map(When::At)
            .map_err(|_| invalid(format!("{s:?}: out of range")))
    }

    /// `--at HH:MM` (the next occurrence after `now`) or
    /// `--at YYYY-MM-DDTHH:MM` (local, must lie after `now`).
    pub fn at(s: &str, now: Timestamp, tz: &TimeZone) -> Result<When, Error> {
        if s.len() == 5 {
            let t = When::Daily(parse_hhmm(s)?)
                .next(tz, now)
                .ok_or_else(|| invalid(format!("{s:?}: out of range")))?;
            return Ok(When::At(t));
        }
        let bad = || invalid(format!("{s:?}: want HH:MM or YYYY-MM-DDTHH:MM"));
        if s.len() != 16 || !s.is_ascii() || s.as_bytes()[10] != b'T' {
            return Err(bad());
        }
        let date: Date = s[..10].parse().map_err(|_| bad())?;
        let dt = date.to_datetime(parse_hhmm(&s[11..])?);
        let t = resolve(tz, dt).ok_or_else(bad)?;
        if t <= now {
            return Err(invalid(format!("{s:?} is in the past")));
        }
        Ok(When::At(t))
    }

    pub fn is_recurring(&self) -> bool {
        !matches!(self, When::At(_))
    }

    /// The slot on local date `d`, if this task has one that day.
    fn on(&self, tz: &TimeZone, d: Date) -> Option<Timestamp> {
        match *self {
            When::At(_) => None,
            When::Daily(t) => resolve(tz, d.to_datetime(t)),
            When::Weekly(wd, t) if d.weekday() == wd => resolve(tz, d.to_datetime(t)),
            When::Weekly(..) => None,
        }
    }

    /// The latest slot at or before `now`; a week asleep yields only the
    /// most recent one.
    pub fn latest(&self, tz: &TimeZone, now: Timestamp) -> Option<Timestamp> {
        if let When::At(t) = *self {
            return (t <= now).then_some(t);
        }
        let today = now.to_zoned(tz.clone()).date();
        // A slot resolved through a gap can move past midnight; start a day
        // ahead and walk back over two weeks of dates.
        (-1..15)
            .filter_map(|k| today.checked_sub(Span::new().days(k)).ok())
            .filter_map(|d| self.on(tz, d))
            .find(|t| *t <= now)
    }

    /// The first slot strictly after `after`.
    pub fn next(&self, tz: &TimeZone, after: Timestamp) -> Option<Timestamp> {
        if let When::At(t) = *self {
            return (t > after).then_some(t);
        }
        let today = after.to_zoned(tz.clone()).date();
        (-1..15)
            .filter_map(|k| today.checked_add(Span::new().days(k)).ok())
            .filter_map(|d| self.on(tz, d))
            .find(|t| *t > after)
    }
}

/// A local date and time as an instant, by the DST rules above.
pub fn resolve(tz: &TimeZone, dt: DateTime) -> Option<Timestamp> {
    let mut dt = dt;
    // Gaps are at most a day (Samoa 2011); step minute by minute out of it.
    for _ in 0..=(2 * 24 * 60) {
        match tz.to_ambiguous_timestamp(dt).offset() {
            AmbiguousOffset::Unambiguous { offset } => return offset.to_timestamp(dt).ok(),
            AmbiguousOffset::Fold { before, .. } => return before.to_timestamp(dt).ok(),
            AmbiguousOffset::Gap { .. } => {
                dt = dt.checked_add(SignedDuration::from_mins(1)).ok()?
            }
        }
    }
    None
}

fn hhmm(t: Time) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

/// `at <RFC 3339 UTC>`, `daily HH:MM`, `weekly <day> HH:MM`: the stored form.
impl fmt::Display for When {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            When::At(t) => write!(f, "at {t}"),
            When::Daily(t) => write!(f, "daily {}", hhmm(t)),
            When::Weekly(wd, t) => {
                let day = DAYS.iter().find(|(_, w)| *w == wd).map_or("?", |(n, _)| n);
                write!(f, "weekly {day} {}", hhmm(t))
            }
        }
    }
}

impl FromStr for When {
    type Err = Error;

    fn from_str(s: &str) -> Result<When, Error> {
        match s.split_once(' ') {
            Some(("at", t)) => t
                .parse::<Timestamp>()
                .map(When::At)
                .map_err(|e| invalid(format!("{t:?}: {e}"))),
            Some(("daily", t)) => When::daily(t),
            Some(("weekly", t)) => When::weekly(t),
            _ => Err(invalid(format!(
                "{s:?}: want \"at <ts>\", \"daily HH:MM\" or \"weekly <day> HH:MM\""
            ))),
        }
    }
}

impl serde::Serialize for When {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for When {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<When, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Europe/Berlin rules without the system tzdb: 2026 springs forward on
    /// Mar 29 02:00 -> 03:00 and falls back on Oct 25 03:00 -> 02:00.
    pub fn berlin() -> TimeZone {
        TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap()
    }

    pub fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn parses_hhmm_strictly() {
        assert_eq!(
            parse_hhmm("07:30").unwrap(),
            Time::new(7, 30, 0, 0).unwrap()
        );
        for bad in ["7:30", "24:00", "07:60", "0730", "07:3a", " 07:30", "+7:30"] {
            assert!(parse_hhmm(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_weekly_and_round_trips() {
        let w = When::weekly("mon 09:00").unwrap();
        assert_eq!(
            w,
            When::Weekly(Weekday::Monday, Time::new(9, 0, 0, 0).unwrap())
        );
        for bad in [
            "monday 09:00",
            "mon  09:00",
            "Mon 09:00",
            "mon",
            "mon 09:00 x",
        ] {
            assert!(When::weekly(bad).is_err(), "{bad}");
        }
        for w in [
            w,
            When::daily("23:59").unwrap(),
            When::At(ts("2026-10-24T05:00:00Z")),
        ] {
            assert_eq!(w.to_string().parse::<When>().unwrap(), w);
            let json = serde_json::to_string(&w).unwrap();
            assert_eq!(serde_json::from_str::<When>(&json).unwrap(), w);
        }
        assert_eq!(When::daily("08:30").unwrap().to_string(), "daily 08:30");
    }

    #[test]
    fn parses_durations() {
        assert_eq!(
            parse_duration("90m").unwrap(),
            SignedDuration::from_mins(90)
        );
        assert_eq!(parse_duration("2h").unwrap(), SignedDuration::from_hours(2));
        assert_eq!(
            parse_duration("3d").unwrap(),
            SignedDuration::from_hours(72)
        );
        for bad in [
            "\u{e9}",
            "1\u{e9}",
            "0m",
            "m",
            "2",
            "1.5h",
            "-1h",
            "2w",
            "99999999999999999999d",
            "",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn at_and_in() {
        let tz = berlin();
        let now = ts("2026-10-24T12:00:00Z"); // 14:00 CEST
        assert_eq!(
            When::after("2h", now).unwrap(),
            When::At(ts("2026-10-24T14:00:00Z"))
        );
        // HH:MM is the next occurrence: later today, or tomorrow.
        assert_eq!(
            When::at("15:00", now, &tz).unwrap(),
            When::At(ts("2026-10-24T13:00:00Z"))
        );
        assert_eq!(
            When::at("07:00", now, &tz).unwrap(),
            When::At(ts("2026-10-25T06:00:00Z"))
        );
        assert_eq!(
            When::at("2026-12-24T18:00", now, &tz).unwrap(),
            When::At(ts("2026-12-24T17:00:00Z"))
        );
        assert!(When::at("2026-10-24T13:00", now, &tz).is_err(), "past");
        assert!(When::at("2026-10-24 18:00", now, &tz).is_err());
        assert!(
            When::at("2026-10-24T\u{e9}:00", now, &tz).is_err(),
            "no panic on UTF-8"
        );
        assert!(When::at("\u{e9}\u{e9}:0", now, &tz).is_err());
    }

    #[test]
    fn spring_forward_fires_at_first_valid_minute() {
        let tz = berlin();
        let d = When::daily("02:30").unwrap();
        // 02:30 does not exist on 2026-03-29; 03:00 CEST = 01:00 UTC.
        assert_eq!(
            d.next(&tz, ts("2026-03-28T12:00:00Z")),
            Some(ts("2026-03-29T01:00:00Z"))
        );
        assert_eq!(
            d.latest(&tz, ts("2026-03-29T00:59:00Z")),
            Some(ts("2026-03-28T01:30:00Z"))
        );
        assert_eq!(
            d.latest(&tz, ts("2026-03-29T01:00:00Z")),
            Some(ts("2026-03-29T01:00:00Z"))
        );
        // The day after, back to 02:30 CEST.
        assert_eq!(
            d.next(&tz, ts("2026-03-29T01:00:00Z")),
            Some(ts("2026-03-30T00:30:00Z"))
        );
        // A slot inside the gap but not at its start still moves to 03:00.
        assert_eq!(
            resolve(&tz, "2026-03-29T02:59".parse().unwrap()),
            Some(ts("2026-03-29T01:00:00Z"))
        );
    }

    #[test]
    fn fall_back_fires_once_at_first_occurrence() {
        let tz = berlin();
        let d = When::daily("02:30").unwrap();
        // 02:30 occurs twice on 2026-10-25: first CEST (00:30 UTC), then CET.
        let first = ts("2026-10-25T00:30:00Z");
        assert_eq!(d.next(&tz, ts("2026-10-24T12:00:00Z")), Some(first));
        // The second 02:30 (01:30 UTC) is not a slot.
        assert_eq!(d.latest(&tz, ts("2026-10-25T01:45:00Z")), Some(first));
        assert_eq!(d.next(&tz, first), Some(ts("2026-10-26T01:30:00Z")));
    }

    #[test]
    fn weekly_slots() {
        let tz = berlin();
        let w = When::weekly("mon 09:00").unwrap();
        // Sat 2026-10-24; the Monday before is 10-19 (CEST), the next 10-26 (CET).
        let now = ts("2026-10-24T12:00:00Z");
        assert_eq!(w.latest(&tz, now), Some(ts("2026-10-19T07:00:00Z")));
        assert_eq!(w.next(&tz, now), Some(ts("2026-10-26T08:00:00Z")));
        // Exactly at the slot: it is the latest, and next is a week on.
        let slot = ts("2026-10-26T08:00:00Z");
        assert_eq!(w.latest(&tz, slot), Some(slot));
        assert_eq!(w.next(&tz, slot), Some(ts("2026-11-02T08:00:00Z")));
    }

    #[test]
    fn once_slots() {
        let tz = berlin();
        let t = ts("2026-10-24T05:00:00Z");
        let w = When::At(t);
        assert!(!w.is_recurring());
        assert_eq!(w.latest(&tz, ts("2026-10-24T04:59:59Z")), None);
        assert_eq!(w.latest(&tz, ts("2026-11-24T00:00:00Z")), Some(t));
        assert_eq!(w.next(&tz, ts("2026-10-24T04:00:00Z")), Some(t));
        assert_eq!(w.next(&tz, t), None);
    }

    #[test]
    fn utc_host_has_no_dst() {
        let d = When::daily("02:30").unwrap();
        assert_eq!(
            d.next(&TimeZone::UTC, ts("2026-03-29T00:00:00Z")),
            Some(ts("2026-03-29T02:30:00Z"))
        );
    }
}
