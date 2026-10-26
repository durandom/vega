//! The Uber-UAC's engaged window (docs/design/uber-uac.md §2, vega-lhc.209.12):
//! the Uber is active only while the Slayer talks to it. No login, heuristics
//! only:
//!
//! - **Start**: a Slayer capture on the Uber host. The capture hook writes its
//!   time to `$XDG_STATE_HOME/vega/uber/engaged.json`; the claim's registry key
//!   carries it as `engaged_at` (`uber_state::published`).
//! - **End**: `WINDOW` without Slayer input, or human input in a herdr client on
//!   any host after it: each host's vegad writes its last `pane.focused` time to
//!   `human.json`, and its beat carries it as `human_at`.
//!
//! `View::uber_engaged` decides from the registry; `decide` is the rule.

use std::path::{Path, PathBuf};

use serde_json::json;

/// The window after the last Slayer capture.
pub const WINDOW: i64 = 2 * 3600;

/// The capture hook's file, next to `claim.json`.
pub const INPUT: &str = "engaged.json";
/// vegad's copy of the newest `human_at` on the Uber host, for the hook.
pub const SEEN: &str = "human_seen.json";

fn uber_dir() -> Option<PathBuf> {
    Some(crate::uber_state::file()?.parent()?.to_path_buf())
}

/// `uber/engaged.json` = {at}: the last Slayer capture (the capture hook).
pub fn input_file() -> Option<PathBuf> {
    Some(uber_dir()?.join(INPUT))
}

/// `uber/human.json` = {at}: this host's last herdr pane focus (vegad).
pub fn human_file() -> Option<PathBuf> {
    Some(uber_dir()?.join("human.json"))
}

/// `uber/human_seen.json` = {host, at}: the newest `human_at` of any host,
/// as vegad on the Uber host last read it (for the capture hook).
pub fn seen_file() -> Option<PathBuf> {
    Some(uber_dir()?.join(SEEN))
}

/// The `at` of one of these files.
pub fn read_at(path: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()?["at"].as_i64()
}

/// The (host, at) of `human_seen.json`.
pub fn read_seen(path: &Path) -> Option<(String, i64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((v["host"].as_str()?.to_string(), v["at"].as_i64()?))
}

fn save(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, value.to_string())?;
    std::fs::rename(tmp, path)
}

/// Writes `{at}` atomically.
pub fn write_at(path: &Path, at: i64) -> std::io::Result<()> {
    save(path, &json!({"at": at}))
}

/// Writes `{host, at}` atomically, only when it changed.
pub fn write_seen(path: &Path, host: &str, at: i64) -> std::io::Result<()> {
    if read_seen(path).is_some_and(|(h, a)| h == host && a == at) {
        return Ok(());
    }
    save(path, &json!({"host": host, "at": at}))
}

/// Whether the Uber is engaged, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Engaged {
    /// `input`: the last Slayer capture; `until`: when the window ends.
    On {
        input: i64,
        until: i64,
    },
    Off(Idle),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Idle {
    /// No Slayer capture since the claim.
    Never,
    /// `WINDOW` passed without Slayer input; `at`: when it ended.
    Expired { at: i64 },
    /// Human input in a herdr client on `host` at `at`, after the last capture.
    Elsewhere { host: String, at: i64 },
}

impl Engaged {
    pub fn on(&self) -> bool {
        matches!(self, Engaged::On { .. })
    }

    /// `yes, until 15:02Z (last Slayer input 3m ago)` or `no (...)`.
    pub fn line(&self, now: i64) -> String {
        match self {
            Engaged::On { input, until } => format!(
                "yes, until {} (last Slayer input {} ago)",
                hhmm(*until),
                ago(now - input)
            ),
            Engaged::Off(idle) => format!("no ({})", idle.reason()),
        }
    }
}

impl Idle {
    /// Why the Uber is idle, one phrase.
    pub fn reason(&self) -> String {
        match self {
            Idle::Never => "no Slayer input since the claim".into(),
            Idle::Expired { at } => format!("2h without Slayer input, since {}", hhmm(*at)),
            Idle::Elsewhere { host, at } => {
                format!("Slayer input in herdr on {host} at {}", hhmm(*at))
            }
        }
    }
}

