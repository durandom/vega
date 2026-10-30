//! The OS dark/light mode for every TUI (docs/design/theme.md, vega-lhc.86).
//!
//! `vega theme` (plugins/theme) and vegad's watcher (plugins/post/src/theme.rs)
//! share this crate:
//!
//! - [`prefs`]: `~/.config/vega/theme.toml`, what the user wants (`mode =
//!   "auto"|"dark"|"light"`, the control host, per-tool theme names).
//! - [`state`]: `$XDG_STATE_HOME/vega/theme.json`, the mode last applied.
//! - [`os`]: the system appearance (macOS `defaults`, GNOME `gsettings`).
//! - [`adapters`]: one per tool; each reads and writes only its own key.
//! - herdr's config.toml is rendered by `vega config` (plugins/config), not here.
//! - [`resolve`] and [`Watcher`]: which mode is due, and when to apply it.

use std::path::{Path, PathBuf};

pub mod adapters;
pub mod files;
pub mod os;
mod pi;
pub mod prefs;
pub mod state;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim() {
            "dark" => Some(Mode::Dark),
            "light" => Some(Mode::Light),
            _ => None,
        }
    }
}

/// What the user asked for: a fixed mode, or follow the OS (or the control
/// host on a headless one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pref {
    Auto,
    Fixed(Mode),
}

impl Pref {
    pub fn as_str(self) -> &'static str {
        match self {
            Pref::Auto => "auto",
            Pref::Fixed(m) => m.as_str(),
        }
    }

    pub fn parse(s: &str) -> Option<Pref> {
        match s.trim() {
            "auto" => Some(Pref::Auto),
            other => Mode::parse(other).map(Pref::Fixed),
        }
    }
}

/// Where a due mode comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `mode = "dark"|"light"` in theme.toml.
    Fixed,
    /// This host's system appearance.
    Os,
    /// The control host's applied mode in the registry (headless hosts).
    Registry(String),
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Fixed => "fixed".into(),
            Source::Os => "os".into(),
            Source::Registry(h) => format!("registry:{h}"),
        }
    }
}

/// The mode due now, or `None` when nothing is known (a headless host whose
/// control host published nothing fresh): then nothing changes.
pub fn resolve(
    pref: Pref,
    headless: bool,
    os: Option<Mode>,
    control: &str,
    remote: Option<Mode>,
) -> Option<(Mode, Source)> {
    match pref {
        Pref::Fixed(m) => Some((m, Source::Fixed)),
        Pref::Auto if headless => remote.map(|m| (m, Source::Registry(control.to_string()))),
        Pref::Auto => os.map(|m| (m, Source::Os)),
    }
}

/// Applies `mode` to every tool and records it as this host's applied mode.
/// The adapters' failures are in the outcomes; the error is the state write.
pub fn apply_due(
    env: &Env,
    prefs: &prefs::Prefs,
    mode: Mode,
    source: &Source,
) -> Result<Vec<(&'static str, adapters::Outcome)>, String> {
    let out = adapters::apply_all(env, prefs, mode);
    state::write(
        env,
        &state::Applied {
            mode,
            source: source.label(),
            at: state::unix_now(),
        },
    )?;
    Ok(out)
}

/// vegad's decision per tick: apply when the due mode differs from the last
/// one applied. The first due mode after start is always applied (adapters
/// are idempotent), so a change while vegad was down is caught.
#[derive(Debug, Default)]
pub struct Watcher {
    applied: Option<Mode>,
}

impl Watcher {
    /// The mode to apply now, if any. Call [`Watcher::applied`] after a
    /// successful apply; a failed one is retried next tick.
    pub fn step(&self, due: Option<Mode>) -> Option<Mode> {
        due.filter(|m| self.applied != Some(*m))
    }

    /// The mode this watcher applied last.
    pub fn last(&self) -> Option<Mode> {
        self.applied
    }

    pub fn applied(&mut self, m: Mode) {
        self.applied = Some(m);
    }
}

/// The directories adapters and stores resolve against. Tests point every
/// field into a temp dir; nothing here falls back to the real home.
#[derive(Debug, Clone)]
pub struct Env {
    pub home: PathBuf,
    /// Installed pi package root, resolved only for the real environment.
    pub pi_package: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME` (default `~/.config`).
    pub config: PathBuf,
    /// `$XDG_STATE_HOME` (default `~/.local/state`).
    pub state: PathBuf,
}

impl Env {
    /// This user's directories from the environment.
    pub fn from_env() -> Option<Env> {
        let home = PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?);
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Some(Env {
            pi_package: pi::installed_package(),
            config: var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
            state: var("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")),
            home,
        })
    }

    /// Everything under `home`, XDG defaults (tests).
    pub fn at(home: &Path) -> Env {
        Env {
            pi_package: None,
            home: home.to_path_buf(),
            config: home.join(".config"),
            state: home.join(".local/state"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_wins_auto_follows_os_or_control() {
        let r = |p, h, os, rem| resolve(p, h, os, "proteus", rem).map(|(m, s)| (m, s.label()));
        assert_eq!(
            r(Pref::Fixed(Mode::Light), true, Some(Mode::Dark), None),
            Some((Mode::Light, "fixed".into()))
        );
        assert_eq!(
            r(Pref::Auto, false, Some(Mode::Dark), Some(Mode::Light)),
            Some((Mode::Dark, "os".into()))
        );
        assert_eq!(
            r(Pref::Auto, true, None, Some(Mode::Light)),
            Some((Mode::Light, "registry:proteus".into()))
        );
        // Nothing known: nothing changes.
        assert_eq!(r(Pref::Auto, true, Some(Mode::Dark), None), None);
        assert_eq!(r(Pref::Auto, false, None, Some(Mode::Dark)), None);
    }

    #[test]
    fn watcher_applies_first_and_on_change_only() {
        let mut w = Watcher::default();
        assert_eq!(w.step(None), None);
        assert_eq!(w.step(Some(Mode::Dark)), Some(Mode::Dark));
        // Not marked applied (the apply failed): due again.
        assert_eq!(w.step(Some(Mode::Dark)), Some(Mode::Dark));
        w.applied(Mode::Dark);
        assert_eq!(w.step(Some(Mode::Dark)), None);
        assert_eq!(w.step(None), None);
        assert_eq!(w.step(Some(Mode::Light)), Some(Mode::Light));
        w.applied(Mode::Light);
        assert_eq!(w.step(Some(Mode::Light)), None);
    }

    #[test]
    fn modes_round_trip() {
        for p in [
            Pref::Auto,
            Pref::Fixed(Mode::Dark),
            Pref::Fixed(Mode::Light),
        ] {
            assert_eq!(Pref::parse(p.as_str()), Some(p));
        }
        assert_eq!(Pref::parse("system"), None);
    }
}
