//! herdr-app's home `~/.config/herdr-app/`, owned by vega (vega-0h4c.1,
//! docs/design/config.md "herdr-app").
//!
//! Keys, seeded settings, themes, the checked whole vega mod folder, and a
//! maintained clone of main. Layout selection uses the app's supported command;
//! shared.json remains app-owned. See docs/design/config.md for activation and
//! clone maintenance.

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

/// The vega mod owns its entire folder; local overrides resolve per shipped file.
const MOD_FILES: [&str; 2] = ["main.js", "mod.json"];

fn mod_files(name: &str) -> &[&str] {
    if name == "vega" {
        &["main.js", "mod.json", "lock.svg"]
    } else if name == "vega-sidebar" {
        &["main.js", "mod.json", "pin.svg"]
    } else if name == "vega-window-tabs" {
        &[
            "main.js",
            "mod.json",
            "navigation.js",
            "tabs.js",
            "diff.js",
            "UPSTREAM.md",
            "changes.svg",
            "win-min.svg",
            "win-max.svg",
            "win-close.svg",
            "win-restore.svg",
        ]
    } else if name == "vega-tabs" {
        &[
            "main.js",
            "mod.json",
            "UPSTREAM.md",
            "win-min.svg",
            "win-max.svg",
            "win-close.svg",
            "win-restore.svg",
        ]
    } else if name == "vega-pane" {
        &[
            "main.js",
            "mod.json",
            "UPSTREAM.md",
            "close.svg",
            "split-down.svg",
            "split-right.svg",
            "zoom.svg",
            "unzoom.svg",
        ]
    } else {
        &MOD_FILES
    }
}
/// Editor helpers herdr-app (re)writes into every mod folder on start. The app owns
/// them: they never count as drift and survive a folder replacement.
const APP_FILES: [&str; 2] = ["gpui-kit.d.ts", "jsconfig.json"];

fn mod_name(unit: crate::Unit) -> Option<&'static str> {
    match unit {
        crate::Unit::HerdrAppMods => Some("vega"),
        crate::Unit::HerdrAppSidebar => Some("vega-sidebar"),
        crate::Unit::HerdrAppInbox => Some("vega-inbox"),
        crate::Unit::HerdrAppWindowTabs => Some("vega-window-tabs"),
        crate::Unit::HerdrAppTabs => Some("vega-tabs"),
        crate::Unit::HerdrAppPane => Some("vega-pane"),
        _ => None,
    }
}

fn selection_marker(env: &Env) -> PathBuf {
    env.state.join("vega/herdr-app-layout-selected")
}

pub fn plan(ctx: &crate::Ctx, unit: crate::Unit) -> Result<crate::Plan, String> {
    use crate::{sources, whole, Plan};
    if let Some(p) = crate::need_herdr_app(&ctx.env) {
        return Ok(p);
    }
    let mut p = Plan::default();
    if let Some(mod_name) = mod_name(unit) {
        let dir = home(&ctx.env).join("mods").join(mod_name);
        let files = mod_files(mod_name);
        for name in files {
            let src = sources::get(&ctx.env, &format!("herdr-app/mods/{mod_name}/{name}"))?;
            p.files.push(whole(dir.join(name), src.text.clone())?);
            p.sources.push(src);
        }
        p.herdr_app_pending = fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink())
            || fs::read_dir(&dir).is_ok_and(|entries| {
                entries
                    .flatten()
                    .any(|e| !files.iter().chain(&APP_FILES).any(|n| e.file_name() == *n))
            });
        if p.herdr_app_pending {
            p.notes.push(
                format!("replace the whole mods/{mod_name} folder; retain the old folder in .vega-mod.{mod_name}-bak"),
            );
        }
        p.whole_dir = Some(dir);
    } else {
        let src = sources::get(&ctx.env, "herdr-app/layouts/vega.json")?;
        p.files.push(whole(
            home(&ctx.env).join("layouts/vega.json"),
            src.text.clone(),
        )?);
        p.notes.push(layout_version_note(
            &ctx.herdr_app,
            &src.text,
            &home(&ctx.env),
        ));
        p.sources.push(src);
        p.herdr_app_pending = !selection_marker(&ctx.env).exists() || p.files[0].live.is_none();
        if p.herdr_app_pending {
            p.notes.push(
                "first apply selects vega through layout.switch; start herdr-app before applying"
                    .into(),
            );
        }
    }
    Ok(p)
}