/// The rule: `input` is the last Slayer capture (at or after the claim),
/// `human` the newest (host, human_at) of any host.
pub fn decide(input: Option<i64>, human: Option<(&str, i64)>, now: i64) -> Engaged {
    let Some(input) = input else {
        return Engaged::Off(Idle::Never);
    };
    let until = input + WINDOW;
    if now >= until {
        return Engaged::Off(Idle::Expired { at: until });
    }
    match human {
        Some((host, at)) if at > input => Engaged::Off(Idle::Elsewhere {
            host: host.into(),
            at,
        }),
        _ => Engaged::On { input, until },
    }
}

/// `15:02Z` (UTC).
fn hhmm(t: i64) -> String {
    let s = t.rem_euclid(86400);
    format!("{:02}:{:02}Z", s / 3600, s % 3600 / 60)
}

fn ago(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// The capture hook's notice when a window opens (a typed prompt only):
/// `before` is the state just before this capture. `None` while engaged.
pub fn notice(before: &Engaged) -> Option<String> {
    let Engaged::Off(idle) = before else {
        return None;
    };
    Some(match idle {
        Idle::Never => "vega uber: engaged now (first Slayer input since the claim); \
                        pull the brief once (vega uber)."
            .into(),
        _ => format!(
            "vega uber: you were idle ({}); tell the Slayer so once. \
             Engaged again now: pull the brief once (vega uber).",
            idle.reason()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_opens_on_input_and_ends_after_2h_or_on_input_elsewhere() {
        assert_eq!(decide(None, None, 100), Engaged::Off(Idle::Never));
        let on = decide(Some(1000), None, 1000 + WINDOW - 1);
        assert_eq!(
            on,
            Engaged::On {
                input: 1000,
                until: 1000 + WINDOW
            }
        );
        assert!(on.on());
        assert_eq!(
            decide(Some(1000), None, 1000 + WINDOW),
            Engaged::Off(Idle::Expired { at: 1000 + WINDOW })
        );
        // Human input before the capture does not end it; after it does.
        assert!(decide(Some(1000), Some(("proteus", 1000)), 1100).on());
        assert_eq!(
            decide(Some(1000), Some(("proteus", 1001)), 1100),
            Engaged::Off(Idle::Elsewhere {
                host: "proteus".into(),
                at: 1001
            })
        );
    }

    #[test]
    fn lines_and_notices_name_the_reason() {
        let now = 86400 + 13 * 3600;
        assert_eq!(
            decide(Some(now - 180), None, now).line(now),
            "yes, until 14:57Z (last Slayer input 3m ago)"
        );
        assert_eq!(
            decide(Some(now - 60), Some(("proteus", now - 30)), now).line(now),
            "no (Slayer input in herdr on proteus at 12:59Z)"
        );
        assert_eq!(notice(&decide(Some(now - 60), None, now)), None);
        assert!(notice(&Engaged::Off(Idle::Never))
            .unwrap()
            .contains("first Slayer input"));
        let n = notice(&decide(Some(now - WINDOW), None, now)).unwrap();
        assert!(n.contains("you were idle (2h without Slayer input, since 13:00Z)"));
    }

    #[test]
    fn files_round_trip_and_seen_writes_only_changes() {
        let dir = tempfile::tempdir().unwrap();
        let at = dir.path().join("uber/engaged.json");
        assert_eq!(read_at(&at), None);
        write_at(&at, 42).unwrap();
        assert_eq!(read_at(&at), Some(42));
        let seen = dir.path().join("uber/human_seen.json");
        write_seen(&seen, "proteus", 7).unwrap();
        let before = std::fs::metadata(&seen).unwrap().modified().unwrap();
        write_seen(&seen, "proteus", 7).unwrap();
        assert_eq!(
            std::fs::metadata(&seen).unwrap().modified().unwrap(),
            before
        );
        assert_eq!(read_seen(&seen), Some(("proteus".into(), 7)));
    }
}
