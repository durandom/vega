//! pi watches the active custom theme file, not settings.json. Keep its
//! name stable and replace its contents to switch already running sessions.
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{by_name, Current, Outcome};
use crate::{files, prefs, Env, Mode};

const AUTO: &str = "vega-auto";

fn settings(env: &Env) -> PathBuf {
    env.home.join(".pi/agent/settings.json")
}

fn output(env: &Env) -> PathBuf {
    env.home.join(".pi/agent/themes/vega-auto.json")
}

/// Resolve the actual executable's package, including npm/fnm symlinks.
/// Tests use Env::at and never consult the process PATH or real HOME.
pub(crate) fn installed_package() -> Option<PathBuf> {
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        let Ok(exe) = fs::canonicalize(dir.join("pi")) else {
            continue;
        };
        for parent in exe.ancestors().skip(1) {
            if files::read_json(&parent.join("package.json"))
                .ok()
                .flatten()
                .is_some_and(|v| v["name"] == "@earendil-works/pi-coding-agent")
            {
                return Some(parent.to_path_buf());
            }
        }
    }
    None
}

fn pair(raw: &str) -> Option<(&str, &str)> {
    let (light, dark) = raw.split_once('/')?;
    (!light.is_empty() && !dark.is_empty() && !dark.contains('/')).then_some((light, dark))
}

fn names(env: &Env, supplied: &prefs::Prefs, raw: &str) -> Result<(String, String), String> {
    // Reload the file: apply_all's Prefs may predate the first migration.
    let saved = prefs::load(env)?;
    let old = pair(raw);
    let one = |mode| {
        supplied
            .name("pi", mode)
            .or_else(|| saved.name("pi", mode))
            .or_else(|| old.map(|(l, d)| if mode == Mode::Light { l } else { d }))
            .unwrap_or(match mode {
                Mode::Light => "catppuccin-latte",
                Mode::Dark => "catppuccin-mocha",
            })
            .to_string()
    };
    Ok((one(Mode::Light), one(Mode::Dark)))
}

/// Search installed git/npm packages without following directory symlinks
/// (which can cycle). Deterministic order makes duplicate names predictable.
fn find_theme(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(dir).ok()?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if kind.is_dir() && depth > 0 {
            if let Some(p) = find_theme(&path, name, depth - 1) {
                return Some(p);
            }
        } else if path.extension().is_some_and(|e| e == "json")
            && path.file_name().is_none_or(|f| f != "vega-auto.json")
            && files::read_json(&path)
                .ok()
                .flatten()
                .is_some_and(|v| v["name"] == name && v["colors"].is_object())
        {
            return Some(path);
        }
    }
    None
}

fn source(env: &Env, name: &str) -> Result<Value, String> {
    if name.is_empty() || name.contains('/') || name == AUTO {
        return Err(format!("pi: invalid source theme {name:?}"));
    }
    let builtin = if matches!(name, "light" | "dark") {
        env.pi_package.as_ref().map(|p| {
            let tree = if p.join("src").is_dir() {
                "src"
            } else {
                "dist"
            };
            p.join(tree)
                .join("modes/interactive/theme")
                .join(format!("{name}.json"))
        })
    } else {
        None
    };
    let path = builtin.filter(|p| p.is_file())
        .or_else(|| find_theme(&env.home.join(".pi/agent/themes"), name, 0))
        .or_else(|| find_theme(&env.home.join(".pi/agent/git"), name, 10))
        .or_else(|| find_theme(&env.home.join(".pi/agent/npm/node_modules"), name, 10))
        .ok_or_else(|| format!("pi: source theme {name:?} not found (built-in, ~/.pi/agent/themes, installed git/npm packages); install it or set [tools.pi] light/dark in theme.toml"))?;
    let mut theme =
        files::read_json(&path)?.ok_or_else(|| format!("{}: missing", path.display()))?;
    if !theme["colors"].is_object() {
        return Err(format!("{}: theme has no colors object", path.display()));
    }
    theme["name"] = AUTO.into();
    Ok(theme)
}

