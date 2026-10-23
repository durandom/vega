//! The system appearance. macOS: `defaults read -g AppleInterfaceStyle`
//! prints `Dark` in dark mode and fails (key absent) in light mode. Linux:
//! GNOME's `color-scheme` when a desktop session is visible. Elsewhere, or
//! when nothing answers: `None`, which makes the host headless unless
//! theme.toml says otherwise.

use std::process::{Command, Stdio};

use crate::Mode;

pub fn mode() -> Option<Mode> {
    if cfg!(target_os = "macos") {
        let out = Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        return Some(macos(
            out.status.success(),
            &String::from_utf8_lossy(&out.stdout),
        ));
    }
    let desktop = ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()));
    if !desktop {
        return None;
    }
    let out = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| gnome(&String::from_utf8_lossy(&out.stdout)))
}

/// `defaults` succeeded with `Dark`: dark; the key is absent: light.
pub fn macos(success: bool, stdout: &str) -> Mode {
    if success && stdout.trim() == "Dark" {
        Mode::Dark
    } else {
        Mode::Light
    }
}

/// `'prefer-dark'` is dark; `'default'` and `'prefer-light'` are light.
pub fn gnome(stdout: &str) -> Mode {
    if stdout.contains("prefer-dark") {
        Mode::Dark
    } else {
        Mode::Light
    }
}

/// Headless: theme.toml decides; else a host without a readable appearance.
pub fn headless(pref: Option<bool>, os: Option<Mode>) -> bool {
    pref.unwrap_or(os.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_os_answers() {
        assert_eq!(macos(true, "Dark\n"), Mode::Dark);
        assert_eq!(macos(false, ""), Mode::Light);
        assert_eq!(gnome("'prefer-dark'\n"), Mode::Dark);
        assert_eq!(gnome("'default'\n"), Mode::Light);
        assert!(headless(None, None));
        assert!(!headless(None, Some(Mode::Dark)));
        assert!(headless(Some(true), Some(Mode::Dark)));
    }
}
