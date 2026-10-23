//! Agent skills per checkout (docs/design/janitor.md, Skills): what the
//! harness dirs install and what `skills/<name>/SKILL.md` owns. Read when a
//! checkout is queried, so it refreshes with the git state.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Harness dirs that install skills, then the owner dir; the label is the first segment.
const DIRS: [&str; 5] = [
    ".agents/skills",
    ".claude/skills",
    ".pi/skills",
    ".codex/skills",
    "skills",
];

/// Characters of the description kept in the index.
const DESC_MAX: usize = 100;

/// One skill in one checkout, over all dirs it appears in.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    /// Frontmatter `description`, cut to `DESC_MAX` characters.
    pub description: String,
    pub locs: Vec<Loc>,
    /// `source` from `skills-lock.json`, if it lists the skill.
    pub lock: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Loc {
    /// `.agents`, `.claude`, `.pi`, `.codex` or `skills` (the owner source).
    pub dir: String,
    /// `dir`, `link -> <target>` or `broken`.
    pub kind: String,
}

impl Skill {
    pub fn dirs(&self) -> String {
        let d: Vec<&str> = self.locs.iter().map(|l| l.dir.as_str()).collect();
        d.join(" ")
    }

    /// The distinct kinds, in dir order.
    pub fn kinds(&self) -> String {
        let mut k: Vec<&str> = Vec::new();
        for l in &self.locs {
            if !k.contains(&l.kind.as_str()) {
                k.push(&l.kind);
            }
        }
        k.join(" | ")
    }

    /// The lock's source, `own` for an owner source not in the lock, else `-`.
    pub fn source(&self) -> &str {
        match &self.lock {
            Some(s) => s,
            None if self.locs.iter().any(|l| l.dir == "skills") => "own",
            None => "-",
        }
    }
}

/// The skills of the checkout at `root`, sorted by name.
pub fn collect(root: &Path) -> Vec<Skill> {
    let lock = lock_sources(root);
    let mut out: Vec<Skill> = Vec::new();
    for rel in DIRS {
        let Ok(rd) = fs::read_dir(root.join(rel)) else {
            continue;
        };
        let label = rel.split('/').next().unwrap_or(rel).to_string();
        let mut names: Vec<_> = rd.flatten().collect();
        names.sort_by_key(|e| e.file_name());
        for e in names {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let Some(kind) = kind_of(&path, rel == "skills") else {
                continue;
            };
            let i = match out.iter().position(|s| s.name == name) {
                Some(i) => i,
                None => {
                    out.push(Skill {
                        lock: lock
                            .iter()
                            .find(|(n, _)| *n == name)
                            .map(|(_, s)| s.clone()),
                        name,
                        description: String::new(),
                        locs: Vec::new(),
                    });
                    out.len() - 1
                }
            };
            let s = &mut out[i];
            if s.description.is_empty() {
                if let Ok(text) = fs::read_to_string(path.join("SKILL.md")) {
                    s.description = description(&text);
                }
            }
            s.locs.push(Loc {
                dir: label.clone(),
                kind,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A dir counts with a SKILL.md, a symlink always (an owner source needs a SKILL.md).
fn kind_of(path: &Path, owner: bool) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    let has_md = path.join("SKILL.md").is_file();
    if meta.file_type().is_symlink() {
        if owner && !has_md {
            return None;
        }
        let target = fs::read_link(path).ok()?;
        return Some(if path.exists() {
            format!("link -> {}", target.display())
        } else {
            "broken".into()
        });
    }
    (meta.is_dir() && has_md).then(|| "dir".into())
}

/// (name, source) pairs of `skills-lock.json`.
fn lock_sources(root: &Path) -> Vec<(String, String)> {
    let Ok(raw) = fs::read(root.join("skills-lock.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(map) = v.get("skills").and_then(|s| s.as_object()) else {
        return Vec::new();
    };
    map.iter()
        .map(|(n, s)| {
            let src = s.get("source").and_then(|x| x.as_str()).unwrap_or("?");
            (n.clone(), src.to_string())
        })
        .collect()
}

/// The frontmatter `description`, plain or a folded/literal block, on one line.
pub fn description(text: &str) -> String {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    let fm: Vec<&str> = lines.take_while(|l| l.trim() != "---").collect();
    let Some(i) = fm.iter().position(|l| l.starts_with("description:")) else {
        return String::new();
    };
    let v = fm[i]["description:".len()..].trim();
    let raw = if v.is_empty() || v.starts_with('>') || v.starts_with('|') {
        fm[i + 1..]
            .iter()
            .take_while(|l| l.is_empty() || l.starts_with(char::is_whitespace))
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        let unq = v
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')));
        unq.unwrap_or(v).to_string()
    };
    let one: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > DESC_MAX {
        let cut: String = one.chars().take(DESC_MAX).collect();
        format!("{}…", cut.trim_end())
    } else {
        one
    }
}

/// The owner of a hosted remote (`github.com/o/r` -> `o`); `None` for a local path.
pub fn remote_owner(remote: &str) -> Option<&str> {
    let mut seg = remote.split('/');
    let host = seg.next()?;
    if host.is_empty() || host.starts_with('.') || !host.contains('.') {
        return None;
    }
    seg.next().filter(|o| !o.is_empty())
}

/// The user for `--mine`: `VEGA_JANITOR_OWNER`, else `git config github.user`,
/// else gh's login (read from its config, no network).
pub fn me() -> Option<String> {
    if let Some(o) = std::env::var("VEGA_JANITOR_OWNER")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        return Some(o.trim().to_string());
    }
    let run = |cmd: &str, args: &[&str]| {
        let out = Command::new(cmd).args(args).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !s.is_empty()).then_some(s)
    };
    run("git", &["config", "--get", "github.user"])
        .or_else(|| run("gh", &["config", "get", "-h", "github.com", "user"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_descriptions() {
        assert_eq!(
            description("---\nname: x\ndescription: \"Does x.\"\n---\nbody"),
            "Does x."
        );
        assert_eq!(
            description("---\ndescription: >\n  Folded\n  text.\nname: x\n---\n"),
            "Folded text."
        );
        assert_eq!(description("no frontmatter"), "");
        let long = format!("---\ndescription: {}\n---\n", "a".repeat(150));
        assert_eq!(description(&long).chars().count(), DESC_MAX + 1);
    }

    #[test]
    fn owners_of_remotes() {
        assert_eq!(remote_owner("github.com/durandom/vega"), Some("durandom"));
        assert_eq!(remote_owner("gitlab.com/o/r"), Some("o"));
        assert_eq!(remote_owner("/srv/git/x"), None);
        assert_eq!(remote_owner("../x"), None);
    }
}
