//! Catppuccin palettes (Macchiato dark, Latte light, picked from the mode
//! `vega theme` applied) + semantic mappings.
//! The full palette and a few helpers are kept complete for future views.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use ratatui::style::Color;
use vega_theme_sync::{state, Env, Mode};

/// One Catppuccin flavour.
#[derive(Debug, PartialEq)]
pub struct Palette {
    pub base: Color,
    pub mantle: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub surface2: Color,
    pub overlay0: Color,
    pub overlay1: Color,
    pub text: Color,
    pub subtext: Color,
    pub rosewater: Color,
    pub red: Color,
    pub maroon: Color,
    pub peach: Color,
    pub yellow: Color,
    pub green: Color,
    pub teal: Color,
    pub sky: Color,
    pub blue: Color,
    pub lavender: Color,
    pub mauve: Color,
    pub flamingo: Color,
}

/// Catppuccin Macchiato, for a dark terminal.
pub const MACCHIATO: Palette = Palette {
    base: Color::Rgb(0x24, 0x27, 0x3a),
    mantle: Color::Rgb(0x1e, 0x20, 0x30),
    surface0: Color::Rgb(0x36, 0x3a, 0x4f),
    surface1: Color::Rgb(0x49, 0x4d, 0x64),
    surface2: Color::Rgb(0x5b, 0x60, 0x78),
    overlay0: Color::Rgb(0x6e, 0x73, 0x8d),
    overlay1: Color::Rgb(0x8a, 0x8f, 0xa8),
    text: Color::Rgb(0xca, 0xd3, 0xf5),
    subtext: Color::Rgb(0xb8, 0xc0, 0xe0),
    rosewater: Color::Rgb(0xf4, 0xdb, 0xd6),
    red: Color::Rgb(0xed, 0x87, 0x96),
    maroon: Color::Rgb(0xee, 0x99, 0xa0),
    peach: Color::Rgb(0xf5, 0xa9, 0x7f),
    yellow: Color::Rgb(0xee, 0xd4, 0x9f),
    green: Color::Rgb(0xa6, 0xda, 0x95),
    teal: Color::Rgb(0x8b, 0xd5, 0xca),
    sky: Color::Rgb(0x91, 0xd7, 0xe3),
    blue: Color::Rgb(0x8a, 0xad, 0xf4),
    lavender: Color::Rgb(0xb7, 0xbd, 0xf8),
    mauve: Color::Rgb(0xc6, 0xa0, 0xf6),
    flamingo: Color::Rgb(0xf0, 0xc6, 0xc6),
};

/// Catppuccin Latte, for a light terminal.
pub const LATTE: Palette = Palette {
    base: Color::Rgb(0xef, 0xf1, 0xf5),
    mantle: Color::Rgb(0xe6, 0xe9, 0xef),
    surface0: Color::Rgb(0xcc, 0xd0, 0xda),
    surface1: Color::Rgb(0xbc, 0xc0, 0xcc),
    surface2: Color::Rgb(0xac, 0xb0, 0xbe),
    overlay0: Color::Rgb(0x9c, 0xa0, 0xb0),
    overlay1: Color::Rgb(0x8c, 0x8f, 0xa1),
    text: Color::Rgb(0x4c, 0x4f, 0x69),
    subtext: Color::Rgb(0x5c, 0x5f, 0x77),
    rosewater: Color::Rgb(0xdc, 0x8a, 0x78),
    red: Color::Rgb(0xd2, 0x0f, 0x39),
    maroon: Color::Rgb(0xe6, 0x45, 0x53),
    peach: Color::Rgb(0xfe, 0x64, 0x0b),
    yellow: Color::Rgb(0xdf, 0x8e, 0x1d),
    green: Color::Rgb(0x40, 0xa0, 0x2b),
    teal: Color::Rgb(0x17, 0x92, 0x99),
    sky: Color::Rgb(0x04, 0xa5, 0xe5),
    blue: Color::Rgb(0x1e, 0x66, 0xf5),
    lavender: Color::Rgb(0x72, 0x87, 0xfd),
    mauve: Color::Rgb(0x88, 0x39, 0xef),
    flamingo: Color::Rgb(0xdd, 0x78, 0x78),
};

impl Palette {
    /// Latte for light; Macchiato for dark or an unknown mode.
    pub fn for_mode(mode: Option<Mode>) -> &'static Palette {
        match mode {
            Some(Mode::Light) => &LATTE,
            _ => &MACCHIATO,
        }
    }
}

static LIGHT: AtomicBool = AtomicBool::new(false);

/// The palette in use: the mode `vega theme` applied, as `ModeWatch` last read it.
pub fn p() -> &'static Palette {
    let light = LIGHT.load(Ordering::Relaxed);
    Palette::for_mode(Some(if light { Mode::Light } else { Mode::Dark }))
}

/// Follows `$XDG_STATE_HOME/vega/theme.json` (vegad rewrites it when the OS
/// mode changes): a stat per poll, a read only when its mtime moved.
pub struct ModeWatch {
    env: Option<Env>,
    seen: Option<Option<SystemTime>>,
}

