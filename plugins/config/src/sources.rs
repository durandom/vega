//! Sources: the repo's `config/` defaults, embedded at build time, and the
//! local overrides in `$XDG_CONFIG_HOME/vega/config/` with the same layout.
//! Per file the local one wins (docs/design/config.md "Sources").

use std::fs;
use std::path::{Path, PathBuf};

use crate::Env;

macro_rules! default {
    ($rel:literal) => {
        ($rel, include_str!(concat!("../../../config/", $rel)))
    };
}

/// Every embedded default: (path relative to `config/`, text).
pub const DEFAULTS: [(&str, &str); 16] = [
    default!("herdr/config.toml"),
    default!("instructions/role.md"),
    default!("instructions/interaction.md"),
    default!("instructions/reasoning.md"),
    default!("instructions/determinism.md"),
    default!("instructions/token-efficiency.md"),
    default!("instructions/git-commits.md"),
    default!("instructions/vega.md"),
    default!("claude/output-styles/ELI5.md"),
    default!("claude/output-styles/caveman-eli5.md"),
    default!("claude/output-styles/caveman-lite.md"),
    default!("claude/settings.json"),
    default!("agentsview/LOCAL-SETUP.md"),
    // Read by vega_herdr::capacity (window scopes), not rendered to a target.
    default!("capacity/windows.toml"),
    default!("dcg/config.toml"),
    default!("dcg/packs/oc.yaml"),
];

pub const HERDR: &str = DEFAULTS[0].1;
pub const STYLES_DIR: &str = "claude/output-styles";

/// Claude's and pi's instruction parts.
pub const CLAUDE_PARTS: [&str; 6] = [
    "role",
    "interaction",
    "reasoning",
    "determinism",
    "token-efficiency",
    "vega",
];
/// Codex's instruction parts.
pub const CODEX_PARTS: [&str; 2] = ["git-commits", "vega"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Origin {
    Default,
    Local,
}

/// One resolved source file.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub rel: String,
    pub text: String,
    pub origin: Origin,
}

/// `$XDG_CONFIG_HOME/vega/config`.
pub fn local_dir(env: &Env) -> PathBuf {
    env.config.join("vega/config")
}

pub fn default(rel: &str) -> Option<&'static str> {
    DEFAULTS.iter().find(|(r, _)| *r == rel).map(|(_, t)| *t)
}

fn read_local(env: &Env, rel: &str) -> Result<Option<String>, String> {
    let p = local_dir(env).join(rel);
    match fs::read_to_string(&p) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// The local file when there is one, else the default.
pub fn get(env: &Env, rel: &str) -> Result<Source, String> {
    if let Some(text) = read_local(env, rel)? {
        return Ok(Source {
            rel: rel.into(),
            text,
            origin: Origin::Local,
        });
    }
    default(rel)
        .map(|t| Source {
            rel: rel.into(),
            text: t.into(),
            origin: Origin::Default,
        })
        .ok_or_else(|| format!("no source {rel}"))
}

/// The output styles: the defaults and the local `*.md`, by file name.
pub fn styles(env: &Env) -> Result<Vec<(String, Source)>, String> {
    directory(env, STYLES_DIR, &[".md"])
}

/// Default packs and local YAML additions, resolved per file.
pub fn packs(env: &Env) -> Result<Vec<(String, Source)>, String> {
    directory(env, "dcg/packs", &[".yaml", ".yml"])
}

fn directory(env: &Env, dir: &str, extensions: &[&str]) -> Result<Vec<(String, Source)>, String> {
    let prefix = format!("{dir}/");
    let mut names: Vec<String> = DEFAULTS
        .iter()
        .filter_map(|(r, _)| r.strip_prefix(&prefix).map(str::to_string))
        .collect();
    if let Ok(rd) = fs::read_dir(local_dir(env).join(dir)) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if extensions.iter().any(|ext| n.ends_with(ext)) && !names.contains(&n) {
                names.push(n);
            }
        }
    }
    names.sort();
    names
        .into_iter()
        .map(|n| get(env, &format!("{prefix}{n}")).map(|s| (n, s)))
        .collect()
}

/// The named parts joined by one blank line, and the sources used.
pub fn instructions(env: &Env, parts: &[&str]) -> Result<(String, Vec<Source>), String> {
    let srcs = parts
        .iter()
        .map(|p| get(env, &format!("instructions/{p}.md")))
        .collect::<Result<Vec<_>, _>>()?;
    let text = srcs
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok((text, srcs))
}