/// A clone needs a human maintenance pass when the installed app changes.
pub fn layout_version_note(bin: &OsStr, text: &str, app_home: &Path) -> String {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let base = v["cloned_from"]["version"].as_str().unwrap_or("unknown");
    let version = Command::new(bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().last().map(str::to_string));
    let installed: Value = fs::read(app_home.join("installed.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    let source_hash = v["vega_source"]["installed_hash"].as_str();
    let installed_hash = installed["layout:main"]["json"].as_str();
    if source_hash != installed_hash || source_hash.is_none() {
        return format!("layout drift: main hash {} -> {}, app {} -> {}; run python3 scripts/herdr-app-layout.py --write, review and commit the generated source (no automatic rebase)",
            source_hash.unwrap_or("unknown"), installed_hash.unwrap_or("unknown"), base, version.as_deref().unwrap_or("unknown"));
    }
    match version {
        Some(v) if v == base => format!("main clone: app {base}, installed {v}"),
        Some(v) => format!("layout drift: main clone from app {base}, installed {v}; review config/herdr-app/layouts/vega.json against shipped main (no automatic rebase)"),
        None => format!("main clone: app {base}; installed version unavailable"),
    }
}

fn app_command(ctx: &crate::Ctx, args: &[&str]) -> Result<Value, String> {
    let out = Command::new(&ctx.herdr_app)
        .args(args)
        .env("HERDR_APP_HOME", home(&ctx.env))
        .output()
        .map_err(|e| {
            format!(
                "herdr-app {}: {e}; start herdr-app and retry",
                args.join(" ")
            )
        })?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        format!(
            "herdr-app {}: invalid reply; start herdr-app and retry",
            args.join(" ")
        )
    })?;
    if !out.status.success() || v.get("error").is_some() {
        return Err(format!("herdr-app {} failed: {v}", args.join(" ")));
    }
    Ok(v)
}

pub fn apply(
    ctx: &crate::Ctx,
    unit: crate::Unit,
    plan: &crate::Plan,
    evidence: &mut Vec<Value>,
) -> crate::Outcome {
    use crate::Outcome;
    if !plan.herdr_app_pending && plan.files.iter().all(|f| !f.changed()) {
        return Outcome::Unchanged;
    }
    if let Some(name) = mod_name(unit) {
        return apply_mod(ctx, plan, name, evidence);
    }
    let file = &plan.files[0];
    let v: Value = match serde_json::from_str(&file.text) {
        Ok(v) => v,
        Err(e) => return Outcome::Rejected(vec![format!("vega layout: {e}")]),
    };
    if !matches!(
        v["type"].as_str(),
        Some("row" | "column" | "stack" | "block")
    ) {
        return Outcome::Rejected(vec!["vega layout: missing or invalid root type".into()]);
    }
    // There is no standalone layout checker in herdr-app 0.1.14. The supported
    // switch command validates activation, and owns persistence in shared.json.
    let mut previous_layout = None;
    if plan.herdr_app_pending {
        for name in [
            "vega",
            "vega-sidebar",
            "vega-inbox",
            "vega-tabs",
            "vega-pane",
        ] {
            if !home(&ctx.env)
                .join("mods")
                .join(name)
                .join("main.js")
                .is_file()
            {
                return Outcome::Rejected(vec![format!(
                    "apply herdr-app mod units before herdr-app.layouts (missing {name})"
                )]);
            }
        }
        match app_command(ctx, &["status"]) {
            Ok(v) => previous_layout = v["layout"].as_str().map(str::to_string),
            Err(e) => return Outcome::Rejected(vec![e]),
        }
    } else if home(&ctx.env).join("herdr-app.sock").exists() {
        previous_layout = app_command(ctx, &["status"])
            .ok()
            .and_then(|v| v["layout"].as_str().map(str::to_string));
    }
    if file.changed() {
        if let Err(e) = crate::write(file) {
            return Outcome::Failed(e);
        }
        evidence.push(crate::evidence::change(ctx, unit, plan, file));
    }
    if plan.herdr_app_pending || previous_layout.as_deref() == Some("vega") {
        // A fresh folder is visible on disk before the app's mod watcher sees it.
        // Retry only that activation error, bounded to five seconds; other errors
        // retain the normal rollback path and are never hidden by a retry.
        let deadline = Instant::now() + Duration::from_secs(5);
        let switched = loop {
            let result = app_command(ctx, &["do", "layout.switch", "--name", "vega"])
                .and_then(|_| app_command(ctx, &["status"]))
                .and_then(|v| {
                    if v["layout"] == "vega" && v["errors"].as_array().is_some_and(|a| a.is_empty())
                    {
                        Ok(())
                    } else {
                        Err(format!("vega layout was not accepted: {v}"))
                    }
                });
            match &result {
                Err(e) if e.contains("no mod folder") && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                _ => break result,
            }
        };
        if let Err(e) = switched {
            let restored = match &file.live {
                Some(text) => fs::write(&file.path, text),
                None => fs::remove_file(&file.path),
            };
            let mut errors = vec![e];
            if let Err(e) = restored {
                errors.push(format!("restoring layout: {e}"));
            }
            if let Some(name) = previous_layout.as_deref() {
                if let Err(e) = app_command(ctx, &["do", "layout.switch", "--name", name]) {
                    errors.push(format!("restoring selected layout: {e}"));
                }
            }
            return Outcome::Rejected(errors);
        }
        evidence.push(
            serde_json::json!({"target": home(&ctx.env).join("shared.json"),
            "operation": "layout.switch", "keys": []}),
        );
        let marker = selection_marker(&ctx.env);
        if let Err(e) =
            fs::create_dir_all(marker.parent().unwrap()).and_then(|_| fs::write(&marker, "vega\n"))
        {
            return Outcome::Failed(format!("recording layout selection: {e}"));
        }
    }
    Outcome::Written(vec![file.path.display().to_string()])
}