pub(crate) fn status(env: &Env, prefs: &prefs::Prefs) -> (String, Current) {
    let raw = match files::read_json(&settings(env)) {
        Ok(Some(v)) => v["theme"].as_str().unwrap_or("").to_string(),
        Ok(None) => {
            return (
                String::new(),
                Current::Absent(format!("no {}", settings(env).display())),
            )
        }
        Err(e) => return (String::new(), Current::Error(e)),
    };
    if raw != AUTO {
        // Fixed built-in themes still have a known mode before migration.
        let current = by_name(&raw, &("light".into(), "dark".into()));
        return (raw, current);
    }
    let result = (|| {
        let n = names(env, prefs, &raw)?;
        let current = files::read_json(&output(env))?;
        let mut missing = None;
        for (name, mode) in [(&n.0, Mode::Light), (&n.1, Mode::Dark)] {
            match source(env, name) {
                Ok(theme) if current.as_ref() == Some(&theme) => return Ok(Current::Mode(mode)),
                Err(e) => missing = Some(e),
                _ => {}
            }
        }
        missing.map_or(Ok(Current::Unknown), Err)
    })();
    (raw, result.unwrap_or_else(Current::Error))
}

pub(crate) fn apply(env: &Env, prefs: &prefs::Prefs, mode: Mode) -> Outcome {
    let old = match files::read_json(&settings(env)) {
        Ok(Some(v)) => v["theme"].as_str().unwrap_or("").to_string(),
        Ok(None) => return Outcome::Skipped(format!("no {}", settings(env).display())),
        Err(e) => return Outcome::Failed(e),
    };
    let result = (|| {
        let (light, dark) = names(env, prefs, &old)?;
        let selected = if mode == Mode::Light { &light } else { &dark };
        // Validate before touching any file, including migration preferences.
        let theme = source(env, selected)?;
        if pair(&old).is_some() {
            prefs::save_pi_pair(env, &light, &dark)?;
        }
        let path = output(env);
        let mut text = serde_json::to_string_pretty(&theme).map_err(|e| e.to_string())?;
        text.push('\n');
        let changed = fs::read_to_string(&path).ok().as_deref() != Some(&text);
        if changed {
            fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            files::write_atomic(&path, &text)?;
        }
        let settings_changed = files::set_json_key(&settings(env), "theme", AUTO)?;
        Ok(if changed || settings_changed {
            Outcome::Set {
                from: old,
                to: format!("{AUTO} ({selected})"),
            }
        } else {
            Outcome::Unchanged
        })
    })();
    result.unwrap_or_else(Outcome::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn write(env: &Env, rel: &str, text: &str) {
        let path = env.home.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn theme(env: &Env, rel: &str, name: &str, color: &str) {
        write(env, rel, &serde_json::json!({"name":name, "vars":{"accent":color}, "colors":{"accent":"accent"}, "export":{"pageBg":color}}).to_string());
    }

    #[test]
    fn migrates_pair_switches_live_file_and_reapply_does_not_write() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::at(temp.path());
        write(
            &env,
            ".pi/agent/settings.json",
            r#"{"theme":"latte/mocha","model":"keep"}"#,
        );
        write(
            &env,
            ".config/vega/theme.toml",
            "# keep\nmode = \"auto\"\n[tools.pi]\nenabled = true # also keep\n",
        );
        theme(
            &env,
            ".pi/agent/git/github.com/hasit/pi-community-themes/themes/latte.json",
            "latte",
            "#aaa",
        );
        theme(
            &env,
            ".pi/agent/git/github.com/hasit/pi-community-themes/themes/mocha.json",
            "mocha",
            "#333",
        );
        let prefs = prefs::load(&env).unwrap();
        assert!(matches!(
            apply(&env, &prefs, Mode::Light),
            Outcome::Set { .. }
        ));
        let config = files::read_json(&settings(&env)).unwrap().unwrap();
        assert_eq!(config["theme"], AUTO);
        assert_eq!(config["model"], "keep");
        let saved = prefs::load(&env).unwrap();
        assert_eq!(saved.name("pi", Mode::Light), Some("latte"));
        assert_eq!(saved.name("pi", Mode::Dark), Some("mocha"));
        let toml = fs::read_to_string(prefs::path(&env)).unwrap();
        assert!(toml.contains("# keep") && toml.contains("# also keep"));
        let light = files::read_json(&output(&env)).unwrap().unwrap();
        assert_eq!(light["name"], AUTO);
        assert_eq!(light["vars"]["accent"], "#aaa");
        assert_eq!(status(&env, &prefs).1, Current::Mode(Mode::Light));
        // The caller may still hold the pre-migration preferences.
        assert!(matches!(
            apply(&env, &prefs, Mode::Dark),
            Outcome::Set { .. }
        ));
        let dark = files::read_json(&output(&env)).unwrap().unwrap();
        assert_eq!(dark["name"], AUTO);
        assert_eq!(dark["export"]["pageBg"], "#333");
        assert_eq!(status(&env, &saved).1, Current::Mode(Mode::Dark));
        let tracked = [output(&env), settings(&env), prefs::path(&env)];
        let before: Vec<_> = tracked
            .iter()
            .map(|p| {
                (
                    fs::metadata(p).unwrap().ino(),
                    fs::metadata(p).unwrap().modified().unwrap(),
                )
            })
            .collect();
        assert_eq!(apply(&env, &saved, Mode::Dark), Outcome::Unchanged);
        for (p, expected) in tracked.iter().zip(before) {
            let meta = fs::metadata(p).unwrap();
            assert_eq!((meta.ino(), meta.modified().unwrap()), expected);
        }
        fs::remove_file(
            env.home
                .join(".pi/agent/git/github.com/hasit/pi-community-themes/themes/latte.json"),
        )
        .unwrap();
        let error = apply(&env, &saved, Mode::Light);
        assert!(
            matches!(error, Outcome::Failed(e) if e.contains("latte") && e.contains("not found"))
        );
        assert_eq!(files::read_json(&output(&env)).unwrap().unwrap(), dark);
    }

    #[test]
    fn missing_source_leaves_migration_and_output_alone() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::at(temp.path());
        let original = r#"{"theme":"missing/also-missing","model":"keep"}"#;
        write(&env, ".pi/agent/settings.json", original);
        assert!(matches!(
            apply(&env, &prefs::Prefs::default(), Mode::Light),
            Outcome::Failed(_)
        ));
        assert_eq!(fs::read_to_string(settings(&env)).unwrap(), original);
        assert!(!output(&env).exists());
        assert!(!prefs::path(&env).exists());
    }

    #[test]
    fn builtin_custom_and_npm_sources_and_explicit_overrides() {
        let temp = tempfile::tempdir().unwrap();
        let mut env = Env::at(temp.path());
        env.pi_package = Some(env.home.join("pi-package"));
        theme(
            &env,
            "pi-package/dist/modes/interactive/theme/light.json",
            "light",
            "#fff",
        );
        theme(&env, ".pi/agent/themes/custom.json", "custom", "#111");
        theme(
            &env,
            ".pi/agent/npm/node_modules/@test/theme-pack/themes/npm.json",
            "npm",
            "#222",
        );
        write(
            &env,
            ".pi/agent/settings.json",
            r#"{"theme":"light/custom"}"#,
        );
        write(
            &env,
            ".config/vega/theme.toml",
            "[tools.pi]\ndark = \"npm\"\n",
        );
        let prefs = prefs::load(&env).unwrap();
        assert!(matches!(
            apply(&env, &prefs, Mode::Light),
            Outcome::Set { .. }
        ));
        let saved = prefs::load(&env).unwrap();
        assert_eq!(saved.name("pi", Mode::Dark), Some("npm"));
        assert!(matches!(
            apply(&env, &saved, Mode::Dark),
            Outcome::Set { .. }
        ));
        assert_eq!(
            files::read_json(&output(&env)).unwrap().unwrap()["vars"]["accent"],
            "#222"
        );
        assert_eq!(source(&env, "custom").unwrap()["vars"]["accent"], "#111");
    }

    #[test]
    fn system_uses_defaults_and_disabled_or_absent_skips() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::at(temp.path());
        assert!(matches!(
            apply(&env, &prefs::Prefs::default(), Mode::Dark),
            Outcome::Skipped(_)
        ));
        write(&env, ".pi/agent/settings.json", r#"{"theme":"system"}"#);
        theme(
            &env,
            ".pi/agent/themes/dark.json",
            "catppuccin-mocha",
            "#222",
        );
        let prefs = prefs::parse("[tools.pi]\nenabled = false\n").unwrap();
        let results = crate::adapters::apply_all(&env, &prefs, Mode::Dark);
        assert!(matches!(
            results.iter().find(|(t, _)| *t == "pi").unwrap().1,
            Outcome::Skipped(_)
        ));
        assert!(!output(&env).exists());
        assert!(matches!(
            apply(&env, &prefs::Prefs::default(), Mode::Dark),
            Outcome::Set { .. }
        ));
        assert!(!prefs::path(&env).exists());
    }
}
