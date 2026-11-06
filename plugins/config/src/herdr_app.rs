//! herdr-app's home `~/.config/herdr-app/`, owned by vega (vega-0h4c.1,
//! docs/design/config.md "herdr-app").
//!
//! Three units: `herdr-app.keys` (the whole `keys.json`), `herdr-app.settings`
//! (seeded keys of `settings.json`, which the app's Settings screen writes too)
//! and `herdr-app.themes` (`themes/<name>.json`, one per source). The app
//! reloads them live and reports a file it could not apply as `<stem>.error`
//! next to it; [`validate_keys`] and [`validate_theme`] catch what it accepts
//! silently (an unknown command shows only as a toast when pressed).

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use crate::Env;

/// `$XDG_CONFIG_HOME/herdr-app`, the app's home (`HERDR_APP_HOME` is not followed).
pub fn home(env: &Env) -> PathBuf {
    env.config.join("herdr-app")
}

/// The app command names (`herdr-app commands`, works without a window);
/// None without herdr-app.
pub fn commands(bin: &OsStr) -> Result<Option<BTreeSet<String>>, String> {
    let out = match Command::new(bin).arg("commands").output() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("herdr-app commands: {e}")),
        Ok(o) if !o.status.success() => {
            return Err(format!("herdr-app commands exited {}", o.status))
        }
        Ok(o) => o,
    };
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("herdr-app commands: not JSON: {e}"))?;
    Ok(Some(
        v.as_array()
            .ok_or("herdr-app commands: not a list")?
            .iter()
            .filter_map(|c| c["name"].as_str().map(str::to_string))
            .collect(),
    ))
}

