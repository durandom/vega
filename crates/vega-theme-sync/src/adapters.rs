//! One adapter per tool, ported from dotfiles `bin/theme-sync` (claude,
//! delta, pi, openusage, gh-dash) plus codex and herdr. Each reads and
//! writes only its own key and writes only on a change, so `apply` is
//! idempotent. A missing config file means the tool is not set up on this
//! host: skipped, never created.
//!
//! Running Claude panes switch only when vegad types `/config theme=<mode>`
//! into them (plugins/post/src/theme.rs); the file here is for new sessions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use toml_edit::{value, DocumentMut};

use crate::prefs::Prefs;
use crate::{files, Env, Mode};

pub const TOOLS: [&str; 7] = [
    "claude",
    "codex",
    "pi",
    "delta",
    "openusage",
    "gh-dash",
    "herdr",
];

/// What a tool's setting means now.
#[derive(Debug, Clone, PartialEq)]
pub enum Current {
    Mode(Mode),
    /// The tool follows the terminal by itself; nothing to do.
    Native,
    /// A value that maps to neither mode.
    Unknown,
    /// Not set up here (no config file, helper missing): the reason.
    Absent(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub tool: &'static str,
    /// The raw setting, e.g. `Bluloco Dark`; empty when unset.
    pub value: String,
    pub current: Current,
    /// Who owns the file when vega must not write it.
    pub read_only: Option<&'static str>,
}

impl Status {
    /// The tool shows something else than `due` and vega could fix it.
    pub fn drift(&self, due: Mode) -> bool {
        match self.current {
            Current::Mode(m) => m != due,
            Current::Unknown => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Unchanged,
    Set {
        from: String,
        to: String,
    },
    Native,
    Skipped(String),
    /// Drift vega leaves to the file's owner: the fix.
    ReadOnly(String),
    Failed(String),
}

impl Outcome {
    pub fn label(&self) -> String {
        match self {
            Outcome::Unchanged => "unchanged".into(),
            Outcome::Set { from, to } => {
                format!(
                    "set {} -> {to}",
                    if from.is_empty() { "(unset)" } else { from }
                )
            }
            Outcome::Native => "native auto".into(),
            Outcome::Skipped(r) => format!("skipped: {r}"),
            Outcome::ReadOnly(r) => format!("read-only: {r}"),
            Outcome::Failed(e) => format!("failed: {e}"),
        }
    }
}

/// Every adapter's status.
pub fn status_all(env: &Env, prefs: &Prefs) -> Vec<Status> {
    TOOLS.iter().map(|t| status(env, prefs, t)).collect()
}

/// Applies `mode` to every enabled tool.
pub fn apply_all(env: &Env, prefs: &Prefs, mode: Mode) -> Vec<(&'static str, Outcome)> {
    TOOLS
        .iter()
        .map(|t| {
            let out = if prefs.enabled(t) {
                apply(env, prefs, t, mode)
            } else {
                Outcome::Skipped("disabled in theme.toml".into())
            };
            (*t, out)
        })
        .collect()
}

pub fn status(env: &Env, prefs: &Prefs, tool: &'static str) -> Status {
    let (value, current) = match tool {
        "claude" => claude_get(env),
        "codex" => codex_get(env, prefs),
        "pi" => pi_get(env, prefs),
        "delta" => delta_get(env, prefs),
        "openusage" => json_get(&openusage_path(env), |v| {
            by_name(v, &names(prefs, "openusage"))
        }),
        "gh-dash" => ghdash_get(env, prefs),
        "herdr" => herdr_get(env),
        _ => (String::new(), Current::Error("unknown tool".into())),
    };
    Status {
        tool,
        value,
        current,
        read_only: (tool == "herdr").then_some("vega config"),
    }
}

pub fn apply(env: &Env, prefs: &Prefs, tool: &'static str, mode: Mode) -> Outcome {
    let st = status(env, prefs, tool);
    match &st.current {
        Current::Native => return Outcome::Native,
        Current::Absent(r) => return Outcome::Skipped(r.clone()),
        Current::Error(e) => return Outcome::Failed(e.clone()),
        Current::Mode(m) if *m == mode => return Outcome::Unchanged,
        _ => {}
    }
    if st.read_only.is_some() {
        return Outcome::ReadOnly(
            "vega renders ~/.config/herdr/config.toml: set [theme] auto_switch = true in herdr/config.toml (vega config init herdr/config.toml), then vega config apply herdr"
                .into(),
        );
    }
    let to = name(prefs, tool, mode);
    let res = match tool {
        "claude" => files::set_json_key(&claude_path(env), "theme", &to),
        "openusage" => files::set_json_key(&openusage_path(env), "theme", &to),
        "pi" => files::set_json_key(&pi_path(env), "theme", &to),
        "codex" => codex_set(env, &to),
        "delta" => delta_set(env, prefs, &st.value, &to),
        "gh-dash" => ghdash_set(env, &to),
        _ => Err("unknown tool".into()),
    };
    match res {
        Ok(_) => Outcome::Set { from: st.value, to },
        Err(e) => Outcome::Failed(e),
    }
}

/// Built-in names per tool; theme.toml `[tools.<tool>]` overrides them.
fn default_name(tool: &str, mode: Mode) -> &'static str {
    match (tool, mode) {
        ("codex", Mode::Light) => "catppuccin-latte",
        ("codex", Mode::Dark) => "catppuccin-mocha",
        ("delta", Mode::Light) => "mirthful-willow",
        ("delta", Mode::Dark) => "weeping-willow",
        ("openusage", Mode::Light) => "Bluloco Light",
        ("openusage", Mode::Dark) => "Bluloco Dark",
        ("gh-dash", Mode::Light) => "theme-light.yml",
        ("gh-dash", Mode::Dark) => "theme-dark.yml",
        (_, m) => m.as_str(),
    }
}

pub fn name(prefs: &Prefs, tool: &str, mode: Mode) -> String {
    prefs
        .name(tool, mode)
        .unwrap_or_else(|| default_name(tool, mode))
        .to_string()
}

/// (light name, dark name).
fn names(prefs: &Prefs, tool: &str) -> (String, String) {
    (
        name(prefs, tool, Mode::Light),
        name(prefs, tool, Mode::Dark),
    )
}

fn by_name(v: &str, (light, dark): &(String, String)) -> Current {
    if v == light {
        Current::Mode(Mode::Light)
    } else if v == dark {
        Current::Mode(Mode::Dark)
    } else {
        Current::Unknown
    }
}

fn no_config(p: &Path) -> Current {
    Current::Absent(format!("no {}", p.display()))
}

/// A JSON tool's `.theme`, classified.
fn json_get(path: &Path, classify: impl Fn(&str) -> Current) -> (String, Current) {
    match files::read_json(path) {
        Ok(None) => (String::new(), no_config(path)),
        Err(e) => (String::new(), Current::Error(e)),
        Ok(Some(v)) => {
            let s = v["theme"].as_str().unwrap_or("").to_string();
            let c = classify(&s);
            (s, c)
        }
    }
}

// --- claude: ~/.claude.json .theme (no key = dark) -------------------------

fn claude_path(env: &Env) -> PathBuf {
    env.home.join(".claude.json")
}

fn claude_get(env: &Env) -> (String, Current) {
    // Daltonized and ANSI variants count as their mode and are kept.
    json_get(&claude_path(env), |v| match v {
        "" => Current::Mode(Mode::Dark),
        v if v.starts_with("light") => Current::Mode(Mode::Light),
        v if v.starts_with("dark") => Current::Mode(Mode::Dark),
        _ => Current::Unknown,
    })
}

// --- pi: ~/.pi/agent/settings.json .theme ------------------------------------

fn pi_path(env: &Env) -> PathBuf {
    env.home.join(".pi/agent/settings.json")
}

/// pi follows the terminal by itself with `system` (its default) or a
/// `light/dark` pair; only a single fixed theme needs switching.
fn pi_get(env: &Env, prefs: &Prefs) -> (String, Current) {
    let n = names(prefs, "pi");
    json_get(&pi_path(env), |v| {
        if v.is_empty() || v == "system" || v.contains('/') {
            Current::Native
        } else {
            by_name(v, &n)
        }
    })
}

// --- openusage: ~/.config/openusage/settings.json .theme ---------------------

fn openusage_path(env: &Env) -> PathBuf {
    env.config.join("openusage/settings.json")
}

// --- codex: $CODEX_HOME/config.toml [tui] theme (syntax highlighting) -------

fn codex_path(env: &Env) -> PathBuf {
    env.home.join(".codex/config.toml")
}

fn codex_doc(env: &Env) -> Result<Option<DocumentMut>, String> {
    let p = codex_path(env);
    match fs::read_to_string(&p) {
        Ok(t) => t
            .parse()
            .map(Some)
            .map_err(|e| format!("{}: not TOML: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

fn codex_get(env: &Env, prefs: &Prefs) -> (String, Current) {
    match codex_doc(env) {
        Ok(None) => (String::new(), no_config(&codex_path(env))),
        Err(e) => (String::new(), Current::Error(e)),
        Ok(Some(doc)) => {
            let v = doc
                .get("tui")
                .and_then(|t| t.get("theme"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            let c = by_name(&v, &names(prefs, "codex"));
            (v, c)
        }
    }
}

fn codex_set(env: &Env, to: &str) -> Result<bool, String> {
    let mut doc = codex_doc(env)?.ok_or("codex config vanished")?;
    if !doc.contains_key("tui") {
        doc["tui"] = toml_edit::table();
    }
    doc["tui"]["theme"] = value(to);
    files::write_atomic(&codex_path(env), &doc.to_string())?;
    Ok(true)
}

// --- delta: ~/.gitconfig.local delta.features --------------------------------

fn delta_path(env: &Env) -> PathBuf {
    env.home.join(".gitconfig.local")
}

fn git_config(file: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("config")
        .arg("--file")
        .arg(file)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    // `--get` exits 1 for a missing key.
    if out.status.success() || out.status.code() == Some(1) {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "git config: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// `delta.features` may list several features; the theme is the one that is
/// a known light or dark name.
fn delta_get(env: &Env, prefs: &Prefs) -> (String, Current) {
    let p = delta_path(env);
    if !p.exists() {
        return (String::new(), no_config(&p));
    }
    match git_config(&p, &["--get", "delta.features"]) {
        Err(e) => (String::new(), Current::Error(e)),
        Ok(v) => {
            let n = names(prefs, "delta");
            let c = v
                .split_whitespace()
                .map(|f| by_name(f, &n))
                .find(|c| *c != Current::Unknown)
                .unwrap_or(Current::Unknown);
            (v, c)
        }
    }
}

/// Swaps the theme feature, keeps the others; appends when none is set.
pub fn delta_features(current: &str, light: &str, dark: &str, to: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut swapped = false;
    for f in current.split_whitespace() {
        if f == light || f == dark {
            if !swapped {
                out.push(to);
                swapped = true;
            }
        } else {
            out.push(f);
        }
    }
    if !swapped {
        out.push(to);
    }
    out.join(" ")
}

fn delta_set(env: &Env, prefs: &Prefs, current: &str, to: &str) -> Result<bool, String> {
    let (light, dark) = names(prefs, "delta");
    let v = delta_features(current, &light, &dark, to);
    git_config(&delta_path(env), &["delta.features", &v])?;
    Ok(true)
}

// --- gh-dash: ~/.config/gh-dash/config.yml theme.colors from a palette ------

fn ghdash_dir(env: &Env) -> PathBuf {
    env.config.join("gh-dash")
}

fn yq(args: &[&str]) -> Result<String, String> {
    let out = Command::new("yq")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("yq: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "yq: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

pub fn has_yq() -> bool {
    Command::new("yq")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// gh-dash bakes hex colours into its config, so the palette is swapped
/// physically; the active one is recognized by `text.primary`.
fn ghdash_get(env: &Env, prefs: &Prefs) -> (String, Current) {
    let dir = ghdash_dir(env);
    let config = dir.join("config.yml");
    if !config.exists() {
        return (String::new(), no_config(&config));
    }
    if !has_yq() {
        return (String::new(), Current::Absent("yq not installed".into()));
    }
    let primary =
        |file: &Path, path: &str| yq(&[&format!("{path} // \"\""), &file.to_string_lossy()]);
    let v = match primary(&config, ".theme.colors.text.primary") {
        Ok(v) => v,
        Err(e) => return (String::new(), Current::Error(e)),
    };
    let palette = |m| primary(&dir.join(name(prefs, "gh-dash", m)), ".text.primary").ok();
    let c = if v.is_empty() {
        Current::Unknown
    } else if palette(Mode::Light).as_deref() == Some(&v) {
        Current::Mode(Mode::Light)
    } else if palette(Mode::Dark).as_deref() == Some(&v) {
        Current::Mode(Mode::Dark)
    } else {
        Current::Unknown
    };
    (v, c)
}

fn ghdash_set(env: &Env, to: &str) -> Result<bool, String> {
    let dir = ghdash_dir(env);
    let palette = dir.join(to);
    if !palette.exists() {
        return Err(format!("no palette {}", palette.display()));
    }
    let expr = format!(".theme.colors = load(\"{}\")", palette.display());
    yq(&["-i", &expr, &dir.join("config.yml").to_string_lossy()])?;
    Ok(true)
}

// --- herdr: ~/.config/herdr/config.toml [theme] (rendered by vega config, read only here) --

fn herdr_get(env: &Env) -> (String, Current) {
    let p = env.config.join("herdr/config.toml");
    let doc: DocumentMut = match fs::read_to_string(&p) {
        Err(_) => return (String::new(), no_config(&p)),
        Ok(t) => match t.parse() {
            Ok(d) => d,
            Err(e) => return (String::new(), Current::Error(format!("not TOML: {e}"))),
        },
    };
    let s = |k: &str| {
        doc.get("theme")
            .and_then(|t| t.get(k))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let auto = doc
        .get("theme")
        .and_then(|t| t.get("auto_switch"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let name = s("name");
    if auto {
        return (
            format!("auto_switch ({}/{})", s("light_name"), s("dark_name")),
            Current::Native,
        );
    }
    let c = by_name(&name, &(s("light_name"), s("dark_name")));
    // A Catppuccin flavour outside the pair still says which mode it is.
    let c = match (&c, name.as_str()) {
        (Current::Unknown, n) if n.ends_with("latte") || n.ends_with("-day") => {
            Current::Mode(Mode::Light)
        }
        (Current::Unknown, n) if n.ends_with("mocha") || n.ends_with("macchiato") => {
            Current::Mode(Mode::Dark)
        }
        _ => c,
    };
    (name, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp HOME with a copy of every config format the adapters handle.
    fn home() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::at(dir.path());
        let w = |rel: &str, text: &str| {
            let p = dir.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        };
        w(
            ".claude.json",
            "{\"numStartups\": 3, \"theme\": \"light\", \"projects\": {}}\n",
        );
        w(
            ".pi/agent/settings.json",
            "{\"defaultModel\": \"x\", \"theme\": \"light\"}\n",
        );
        w(
            ".codex/config.toml",
            "model = \"gpt\"\n\n[tui]\nstatus_line = [\"a\"]\ntheme = \"catppuccin-latte\" # syntax\n",
        );
        w(
            ".gitconfig.local",
            "[delta]\n\tfeatures = side-by-side mirthful-willow\n[user]\n\tname = x\n",
        );
        w(
            ".config/openusage/settings.json",
            "{\"theme\": \"Bluloco Light\", \"hooks\": []}\n",
        );
        w(
            ".config/gh-dash/config.yml",
            "# mine\nprSections: []\ntheme:\n  colors:\n    text:\n      primary: \"#373a41\"\n",
        );
        w(
            ".config/gh-dash/theme-light.yml",
            "text:\n  primary: \"#373a41\"\n",
        );
        w(
            ".config/gh-dash/theme-dark.yml",
            "text:\n  primary: \"#b9c0cb\"\n",
        );
        w(
            ".config/herdr/config.toml",
            "[theme]\nname = \"catppuccin-latte\"\nauto_switch = false\nlight_name = \"tokyo-night-day\"\ndark_name = \"tokyo-night\"\n",
        );
        (dir, env)
    }

    fn modes(env: &Env, prefs: &Prefs) -> Vec<(&'static str, Current)> {
        status_all(env, prefs)
            .into_iter()
            .map(|s| (s.tool, s.current))
            .collect()
    }

    #[test]
    fn reads_every_format() {
        let (_d, env) = home();
        let p = Prefs::default();
        let yq = has_yq();
        for (tool, c) in modes(&env, &p) {
            match tool {
                "gh-dash" if !yq => assert!(matches!(c, Current::Absent(_))),
                _ => assert_eq!(c, Current::Mode(Mode::Light), "{tool}"),
            }
        }
    }

    #[test]
    fn apply_dark_then_again_is_idempotent_and_keeps_other_keys() {
        let (d, env) = home();
        let p = Prefs::default();
        let first = apply_all(&env, &p, Mode::Dark);
        for (tool, out) in &first {
            match *tool {
                "herdr" => assert!(matches!(out, Outcome::ReadOnly(_)), "{out:?}"),
                "gh-dash" if !has_yq() => assert!(matches!(out, Outcome::Skipped(_))),
                _ => assert!(matches!(out, Outcome::Set { .. }), "{tool}: {out:?}"),
            }
        }
        let second = apply_all(&env, &p, Mode::Dark);
        for (tool, out) in &second {
            assert!(
                matches!(
                    out,
                    Outcome::Unchanged | Outcome::ReadOnly(_) | Outcome::Skipped(_)
                ),
                "{tool}: {out:?}"
            );
        }
        let read = |rel: &str| fs::read_to_string(d.path().join(rel)).unwrap();
        assert!(read(".claude.json").contains("\"numStartups\": 3"));
        assert!(read(".claude.json").contains("\"theme\": \"dark\""));
        assert!(read(".pi/agent/settings.json").contains("\"defaultModel\""));
        let codex = read(".codex/config.toml");
        assert!(codex.contains("model = \"gpt\""));
        assert!(codex.contains("status_line = [\"a\"]"));
        assert!(codex.contains("theme = \"catppuccin-mocha\""));
        let delta = read(".gitconfig.local");
        assert!(delta.contains("side-by-side weeping-willow"), "{delta}");
        assert!(delta.contains("name = x"));
        assert!(read(".config/openusage/settings.json").contains("Bluloco Dark"));
        if has_yq() {
            let gh = read(".config/gh-dash/config.yml");
            assert!(gh.contains("#b9c0cb"), "{gh}");
            assert!(gh.contains("prSections"));
        }
        // herdr is never written.
        assert!(read(".config/herdr/config.toml").contains("auto_switch = false"));
        for (tool, c) in modes(&env, &p) {
            if !matches!(tool, "herdr" | "gh-dash") {
                assert_eq!(c, Current::Mode(Mode::Dark), "{tool}");
            }
        }
    }

    #[test]
    fn native_missing_disabled_and_custom_names() {
        let (d, env) = home();
        fs::write(
            d.path().join(".pi/agent/settings.json"),
            "{\"theme\": \"latte/mocha\"}",
        )
        .unwrap();
        fs::remove_file(d.path().join(".config/openusage/settings.json")).unwrap();
        fs::write(
            d.path().join(".config/herdr/config.toml"),
            "[theme]\nauto_switch = true\nlight_name = \"a\"\ndark_name = \"b\"\n",
        )
        .unwrap();
        let p = crate::prefs::parse(
            "[tools.codex]\ndark = \"gruvbox\"\n[tools.delta]\nenabled = false\n",
        )
        .unwrap();
        let out: std::collections::HashMap<_, _> =
            apply_all(&env, &p, Mode::Dark).into_iter().collect();
        assert_eq!(out["pi"], Outcome::Native);
        assert_eq!(out["herdr"], Outcome::Native);
        assert!(matches!(out["openusage"], Outcome::Skipped(_)));
        assert!(matches!(out["delta"], Outcome::Skipped(_)));
        assert!(fs::read_to_string(d.path().join(".gitconfig.local"))
            .unwrap()
            .contains("mirthful-willow"));
        assert!(fs::read_to_string(d.path().join(".codex/config.toml"))
            .unwrap()
            .contains("theme = \"gruvbox\""));
        // Nothing set up at all: every tool skips, nothing is created.
        let empty = tempfile::tempdir().unwrap();
        let env = Env::at(empty.path());
        for (tool, o) in apply_all(&env, &Prefs::default(), Mode::Light) {
            assert!(matches!(o, Outcome::Skipped(_)), "{tool}: {o:?}");
        }
        assert_eq!(fs::read_dir(empty.path()).unwrap().count(), 0);
    }

    #[test]
    fn claude_variants_and_unset() {
        let (d, env) = home();
        let p = Prefs::default();
        let set = |t: &str| fs::write(d.path().join(".claude.json"), t).unwrap();
        set("{\"theme\": \"light-daltonized\"}");
        assert_eq!(apply(&env, &p, "claude", Mode::Light), Outcome::Unchanged);
        set("{}");
        assert_eq!(
            status(&env, &p, "claude").current,
            Current::Mode(Mode::Dark)
        );
        assert_eq!(
            apply(&env, &p, "claude", Mode::Light),
            Outcome::Set {
                from: String::new(),
                to: "light".into()
            }
        );
    }

    #[test]
    fn delta_feature_swap() {
        let f = |c, to| delta_features(c, "l", "d", to);
        assert_eq!(f("", "d"), "d");
        assert_eq!(f("side l", "d"), "side d");
        assert_eq!(f("l side", "d"), "d side");
        assert_eq!(f("side", "l"), "side l");
    }
}
