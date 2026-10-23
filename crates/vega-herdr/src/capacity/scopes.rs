//! Window scopes as data: `capacity/windows.toml`, a vega config source
//! (docs/design/config.md "Sources"). The repo default is embedded; a local
//! `$XDG_CONFIG_HOME/vega/config/capacity/windows.toml` replaces it. A window
//! not listed is account-wide.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

use super::{Sample, Scope};

/// The source path, relative to `config/` and the local config dir.
pub const SOURCE: &str = "capacity/windows.toml";

/// The repo default.
pub const DEFAULT: &str = include_str!("../../../../config/capacity/windows.toml");

/// (provider, window) → scope.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scopes(BTreeMap<(String, String), Scope>);

impl Scopes {
    /// `[<provider>] <window> = "account" | "model:<name>"`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
        let mut map = BTreeMap::new();
        for (provider, item) in doc.iter() {
            let table = item
                .as_table_like()
                .ok_or_else(|| format!("{provider} is no table of windows"))?;
            for (window, v) in table.iter() {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("{provider}.{window} is no string"))?;
                let scope = Scope::parse(s).map_err(|why| format!("{provider}.{window}: {why}"))?;
                map.insert((provider.to_string(), window.to_string()), scope);
            }
        }
        Ok(Scopes(map))
    }

    /// The local source when there is one, else the default. `Err` names
    /// the file and why it does not parse.
    pub fn load() -> Result<Self, String> {
        Self::load_at(local_path())
    }

    /// `load` with the local source at `local`.
    pub fn load_at(local: Option<PathBuf>) -> Result<Self, String> {
        match local.map(|p| (fs::read_to_string(&p), p)) {
            Some((Ok(text), p)) => Self::parse(&text).map_err(|e| format!("{}: {e}", p.display())),
            Some((Err(e), p)) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("{}: {e}", p.display()))
            }
            _ => Self::parse(DEFAULT).map_err(|e| format!("default {SOURCE}: {e}")),
        }
    }

    pub fn scope(&self, provider: &str, window: &str) -> Scope {
        self.0
            .get(&(provider.to_string(), window.to_string()))
            .cloned()
            .unwrap_or(Scope::Account)
    }

    /// Sets every reading's scope.
    pub fn apply(&self, sample: &mut Sample) {
        for r in &mut sample.readings {
            r.scope = self.scope(&r.provider, &r.window);
        }
    }
}

/// `$XDG_CONFIG_HOME/vega/config/capacity/windows.toml` (default `~/.config`).
pub fn local_path() -> Option<PathBuf> {
    let config = env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(config.join("vega/config").join(SOURCE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_lists_model_windows_and_the_rest_is_account() {
        let s = Scopes::parse(DEFAULT).unwrap();
        assert_eq!(
            s.scope("claude", "seven_day_fable"),
            Scope::Model("fable".into())
        );
        assert_eq!(
            s.scope("codex", "additional_codex_bengalfox_primary"),
            Scope::Model("bengalfox".into())
        );
        assert_eq!(s.scope("claude", "five_hour"), Scope::Account);
        assert_eq!(s.scope("acme", "daily"), Scope::Account);
    }

    #[test]
    fn a_local_source_replaces_the_default() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(SOURCE);
        assert_eq!(Scopes::load_at(Some(p.clone())), Scopes::parse(DEFAULT));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "[acme]\ndaily_turbo = \"model:turbo\"\n").unwrap();
        let s = Scopes::load_at(Some(p.clone())).unwrap();
        assert_eq!(s.scope("acme", "daily_turbo"), Scope::Model("turbo".into()));
        assert_eq!(s.scope("claude", "seven_day_fable"), Scope::Account);
        fs::write(&p, "[acme\n").unwrap();
        assert!(Scopes::load_at(Some(p.clone()))
            .unwrap_err()
            .starts_with(&p.display().to_string()));
    }

    #[test]
    fn bad_sources_name_the_entry() {
        assert_eq!(
            Scopes::parse("[claude]\nx = \"global\"\n").unwrap_err(),
            "claude.x: global is neither account nor model:<name>"
        );
        assert_eq!(
            Scopes::parse("[claude]\nx = \"model:\"\n").unwrap_err(),
            "claude.x: model: needs a name"
        );
        assert_eq!(
            Scopes::parse("claude = 1\n").unwrap_err(),
            "claude is no table of windows"
        );
        assert_eq!(
            Scopes::parse("[claude]\nx = 1\n").unwrap_err(),
            "claude.x is no string"
        );
        assert!(Scopes::parse("[claude\n").is_err());
        assert_eq!(
            Scopes::parse("[acme]\nweekly = \"account\"\n")
                .unwrap()
                .scope("acme", "weekly"),
            Scope::Account
        );
    }
}
