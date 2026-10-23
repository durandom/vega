//! The skills library: a git repo of curated skills that vega clones and
//! manages (docs/design/skills-management.md).
//!
//! - Remote: first line of `$XDG_CONFIG_HOME/vega/skills-remote`.
//! - Global set: `$XDG_CONFIG_HOME/vega/skills-global`, one skill per line;
//!   `lib install -g` adds, `lib remove -g` drops, `lib sync` restores.
//! - Clone: `$XDG_DATA_HOME/vega/skills` (default `~/.local/share/vega/skills`).

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const REMOTE_FILE: &str = "skills-remote";
pub const GLOBAL_FILE: &str = "skills-global";

fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    env::var_os(var)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(fallback)))
}

/// `$XDG_CONFIG_HOME/vega` (default `~/.config/vega`).
pub fn config_dir() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config").map(|c| c.join("vega"))
}

/// `$XDG_DATA_HOME/vega/skills` (default `~/.local/share/vega/skills`).
pub fn clone_dir() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share").map(|d| d.join("vega").join("skills"))
}

pub fn remote_file() -> Option<PathBuf> {
    config_dir().map(|c| c.join(REMOTE_FILE))
}

/// The configured library remote, if any.
pub fn remote() -> Option<String> {
    let text = fs::read_to_string(remote_file()?).ok()?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.to_string())
}

/// The command that sets the remote, for hints.
pub fn set_remote_hint() -> String {
    let file = remote_file().map_or("~/.config/vega/skills-remote".into(), |p| {
        collapse_home(&p.display().to_string())
    });
    format!("echo <git-url> > {file} && vega skills lib sync")
}

/// Skill names in the global set, in file order.
pub fn global_set() -> Vec<String> {
    let Some(file) = config_dir().map(|c| c.join(GLOBAL_FILE)) else {
        return Vec::new();
    };
    fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Rewrites the global set; keeps it sorted and unique.
pub fn write_global_set(names: &[String]) -> Result<(), String> {
    let dir = config_dir().ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut names: Vec<&String> = names.iter().collect();
    names.sort();
    names.dedup();
    let mut text = String::new();
    for n in names {
        text.push_str(n);
        text.push('\n');
    }
    let file = dir.join(GLOBAL_FILE);
    fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))
}

/// One skill in the library clone.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
}

/// The skills in a library checkout, by name. Looks where skills.sh does for
/// a local source: `skills/`, `.agents/skills/` and top-level directories.
/// The first one found wins on a duplicate name.
pub fn skills(root: &Path) -> BTreeMap<String, Skill> {
    let mut out = BTreeMap::new();
    let bases = [
        root.join("skills"),
        root.join(".agents").join("skills"),
        root.to_path_buf(),
    ];
    for base in bases {
        let Ok(entries) = fs::read_dir(&base) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !n.starts_with('.'))
            })
            .filter(|p| p.join("SKILL.md").is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let Ok(text) = fs::read_to_string(dir.join("SKILL.md")) else {
                continue;
            };
            let Some((name, description)) = frontmatter(&text) else {
                continue;
            };
            out.entry(name.clone()).or_insert(Skill {
                name,
                description,
                dir,
            });
        }
    }
    out
}

/// `name` and `description` from SKILL.md frontmatter, leniently: quoted
/// scalars and folded or literal blocks (`>`, `|`) are accepted, other keys ignored.
pub fn frontmatter(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let block: Vec<&str> = lines.take_while(|l| l.trim_end() != "---").collect();
    let value = |key: &str| -> Option<String> {
        let i = block.iter().position(|l| {
            l.strip_prefix(key)
                .is_some_and(|rest| rest.starts_with(':'))
        })?;
        let raw = block[i][key.len() + 1..].trim();
        let v = if raw.is_empty() || raw.starts_with(['>', '|']) {
            block[i + 1..]
                .iter()
                .take_while(|l| l.starts_with([' ', '\t']) || l.trim().is_empty())
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        } else if raw.len() >= 2
            && ((raw.starts_with('"') && raw.ends_with('"'))
                || (raw.starts_with('\'') && raw.ends_with('\'')))
        {
            raw[1..raw.len() - 1].to_string()
        } else {
            raw.to_string()
        };
        Some(v)
    };
    let name = value("name").filter(|n| !n.is_empty())?;
    let description = value("description").unwrap_or_default();
    Some((name, description))
}