/// Every local source file, relative to the local dir, sorted.
pub fn local_files(env: &Env) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else if let Ok(r) = p.strip_prefix(base) {
                out.push(r.to_string_lossy().into_owned());
            }
        }
    }
    let base = local_dir(env);
    let mut out = Vec::new();
    walk(&base, &base, &mut out);
    out.sort();
    out
}

/// Copies the default `rel` to its local path; refuses an existing one.
pub fn init(env: &Env, rel: &str) -> Result<PathBuf, String> {
    let text = default(rel).ok_or_else(|| format!("no default source {rel}"))?;
    let p = local_dir(env).join(rel);
    if p.exists() {
        return Err(format!("{} exists already", p.display()));
    }
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn repo_files(dir: &str) -> BTreeSet<String> {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config")
            .join(dir);
        fs::read_dir(d)
            .unwrap()
            .map(|e| format!("{dir}/{}", e.unwrap().file_name().into_string().unwrap()))
            .collect()
    }

    #[test]
    fn every_repo_source_is_embedded() {
        let embedded: BTreeSet<String> = DEFAULTS.iter().map(|(r, _)| r.to_string()).collect();
        for dir in [
            "herdr",
            "instructions",
            "claude/output-styles",
            "agentsview",
            "capacity",
            "dcg/packs",
        ] {
            for f in repo_files(dir) {
                assert!(embedded.contains(&f), "{f} is not in DEFAULTS");
            }
        }
        for p in CLAUDE_PARTS.iter().chain(&CODEX_PARTS) {
            assert!(default(&format!("instructions/{p}.md")).is_some(), "{p}");
        }
        let v: serde_json::Value =
            serde_json::from_str(default("claude/settings.json").unwrap()).unwrap();
        assert!(v.is_object());
        assert_eq!(
            default("capacity/windows.toml"),
            Some(vega_herdr::capacity::scopes::DEFAULT)
        );
    }

    #[test]
    fn parts_are_sections_joined_by_a_blank_line() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::at(d.path());
        for (r, t) in DEFAULTS
            .iter()
            .filter(|(r, _)| r.starts_with("instructions/"))
        {
            assert!(t.starts_with("# "), "{r}");
            assert!(t.ends_with('\n') && !t.ends_with("\n\n"), "{r}");
        }
        let (claude, srcs) = instructions(&env, &CLAUDE_PARTS).unwrap();
        assert!(claude.starts_with("# Role: Active Partner\n"));
        assert!(claude.contains("\n\n# vega-mux / vega-post\n"));
        assert!(srcs.iter().all(|s| s.origin == Origin::Default));
        let (codex, _) = instructions(&env, &CODEX_PARTS).unwrap();
        assert!(codex.starts_with("# Git commits\n"));
        assert!(codex.contains("from_parent: yes"));
    }

    #[test]
    fn local_wins_adds_styles_and_init_copies_the_default() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::at(d.path());
        assert!(local_files(&env).is_empty());
        let p = init(&env, "instructions/vega.md").unwrap();
        assert_eq!(p, d.path().join(".config/vega/config/instructions/vega.md"));
        assert!(init(&env, "instructions/vega.md")
            .unwrap_err()
            .contains("exists already"));
        assert!(init(&env, "nope.md").is_err());
        fs::write(&p, "# Mine\n").unwrap();
        let s = get(&env, "instructions/vega.md").unwrap();
        assert_eq!((s.text.as_str(), s.origin), ("# Mine\n", Origin::Local));
        let (codex, _) = instructions(&env, &CODEX_PARTS).unwrap();
        assert!(codex.ends_with("\n\n# Mine\n"));
        let styles_dir = local_dir(&env).join(STYLES_DIR);
        fs::create_dir_all(&styles_dir).unwrap();
        fs::write(styles_dir.join("terse.md"), "t\n").unwrap();
        fs::write(styles_dir.join("ELI5.md"), "e\n").unwrap();
        fs::write(styles_dir.join("notes.txt"), "x\n").unwrap();
        let st = styles(&env).unwrap();
        let names: Vec<&str> = st.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["ELI5.md", "caveman-eli5.md", "caveman-lite.md", "terse.md"]
        );
        assert_eq!(st[0].1.origin, Origin::Local);
        assert_eq!(st[1].1.origin, Origin::Default);
        assert_eq!(
            local_files(&env),
            [
                "claude/output-styles/ELI5.md",
                "claude/output-styles/notes.txt",
                "claude/output-styles/terse.md",
                "instructions/vega.md"
            ]
        );
    }
}
