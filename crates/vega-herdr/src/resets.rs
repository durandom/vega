//! The Slayer's provider quota resets, for the capacity forecast.
//!
//! The Slayer owns resets (Claude, Codex) and triggers them himself; vega
//! only records what he enters: resets owned, the date he plans to use one
//! by, the resets used, and an override that silences the spawn warning.
//! vega never contacts a provider about them.
//!
//! One JSON file, `$XDG_STATE_HOME/vega/capacity.json`, written atomically
//! (temp file, rename). Dates are UTC days (`YYYY-MM-DD`), valid through
//! their end; "this week" is the coming seven days.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Used resets kept per provider.
const KEEP_USED: usize = 10;

/// One provider's resets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Resets the Slayer holds and has not used.
    #[serde(default)]
    pub owned: u32,
    /// The UTC day by which he plans to use one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The UTC day through which the spawn warning stays silent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_until: Option<String>,
    /// When resets were used (Unix seconds), oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used: Vec<i64>,
}

/// Why a thin window does not warn.
#[derive(Debug, Clone, PartialEq)]
pub enum Cover {
    Override(String),
    Plan(String),
}

impl Cover {
    /// `reset planned this week`, `reset planned by 2026-10-20`,
    /// `override until 2026-10-14`.
    pub fn text(&self, now: i64) -> String {
        match self {
            Cover::Override(d) => format!("override until {d}"),
            Cover::Plan(d) if day_end(d).is_some_and(|e| e <= week_end(now)) => {
                "reset planned this week".into()
            }
            Cover::Plan(d) => format!("reset planned by {d}"),
        }
    }
}

impl Entry {
    /// An active override, else a plan with a reset owned, both through
    /// their day.
    pub fn cover(&self, now: i64) -> Option<Cover> {
        let live = |d: &Option<String>| d.clone().filter(|d| day_end(d).is_some_and(|e| e >= now));
        if let Some(d) = live(&self.override_until) {
            return Some(Cover::Override(d));
        }
        live(&self.plan).filter(|_| self.owned > 0).map(Cover::Plan)
    }

    /// A plan whose day passed without a used reset.
    pub fn plan_passed(&self, now: i64) -> Option<&str> {
        self.plan
            .as_deref()
            .filter(|d| day_end(d).is_some_and(|e| e < now))
    }
}

/// Every provider's resets, by token-burn provider name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub providers: BTreeMap<String, Entry>,
}

impl Ledger {
    pub fn cover(&self, provider: &str, now: i64) -> Option<Cover> {
        self.providers.get(provider)?.cover(now)
    }

    /// Records `n` resets owned.
    pub fn set_owned(&mut self, provider: &str, n: u32) {
        self.entry(provider).owned = n;
    }

    /// Plans to use a reset by `day`; needs one owned.
    pub fn plan(&mut self, provider: &str, day: &str) -> Result<(), String> {
        check_day(day)?;
        let e = self.entry(provider);
        if e.owned == 0 {
            return Err(format!("no {provider} reset owned"));
        }
        e.plan = Some(day.into());
        Ok(())
    }

    /// Records one reset used at `now`: one fewer owned, the plan done.
    pub fn used(&mut self, provider: &str, now: i64) -> Result<(), String> {
        let e = self.entry(provider);
        if e.owned == 0 {
            return Err(format!("no {provider} reset owned"));
        }
        e.owned -= 1;
        e.plan = None;
        e.used.push(now);
        let excess = e.used.len().saturating_sub(KEEP_USED);
        e.used.drain(..excess);
        Ok(())
    }

    /// Silences the spawn warning through `day`.
    pub fn override_until(&mut self, provider: &str, day: &str) -> Result<(), String> {
        check_day(day)?;
        self.entry(provider).override_until = Some(day.into());
        Ok(())
    }

    pub fn clear_plan(&mut self, provider: &str) {
        self.entry(provider).plan = None;
    }

    pub fn clear_override(&mut self, provider: &str) {
        self.entry(provider).override_until = None;
    }

    fn entry(&mut self, provider: &str) -> &mut Entry {
        self.providers.entry(provider.into()).or_default()
    }
}

/// `$XDG_STATE_HOME/vega/capacity.json` (default `~/.local/state`).
pub fn path() -> Option<PathBuf> {
    let state = env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("HOME").ok().map(|h| format!("{h}/.local/state")))?;
    Some(Path::new(&state).join("vega").join("capacity.json"))
}

/// The ledger at `file`; empty when there is none.
pub fn load(file: &Path) -> Result<Ledger, String> {
    match std::fs::read_to_string(file) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{}: {e}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::default()),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

/// The ledger at the default path; empty without one. `Err` is the reason
/// it is unreadable.
pub fn read() -> Result<Ledger, String> {
    match path() {
        Some(p) => load(&p),
        None => Ok(Ledger::default()),
    }
}