/// The owner of each vendored skill, name to source, from the clone's
/// `skills-lock.json` (written by `skills add --copy`).
pub fn owners(clone: &Path) -> BTreeMap<String, String> {
    let Some(json) = fs::read_to_string(clone.join("skills-lock.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    else {
        return BTreeMap::new();
    };
    json["skills"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(n, e)| Some((n.clone(), e["source"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Short HEAD commit of the clone.
pub fn head(clone: &Path) -> Option<String> {
    git(clone, &["rev-parse", "--short=7", "HEAD"]).ok()
}

pub fn origin(clone: &Path) -> Option<String> {
    git(clone, &["remote", "get-url", "origin"]).ok()
}

/// A git URL as skills.sh names a source: `owner/repo` for GitHub, other
/// URLs (and local paths) as they are.
pub fn slug(url: &str) -> String {
    let short = url.trim_end_matches('/').trim_end_matches(".git");
    for prefix in [
        "git@github.com:",
        "ssh://git@github.com/",
        "https://github.com/",
        "http://github.com/",
    ] {
        if let Some(rest) = short.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    url.to_string()
}

/// Runs git in `dir`; returns trimmed stdout, or stderr on failure.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git did not start ({e})"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(last_line(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// Clones the library, or with `pull` fast-forwards an existing clone to its
/// remote (re-pointing origin when the setting changed). Returns what it did.
pub fn fetch(remote: &str, clone: &Path, pull: bool) -> Result<&'static str, String> {
    if clone.join(".git").exists() {
        if !pull {
            return Ok("present");
        }
        if origin(clone).as_deref() != Some(remote) {
            git(clone, &["remote", "set-url", "origin", remote])?;
        }
        let before = head(clone);
        git(clone, &["pull", "--ff-only", "-q"])?;
        return Ok(if head(clone) == before {
            "current"
        } else {
            "pulled"
        });
    }
    if clone.exists() && fs::read_dir(clone).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!(
            "{} exists and is not a git clone",
            collapse_home(&clone.display().to_string())
        ));
    }
    let parent = clone.parent().ok_or("clone dir has no parent")?;
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let out = Command::new("git")
        .args(["clone", "-q", remote])
        .arg(clone)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git did not start ({e})"))?;
    if out.status.success() {
        Ok("cloned")
    } else {
        Err(last_line(&String::from_utf8_lossy(&out.stderr)))
    }
}

fn last_line(s: &str) -> String {
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("failed without output")
        .trim()
        .to_string()
}

/// A directory's files as (relative path, bytes), sorted; symlinks followed.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, out);
            } else if let Ok(bytes) = fs::read(&p) {
                let rel = p.strip_prefix(base).unwrap_or(&p).display().to_string();
                out.push((rel, bytes));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Whether two skill directories hold the same files.
pub fn same_tree(a: &Path, b: &Path) -> bool {
    tree(a) == tree(b)
}

/// `~` for HOME, also for its canonical form (canonicalized paths).
pub fn collapse_home(path: &str) -> String {
    let Some(home) = home() else {
        return path.to_string();
    };
    let canonical = home.canonicalize().unwrap_or_else(|_| home.clone());
    for h in [home, canonical] {
        if let Ok(rest) = Path::new(path).strip_prefix(&h) {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_is_lenient() {
        let plain = "---\nname: a\ndescription: Does a. Use when a.\nlicense: MIT\n---\nbody\n";
        assert_eq!(
            frontmatter(plain),
            Some(("a".into(), "Does a. Use when a.".into()))
        );
        let quoted = "---\nname: 'b'\ndescription: \"x: y\"\n---\n";
        assert_eq!(frontmatter(quoted), Some(("b".into(), "x: y".into())));
        let folded = "---\nname: c\ndescription: >\n  one\n  two\nmetadata:\n  k: v\n---\n";
        assert_eq!(frontmatter(folded), Some(("c".into(), "one two".into())));
        assert_eq!(frontmatter("# no frontmatter\n"), None);
        assert_eq!(frontmatter("---\ndescription: d\n---\n"), None);
    }

    #[test]
    fn skills_scan_known_layouts_first_wins() {
        let dir = tempfile::tempdir().unwrap();
        let mk = |rel: &str, name: &str, desc: &str| {
            let d = dir.path().join(rel);
            fs::create_dir_all(&d).unwrap();
            fs::write(
                d.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\nb\n"),
            )
            .unwrap();
        };
        mk("skills/a", "a", "from skills");
        mk(".agents/skills/b", "b", "vendored");
        mk(".agents/skills/a", "a", "duplicate");
        mk("c", "c", "top level");
        mk(".hidden/d", "d", "ignored");
        let s = skills(dir.path());
        let names: Vec<&str> = s.keys().map(String::as_str).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert_eq!(s["a"].description, "from skills");
    }

    #[test]
    fn slug_shortens_github_urls_only() {
        for url in [
            "git@github.com:durandom/vega-skills.git",
            "https://github.com/durandom/vega-skills",
            "ssh://git@github.com/durandom/vega-skills.git",
        ] {
            assert_eq!(slug(url), "durandom/vega-skills");
        }
        assert_eq!(slug("/srv/lib"), "/srv/lib");
        assert_eq!(slug("git@example.com:a/b.git"), "git@example.com:a/b.git");
    }

    #[test]
    fn same_tree_compares_contents() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for d in [a.path(), b.path()] {
            fs::create_dir_all(d.join("references")).unwrap();
            fs::write(d.join("SKILL.md"), "x").unwrap();
            fs::write(d.join("references/r.md"), "r").unwrap();
        }
        assert!(same_tree(a.path(), b.path()));
        fs::write(b.path().join("references/r.md"), "changed").unwrap();
        assert!(!same_tree(a.path(), b.path()));
    }
}