impl ModeWatch {
    pub fn new(env: Option<Env>) -> ModeWatch {
        ModeWatch { env, seen: None }
    }

    /// The applied mode, if the state file changed since the last call
    /// (always on the first); `Some(None)` when it is missing or unreadable.
    pub fn changed(&mut self) -> Option<Option<Mode>> {
        let path = self.env.as_ref().map(state::path);
        let mtime = path
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        if self.seen == Some(mtime) {
            return None;
        }
        self.seen = Some(mtime);
        Some(self.env.as_ref().and_then(state::read).map(|a| a.mode))
    }

    /// Switches the global palette when the applied mode changed.
    pub fn poll(&mut self) {
        if let Some(mode) = self.changed() {
            LIGHT.store(mode == Some(Mode::Light), Ordering::Relaxed);
        }
    }
}

/// Column/group accent for a status.
pub fn status_color(status: &str) -> Color {
    match status {
        "open" => p().blue,
        "in_progress" => p().yellow,
        "blocked" => p().red,
        "deferred" => p().overlay1,
        "closed" => p().green,
        "pinned" => p().mauve,
        "hooked" => p().teal,
        _ => p().lavender,
    }
}

/// Priority 0..=4 color (0 = critical/red .. 4 = backlog/overlay).
pub fn priority_color(prio: u8) -> Color {
    match prio {
        0 => p().red,
        1 => p().peach,
        2 => p().yellow,
        3 => p().teal,
        _ => p().overlay0,
    }
}

pub fn priority_glyph(p: u8) -> &'static str {
    match p {
        0 => "P0",
        1 => "P1",
        2 => "P2",
        3 => "P3",
        _ => "P4",
    }
}

/// Dim agent-state hint color (idle/working/blocked/unknown), never authoritative.
pub fn agent_color(state: &str) -> Color {
    match state {
        "blocked" => p().red,
        "working" => p().blue,
        "idle" => p().green,
        _ => p().overlay0,
    }
}

/// The theme's symbol for `t` (vega_herdr::theme, `~/.config/vega/theme.toml`),
/// one space after it.
pub fn icon(t: &vega_herdr::theme::Topic) -> String {
    vega_herdr::theme::symbols().icon(t)
}

/// Terminal cells `s` takes: a theme symbol may be any string.
pub fn cells(s: &str) -> usize {
    ratatui::text::Span::raw(s).width()
}

/// A short tag for an issue type, one space after it: a letter, the theme's
/// epic or decision symbol, `·` for a plain task.
pub fn type_glyph(t: &str) -> String {
    match t {
        "epic" => icon(&vega_herdr::theme::EPIC),
        "decision" => icon(&vega_herdr::theme::DECISION_BEAD),
        "bug" => "B ".into(),
        "feature" => "F ".into(),
        "chore" => "C ".into(),
        "spike" => "S ".into(),
        "story" => "Y ".into(),
        "milestone" => "M ".into(),
        _ => "· ".into(),
    }
}

pub fn type_color(t: &str) -> Color {
    match t {
        "bug" => p().red,
        "feature" => p().green,
        "epic" => p().mauve,
        "chore" => p().overlay1,
        "decision" => p().sky,
        "spike" => p().peach,
        "story" => p().teal,
        "milestone" => p().yellow,
        _ => p().overlay0,
    }
}

/// The marker before the title of a bead blocked by another bead.
pub fn blocked_span(b: &crate::bd::types::Bead) -> Option<ratatui::text::Span<'static>> {
    b.is_blocked.then(|| {
        ratatui::text::Span::styled(
            icon(&vega_herdr::theme::BLOCKED_BY),
            ratatui::style::Style::default().fg(p().red),
        )
    })
}

/// Columns taken by `blocked_span`.
pub fn blocked_width(b: &crate::bd::types::Bead) -> usize {
    if b.is_blocked {
        cells(&icon(&vega_herdr::theme::BLOCKED_BY))
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(env: &Env, mode: Mode) {
        let a = state::Applied {
            mode,
            source: "os".into(),
            at: 1,
        };
        state::write(env, &a).unwrap();
    }

    #[test]
    fn light_is_latte_dark_or_unknown_is_macchiato() {
        assert_eq!(Palette::for_mode(Some(Mode::Light)), &LATTE);
        assert_eq!(Palette::for_mode(Some(Mode::Dark)), &MACCHIATO);
        assert_eq!(Palette::for_mode(None), &MACCHIATO);
    }

    #[test]
    fn watch_reads_state_falls_back_when_missing_and_follows_changes() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::at(dir.path());
        let mut w = ModeWatch::new(Some(env.clone()));
        assert_eq!(
            w.changed(),
            Some(None),
            "missing state: unknown, dark palette"
        );
        assert_eq!(w.changed(), None, "unchanged: no re-read");

        applied(&env, Mode::Light);
        assert_eq!(w.changed(), Some(Some(Mode::Light)));

        // A rewrite within the same mtime tick would go unseen; force a new one.
        std::fs::write(state::path(&env), "not json").unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(state::path(&env))
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(w.changed(), Some(None), "unreadable state: unknown");

        assert_eq!(ModeWatch::new(None).changed(), Some(None), "no HOME");
    }
}