/// Writes `ledger` to `file` atomically.
pub fn save(file: &Path, ledger: &Ledger) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(ledger).expect("ledger serializes");
    std::fs::write(&tmp, json + "\n")?;
    std::fs::rename(tmp, file)
}

/// `YYYY-MM-DD` or an error naming the format.
pub fn check_day(day: &str) -> Result<(), String> {
    day_end(day)
        .filter(|_| day.len() == 10)
        .map(|_| ())
        .ok_or_else(|| format!("{day} is no date; use YYYY-MM-DD"))
}

/// The last second of UTC day `YYYY-MM-DD`.
pub fn day_end(day: &str) -> Option<i64> {
    let m: i64 = day.get(5..7)?.parse().ok()?;
    let d: i64 = day.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    crate::capacity::parse_rfc3339(&format!("{day}T23:59:59Z"))
}

/// The UTC day of `unix` as `YYYY-MM-DD`.
pub fn day_of(unix: i64) -> String {
    // Civil from days (Howard Hinnant).
    let z = unix.div_euclid(86400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The last second of the coming seven UTC days, today included: "this
/// week" said on a Sunday means the week ahead, not the day.
pub fn week_end(now: i64) -> i64 {
    (now.div_euclid(86400) + 6) * 86400 + 86399
}

/// The last day of `week_end`, as `YYYY-MM-DD`.
pub fn this_week(now: i64) -> String {
    day_of(week_end(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sunday 2026-10-11 12:00 UTC.
    const SUN: i64 = 1_791_720_000;

    #[test]
    fn days_and_weeks() {
        assert_eq!(day_of(SUN), "2026-10-11");
        // Said on a Sunday: the week ahead.
        assert_eq!(this_week(SUN), "2026-10-17");
        assert_eq!(day_end("2026-10-11"), Some(SUN + 12 * 3600 - 1));
        assert!(check_day("2026-10-11").is_ok());
        assert!(check_day("2026-13-01").is_err());
        assert!(check_day("next week").is_err());
        assert!(check_day("2026-10-1").is_err());
    }

    #[test]
    fn plan_needs_a_reset_and_covers_through_its_day() {
        let mut l = Ledger::default();
        assert_eq!(
            l.plan("claude", "2026-10-11"),
            Err("no claude reset owned".into())
        );
        l.set_owned("claude", 1);
        l.plan("claude", "2026-10-11").unwrap();
        let c = l.cover("claude", SUN).unwrap();
        assert_eq!(c.text(SUN), "reset planned this week");
        assert_eq!(c.text(SUN - 7 * 86400), "reset planned by 2026-10-11");
        // Past its day: no cover, and the plan is reported as passed.
        assert_eq!(l.cover("claude", SUN + 86400), None);
        assert_eq!(
            l.providers["claude"].plan_passed(SUN + 86400),
            Some("2026-10-11")
        );
        // Without a reset left the plan covers nothing.
        l.set_owned("claude", 0);
        assert_eq!(l.cover("claude", SUN), None);
        assert_eq!(l.cover("codex", SUN), None);
    }

    #[test]
    fn used_spends_one_and_ends_the_plan() {
        let mut l = Ledger::default();
        assert!(l.used("codex", SUN).is_err());
        l.set_owned("codex", 2);
        l.plan("codex", "2026-10-15").unwrap();
        l.used("codex", SUN).unwrap();
        let e = &l.providers["codex"];
        assert_eq!(
            (e.owned, e.plan.as_deref(), e.used.as_slice()),
            (1, None, &[SUN][..])
        );
        for i in 0..20 {
            l.set_owned("codex", 1);
            l.used("codex", SUN + i).unwrap();
        }
        assert_eq!(l.providers["codex"].used.len(), KEEP_USED);
    }

    #[test]
    fn override_wins_without_resets() {
        let mut l = Ledger::default();
        l.override_until("claude", "2026-10-14").unwrap();
        assert_eq!(
            l.cover("claude", SUN).map(|c| c.text(SUN)),
            Some("override until 2026-10-14".into())
        );
        l.clear_override("claude");
        assert_eq!(l.cover("claude", SUN), None);
    }

    #[test]
    fn save_and_load_round_trip_in_a_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("vega/capacity.json");
        assert_eq!(load(&file), Ok(Ledger::default()));
        let mut l = Ledger::default();
        l.set_owned("claude", 1);
        l.plan("claude", "2026-10-18").unwrap();
        save(&file, &l).unwrap();
        assert_eq!(load(&file), Ok(l));
        assert!(!file.with_extension("json.tmp").exists());
        std::fs::write(&file, "{").unwrap();
        assert!(load(&file).is_err());
    }
}
