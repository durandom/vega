//! `~/.config/vega/theme.toml` (`$XDG_CONFIG_HOME/vega/theme.toml`): what the
//! user wants. Every key is optional:
//!
//! ```toml
//! mode = "auto"          # auto (follow the OS) | dark | light
//! control = "proteus"    # the host headless hosts follow
//! headless = true        # default: no readable system appearance
//! live_claude = true     # vegad types /config theme=<mode> into idle claude panes
//!
//! [tools.codex]          # per tool: theme names, or off
//! light = "catppuccin-latte"
//! dark = "catppuccin-mocha"
//! enabled = false
//! ```
//!
//! The applied mode is state, not config (crate::state).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use toml_edit::{value, DocumentMut};

use crate::{files, Env, Mode, Pref};

pub const DEFAULT_CONTROL: &str = "proteus";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolPrefs {
    pub light: Option<String>,
    pub dark: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub mode: Pref,
    pub control: String,
    /// `None`: detect (no readable system appearance means headless).
    pub headless: Option<bool>,
    pub live_claude: bool,
    pub tools: BTreeMap<String, ToolPrefs>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            mode: Pref::Auto,
            control: DEFAULT_CONTROL.into(),
            headless: None,
            live_claude: true,
            tools: BTreeMap::new(),
        }
    }
}

impl Prefs {
    /// The tool's theme name for `mode`, if the user set one.
    pub fn name(&self, tool: &str, mode: Mode) -> Option<&str> {
        let t = self.tools.get(tool)?;
        match mode {
            Mode::Light => t.light.as_deref(),
            Mode::Dark => t.dark.as_deref(),
        }
    }

    pub fn enabled(&self, tool: &str) -> bool {
        self.tools.get(tool).and_then(|t| t.enabled).unwrap_or(true)
    }
}

pub fn path(env: &Env) -> PathBuf {
    env.config.join("vega").join("theme.toml")
}

/// The preferences; a missing file is all defaults. Unknown keys are
/// ignored, a bad value is an error naming it.
pub fn load(env: &Env) -> Result<Prefs, String> {
    let p = path(env);
    match fs::read_to_string(&p) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Prefs::default()),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

pub fn parse(text: &str) -> Result<Prefs, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("not TOML: {e}"))?;
    let mut p = Prefs::default();
    if let Some(m) = doc.get("mode") {
        let s = m.as_str().unwrap_or("");
        p.mode = Pref::parse(s)
            .ok_or_else(|| format!("mode = {s:?}: expected \"auto\", \"dark\" or \"light\""))?;
    }
    if let Some(c) = doc.get("control").and_then(|c| c.as_str()) {
        p.control = c.to_string();
    }
    p.headless = doc.get("headless").and_then(|h| h.as_bool());
    if let Some(l) = doc.get("live_claude").and_then(|l| l.as_bool()) {
        p.live_claude = l;
    }
    if let Some(tools) = doc.get("tools").and_then(|t| t.as_table_like()) {
        for (name, t) in tools.iter() {
            let Some(t) = t.as_table_like() else { continue };
            let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(String::from);
            p.tools.insert(
                name.to_string(),
                ToolPrefs {
                    light: s("light"),
                    dark: s("dark"),
                    enabled: t.get("enabled").and_then(|v| v.as_bool()),
                },
            );
        }
    }
    Ok(p)
}

/// Sets `mode` in theme.toml, keeping every other line; creates the file.
pub fn set_mode(env: &Env, pref: Pref) -> Result<(), String> {
    let p = path(env);
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", p.display())),
    };
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("{}: not TOML: {e}", p.display()))?;
    if doc.get("mode").and_then(|m| m.as_str()) == Some(pref.as_str()) {
        return Ok(());
    }
    doc["mode"] = value(pref.as_str());
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if !p.exists() {
        fs::write(&p, "").map_err(|e| format!("{}: {e}", p.display()))?;
    }
    files::write_atomic(&p, &doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_keys_and_defaults() {
        assert_eq!(parse("").unwrap(), Prefs::default());
        let p = parse(
            "mode = \"light\"\ncontrol = \"titan\"\nheadless = true\nlive_claude = false\n\
             [tools.codex]\nlight = \"a\"\ndark = \"b\"\n[tools.delta]\nenabled = false\n",
        )
        .unwrap();
        assert_eq!(p.mode, Pref::Fixed(Mode::Light));
        assert_eq!(p.control, "titan");
        assert_eq!(p.headless, Some(true));
        assert!(!p.live_claude);
        assert_eq!(p.name("codex", Mode::Dark), Some("b"));
        assert_eq!(p.name("pi", Mode::Dark), None);
        assert!(!p.enabled("delta"));
        assert!(p.enabled("codex"));
        assert!(parse("mode = \"system\"").unwrap_err().contains("mode"));
    }

    #[test]
    fn set_mode_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::at(dir.path());
        set_mode(&env, Pref::Fixed(Mode::Dark)).unwrap();
        assert_eq!(load(&env).unwrap().mode, Pref::Fixed(Mode::Dark));
        let text = "# mine\nmode = \"dark\" # keep\n\n[tools.codex]\nlight = \"x\"\n";
        fs::write(path(&env), text).unwrap();
        set_mode(&env, Pref::Auto).unwrap();
        let after = fs::read_to_string(path(&env)).unwrap();
        assert!(after.contains("# mine"));
        assert!(after.contains("light = \"x\""));
        assert_eq!(load(&env).unwrap().mode, Pref::Auto);
    }
}