fn apply_mod(
    ctx: &crate::Ctx,
    plan: &crate::Plan,
    name: &str,
    evidence: &mut Vec<Value>,
) -> crate::Outcome {
    use crate::Outcome;
    let manifest = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("mod.json"))
        .unwrap();
    if !serde_json::from_str::<Value>(&manifest.text).is_ok_and(|v| v.is_object()) {
        return Outcome::Rejected(vec!["vega mod.json: not a JSON object".into()]);
    }
    let app_home = home(&ctx.env);
    let staged = match tempfile::Builder::new()
        .prefix(".vega-mod-stage-")
        .tempdir_in(&app_home)
    {
        Ok(d) => d,
        Err(e) => return Outcome::Failed(format!("stage vega mod: {e}")),
    };
    let folder = staged.path().join(name);
    let prepare = || -> std::io::Result<()> {
        fs::create_dir(&folder)?;
        for f in &plan.files {
            fs::write(folder.join(f.path.file_name().unwrap()), &f.text)?;
        }
        Ok(())
    };
    if let Err(e) = prepare() {
        return Outcome::Failed(format!("stage vega mod: {e}"));
    }
    // check only loads/renders eager elements; it does not run spawned exec work.
    let checked = Command::new(&ctx.herdr_app)
        .arg("check")
        .arg(&folder)
        .env("HERDR_APP_HOME", staged.path())
        .output();
    match checked {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            return Outcome::Rejected(vec![format!(
                "herdr-app check: {}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )])
        }
        Err(e) => {
            return Outcome::Rejected(vec![format!(
                "herdr-app check required before writing mods/vega: {e}"
            )])
        }
    }
    let target = app_home.join("mods").join(name);
    let backup = app_home.join(format!(".vega-mod.{name}-bak"));
    let result = || -> std::io::Result<()> {
        // The checker generates editor helpers alongside main.js (with the staging
        // folder as its home). Drop those and carry over the app's own copies.
        for name in APP_FILES {
            match fs::remove_file(folder.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let live = target.join(name);
            if fs::symlink_metadata(&live).is_ok_and(|m| m.is_file()) {
                fs::copy(&live, folder.join(name))?;
            }
        }
        fs::create_dir_all(target.parent().unwrap())?;
        let exists = fs::symlink_metadata(&target).is_ok();
        if exists {
            if let Ok(m) = fs::symlink_metadata(&backup) {
                if m.is_dir() && !m.file_type().is_symlink() {
                    fs::remove_dir_all(&backup)?;
                } else {
                    fs::remove_file(&backup)?;
                }
            }
            fs::rename(&target, &backup)?;
        }
        if let Err(e) = fs::rename(&folder, &target) {
            if exists {
                fs::rename(&backup, &target)?;
            }
            return Err(e);
        }
        Ok(())
    };
    if let Err(e) = result() {
        return Outcome::Failed(format!("replace vega mod: {e}"));
    }
    // Folder-level evidence includes removed stale files, without exposing content.
    evidence.push(
        serde_json::json!({"target": target, "operation": "replaced_folder",
        "backup_refreshed": if backup.exists() { Some(&backup) } else { None }, "keys": []}),
    );
    Outcome::Written(vec![target.display().to_string()])
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
