//! File-only status-tab edits. Stable IDs survive display renames and promotion.
use crate::{
    profiles::{self, Profile, Tab, Terminal},
    Env,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Overlay {
    pub hidden: Vec<String>,
    pub order: Vec<String>,
    pub titles: BTreeMap<String, String>,
    pub added: Vec<Tab>,
}

pub fn path(env: &Env, name: &str) -> PathBuf {
    env.config.join("vega/windows").join(format!("{name}.toml"))
}
pub fn read(path: &Path) -> Result<Overlay, String> {
    match fs::read_to_string(path) {
        Ok(s) => toml_edit::de::from_str(&s).map_err(|e| format!("overlay: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Overlay::default()),
        Err(e) => Err(e.to_string()),
    }
}
pub fn text(p: &Profile) -> Result<String, String> {
    toml_edit::ser::to_string_pretty(p).map_err(|e| e.to_string())
}
pub fn write(path: &Path, text: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("missing parent directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    std::io::Write::write_all(&mut temp, text.as_bytes()).map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn save(path: &Path, overlay: &Overlay) -> Result<(), String> {
    write(
        path,
        &toml_edit::ser::to_string_pretty(overlay).map_err(|e| e.to_string())?,
    )
}
pub fn effective(base: &Profile, overlay: &Overlay) -> Result<Profile, String> {
    let mut p = base.clone();
    let mut ids: BTreeSet<_> = p.tabs.iter().map(|t| t.name.clone()).collect();
    for tab in &overlay.added {
        if !ids.insert(tab.name.clone()) {
            let existing = p.tabs.iter().find(|t| t.name == tab.name).unwrap();
            if serde_json::to_value(&existing.terminals).unwrap()
                == serde_json::to_value(&tab.terminals).unwrap()
            {
                continue; // A promoted addition is now in the applied base.
            }
            return Err(format!("overlay tab collides with repo tab {}", tab.name));
        }
        p.tabs.push(tab.clone());
    }
    p.tabs.retain(|t| !overlay.hidden.contains(&t.name));
    for tab in &mut p.tabs {
        if let Some(title) = overlay.titles.get(&tab.name) {
            tab.title = Some(title.clone());
        }
    }
    p.tabs.sort_by_key(|t| {
        overlay
            .order
            .iter()
            .position(|id| id == &t.name)
            .unwrap_or(usize::MAX)
    });
    profiles::parse(&text(&p)?)
}
pub fn add(base: &Profile, overlay: &mut Overlay, machine: &str) -> Result<String, String> {
    if machine.is_empty() || machine.starts_with('-') || machine.chars().any(char::is_control) {
        return Err("choose a machine name or local".into());
    }
    let used: BTreeSet<_> = base
        .tabs
        .iter()
        .chain(&overlay.added)
        .map(|t| t.name.as_str())
        .chain(base.terminals().map(|t| t.name.as_str()))
        .chain(
            overlay
                .added
                .iter()
                .flat_map(|t| t.terminals.iter().map(|t| t.name.as_str())),
        )
        .collect();
    // Reset leaves terminals alive. Never reuse their old slot IDs for a new shell.
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let name = (0..)
        .map(|i| format!("shell-{:x}", epoch + i))
        .find(|id| !used.contains(id.as_str()))
        .unwrap();
    overlay.added.push(Tab {
        name: name.clone(),
        title: Some(machine.into()),
        terminals: vec![Terminal {
            name: name.clone(),
            machine: machine.into(),
            argv: vec![],
            optional: false,
        }],
    });
    overlay.order.push(name.clone());
    Ok(name)
}
/// An explicit checkout avoids writing embedded sources or guessing a live install path.
pub fn promote(
    repo: &Path,
    name: &str,
    base: &Profile,
    overlay: &Overlay,
) -> Result<PathBuf, String> {
    let destination = repo
        .join("config/herdr-app/profiles")
        .join(format!("{name}.toml"));
    if !repo.join(".git").exists() || !destination.is_file() {
        return Err("--repo must name a vega checkout containing this profile".into());
    }
    let current = profiles::parse(&fs::read_to_string(&destination).map_err(|e| e.to_string())?)?;
    let desired = text(&effective(base, overlay)?)?;
    if text(&current)? == desired {
        return Ok(destination);
    }
    if text(&current)? != text(base)? {
        return Err("repo profile differs from applied base; apply that checkout's profile before promoting".into());
    }
    write(&destination, &desired)?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> Profile {
        profiles::parse(crate::sources::default("herdr-app/profiles/status.toml").unwrap()).unwrap()
    }
    #[test]
    fn patches_roundtrip_hide_rename_reorder_and_reset_without_changing_slots() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("windows/status.toml");
        let base = base();
        let mut o = Overlay::default();
        let id = add(&base, &mut o, "local").unwrap();
        let fresh = add(&base, &mut Overlay::default(), "other").unwrap();
        assert_ne!(id, fresh, "reset must not recycle a live terminal slot");
        o.hidden.push("proteus".into());
        o.titles.insert("tokens".into(), "Token usage 🪙".into());
        o.order = vec!["tokens".into(), id.clone()];
        save(&path, &o).unwrap();
        let p = effective(&base, &read(&path).unwrap()).unwrap();
        assert_eq!(p.tabs[0].title.as_deref(), Some("Token usage 🪙"));
        assert_eq!(p.tabs[0].terminals[0].name, "ganymede-tokens");
        assert_eq!(p.tabs[1].name, id);
        assert!(p.tabs[1].terminals[0].argv.is_empty());
        assert!(!p.tabs.iter().any(|t| t.name == "proteus"));
        assert_eq!(effective(&base, &Overlay::default()).unwrap().tabs.len(), 4);
        o.hidden = base
            .tabs
            .iter()
            .chain(&o.added)
            .map(|t| t.name.clone())
            .collect();
        assert!(effective(&base, &o).unwrap().tabs.is_empty());
        fs::write(&path, "invalid = true").unwrap();
        assert!(read(&path).is_err());
    }
    #[test]
    fn promote_only_changes_checkout_profile_and_rejects_stale_base() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".git"), "gitdir: fake").unwrap();
        let destination = dir.path().join("config/herdr-app/profiles/status.toml");
        let base = base();
        write(&destination, &text(&base).unwrap()).unwrap();
        let mut o = Overlay::default();
        let id = add(&base, &mut o, "remote").unwrap();
        o.titles.insert(id, "Remote shell".into());
        o.hidden.push("io".into());
        promote(dir.path(), "status", &base, &o).unwrap();
        let promoted = profiles::parse(&fs::read_to_string(&destination).unwrap()).unwrap();
        assert_eq!(
            text(&promoted).unwrap(),
            text(&effective(&base, &o).unwrap()).unwrap()
        );
        assert_eq!(
            text(&effective(&promoted, &o).unwrap()).unwrap(),
            text(&promoted).unwrap()
        );
        assert!(promote(dir.path(), "status", &base, &o).is_ok());
        let mut changed = promoted;
        changed.tabs[0].title = Some("Concurrent repo edit".into());
        write(&destination, &text(&changed).unwrap()).unwrap();
        assert!(promote(dir.path(), "status", &base, &o).is_err());
        assert!(!dir.path().join("shared.json").exists());
        assert!(promote(dir.path(), "missing", &base, &o).is_err());
    }
}