/// The mod folders under `home/mods` (`<mod>.<command>` actions).
pub fn mods(home: &Path) -> BTreeSet<String> {
    fs::read_dir(home.join("mods"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Problems with a `keys.json`: its shape, and with `commands`, every action
/// naming neither an app command, `send:`, `terminal` nor a mod in `mods`.
pub fn validate_keys(
    text: &str,
    commands: Option<&BTreeSet<String>>,
    mods: &BTreeSet<String>,
) -> Vec<String> {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return vec![format!("keys.json: not JSON: {e}")],
    };
    let Some(sections) = v.as_object() else {
        return vec!["keys.json: not an object of sections".into()];
    };
    let mut out = Vec::new();
    for (section, keys) in sections {
        let Some(keys) = keys.as_object() else {
            out.push(format!("{section}: not an object of keys"));
            continue;
        };
        for (key, action) in keys {
            let name = match action {
                Value::String(s) => s.split(':').next().unwrap_or("").to_string(),
                Value::Object(o) => match o.get("command").and_then(Value::as_str) {
                    Some(c) => c.to_string(),
                    None => {
                        out.push(format!("{section} {key}: object without a command"));
                        continue;
                    }
                },
                _ => {
                    out.push(format!("{section} {key}: not a string or object"));
                    continue;
                }
            };
            let Some(cmds) = commands else { continue };
            let known = name == "send"
                || name == "terminal"
                || cmds.contains(&name)
                || name.split_once('.').is_some_and(|(m, _)| mods.contains(m));
            if !known {
                out.push(format!("{section} {key}: unknown command {name}"));
            }
        }
    }
    out
}

/// Problems with a theme file: `background`, `foreground`, `selection`,
/// optional `cursor`, 16 `ansi` and optional `colors`, all `#rrggbb`.
pub fn validate_theme(name: &str, text: &str) -> Vec<String> {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return vec![format!("{name}: not JSON: {e}")],
    };
    let hex = |v: &Value| {
        v.as_str().is_some_and(|s| {
            s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
        })
    };
    let mut out = Vec::new();
    for k in ["background", "foreground", "selection"] {
        if !hex(&v[k]) {
            out.push(format!("{name}: {k} is not #rrggbb"));
        }
    }
    if !v["cursor"].is_null() && !hex(&v["cursor"]) {
        out.push(format!("{name}: cursor is not #rrggbb"));
    }
    match v["ansi"].as_array() {
        Some(a) if a.len() == 16 && a.iter().all(hex) => {}
        _ => out.push(format!("{name}: ansi is not 16 #rrggbb colours")),
    }
    if let Some(c) = v.get("colors") {
        match c.as_object() {
            Some(m) => {
                for (k, c) in m {
                    if !hex(c) {
                        out.push(format!("{name}: colors.{k} is not #rrggbb"));
                    }
                }
            }
            None => out.push(format!("{name}: colors is not an object")),
        }
    }
    out
}

/// After writing `file` at `since`: the app's `<stem>.error` for it, if a
/// running app (its socket in the home) wrote one within `wait`.
pub fn app_error(file: &Path, since: SystemTime, wait: Duration) -> Option<String> {
    let dir = file.parent()?;
    if !dir.join("herdr-app.sock").exists() {
        return None;
    }
    let err = file.with_extension("error");
    let start = Instant::now();
    loop {
        let fresh = fs::metadata(&err)
            .and_then(|m| m.modified())
            .is_ok_and(|m| m >= since);
        if fresh {
            return fs::read_to_string(&err).ok().map(|t| t.trim().to_string());
        }
        if start.elapsed() >= wait {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds() -> BTreeSet<String> {
        ["tab.new", "pane.go"].map(String::from).into()
    }

    #[test]
    fn keys_report_shape_and_unknown_commands() {
        let mods = BTreeSet::from(["board".to_string()]);
        let ok = r#"{"global": {"ctrl-s c": "tab.new", "ctrl-s left": "pane.go:left",
            "ctrl-s u": "board.toggle", "ctrl-s r": {"command": "tab.new", "args": {}}},
            "terminal": {"ctrl-s ctrl-s": "send:\u0013", "ctrl-1": "terminal"}}"#;
        assert!(validate_keys(ok, Some(&cmds()), &mods).is_empty());
        let bad = r#"{"global": {"a": "nope.x", "b": 1, "c": {"args": {}}}, "x": []}"#;
        assert_eq!(
            validate_keys(bad, Some(&cmds()), &mods),
            [
                "global a: unknown command nope.x",
                "global b: not a string or object",
                "global c: object without a command",
                "x: not an object of keys"
            ]
        );
        // Without herdr-app only the shape is checked.
        assert_eq!(validate_keys(bad, None, &mods).len(), 3);
        assert!(validate_keys("[", None, &mods)[0].contains("not JSON"));
    }

    #[test]
    fn repo_sources_are_valid() {
        let keys = crate::sources::default("herdr-app/keys.json").unwrap();
        assert!(validate_keys(keys, None, &BTreeSet::new()).is_empty());
        let v: Value = serde_json::from_str(keys).unwrap();
        assert_eq!(v["terminal"]["ctrl-s ctrl-s"], "send:\u{13}");
        assert_eq!(v["global"]["ctrl-s left"], "pane.go:left");
        for (rel, text) in crate::sources::DEFAULTS
            .iter()
            .filter(|(r, _)| r.starts_with("herdr-app/themes/"))
        {
            assert_eq!(validate_theme(rel, text), Vec::<String>::new());
        }
        let bad = r##"{"background": "#fff", "foreground": "#000000", "selection": "#000000",
            "ansi": ["#000000"], "colors": {"done": "blue"}}"##;
        assert_eq!(
            validate_theme("t", bad),
            [
                "t: background is not #rrggbb",
                "t: ansi is not 16 #rrggbb colours",
                "t: colors.done is not #rrggbb"
            ]
        );
    }

    #[test]
    fn app_error_needs_a_running_app_and_a_fresh_file() {
        let d = tempfile::tempdir().unwrap();
        let keys = d.path().join("keys.json");
        let since = SystemTime::now() - Duration::from_secs(1);
        fs::write(
            d.path().join("keys.error"),
            "keys.json was not applied: x\n",
        )
        .unwrap();
        assert_eq!(app_error(&keys, since, Duration::ZERO), None, "no socket");
        fs::write(d.path().join("herdr-app.sock"), "").unwrap();
        assert_eq!(
            app_error(&keys, since, Duration::ZERO).as_deref(),
            Some("keys.json was not applied: x")
        );
        let later = SystemTime::now() + Duration::from_secs(60);
        assert_eq!(app_error(&keys, later, Duration::ZERO), None, "stale");
    }
}
