//! Turn a piece of selected terminal text into an existing file target.
//!
//! Pure functions only: parsing never touches the filesystem, resolving only
//! checks existence. Both are unit-tested without herdr.

use std::path::{Path, PathBuf};

use url::Url;
use vega_herdr::pane::peel;

/// A file reference as written on screen, not yet resolved.
#[derive(Debug, PartialEq, Eq)]
pub struct FileRef {
    pub path: String,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

/// A reference that points at something that exists on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

impl Target {
    pub fn is_dir(&self) -> bool {
        self.path.is_dir()
    }

    /// `path`, `path:line` or `path:line:col`, for display.
    pub fn display(&self) -> String {
        let mut s = self.path.display().to_string();
        if let Some(line) = self.line {
            s.push_str(&format!(":{line}"));
            if let Some(col) = self.col {
                s.push_str(&format!(":{col}"));
            }
        }
        s
    }
}

/// Parse `path[:line[:col]]` out of a selection. Returns `None` for empty
/// text and for URLs other than `file://`.
pub fn parse(text: &str) -> Option<FileRef> {
    let s = peel(text.lines().find(|l| !l.trim().is_empty())?);
    let s = s.trim_end_matches(':');

    // A `file://` URL keeps any `:line` suffix in its path, so decode first
    // and split the position afterwards.
    let decoded;
    let s = if s.starts_with("file://") {
        decoded = Url::parse(s).ok()?.to_file_path().ok()?;
        decoded.to_str()?
    } else if s.contains("://") {
        return None;
    } else {
        s
    };

    let (path, line, col) = split_position(s);
    if path.is_empty() {
        return None;
    }
    Some(FileRef {
        path: path.to_string(),
        line,
        col,
    })
}

/// Split a trailing `:line` or `:line:col` off a path.
fn split_position(s: &str) -> (&str, Option<u32>, Option<u32>) {
    let num = |t: &str| {
        (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
            .then(|| t.parse().ok())
            .flatten()
    };
    let Some((head, last)) = s.rsplit_once(':') else {
        return (s, None, None);
    };
    let Some(n_last) = num(last) else {
        return (s, None, None);
    };
    if let Some((path, mid)) = head.rsplit_once(':') {
        if let Some(n_mid) = num(mid) {
            return (path, Some(n_mid), Some(n_last));
        }
    }
    (head, Some(n_last), None)
}

/// Find the first existing file for `r`. Relative paths are tried against
/// the pane's cwd first, then against the repository root (agents usually
/// print repo-relative paths, even when the pane sits in a subdirectory).
pub fn resolve(
    r: &FileRef,
    cwd: &Path,
    repo_root: Option<&Path>,
    home: Option<&Path>,
) -> Option<Target> {
    let candidates: Vec<PathBuf> = if let Some(rest) = r.path.strip_prefix("~/") {
        home.map(|h| vec![h.join(rest)]).unwrap_or_default()
    } else if Path::new(&r.path).is_absolute() {
        vec![PathBuf::from(&r.path)]
    } else {
        let mut v = vec![cwd.join(&r.path)];
        if let Some(root) = repo_root {
            v.push(root.join(&r.path));
        }
        v
    };
    candidates.into_iter().find(|p| p.exists()).map(|p| Target {
        // Drops `.` and `..` lexically, without following symlinks.
        path: path_clean::clean(&p),
        line: r.line,
        col: r.col,
    })
}

/// Resolve the whole selection, or else its first word that resolves, so a
/// sloppy selection like "see `src/a.rs:3` for" still works. Returns the
/// parsed whole-text ref as well, for error messages.
pub fn find(
    text: &str,
    cwd: &Path,
    repo_root: Option<&Path>,
    home: Option<&Path>,
) -> Result<Target, Option<FileRef>> {
    let whole = parse(text);
    if let Some(t) = whole
        .as_ref()
        .and_then(|r| resolve(r, cwd, repo_root, home))
    {
        return Ok(t);
    }
    text.split_whitespace()
        .filter_map(parse)
        .find_map(|r| resolve(&r, cwd, repo_root, home))
        .ok_or(whole)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn r(path: &str, line: Option<u32>, col: Option<u32>) -> Option<FileRef> {
        Some(FileRef {
            path: path.into(),
            line,
            col,
        })
    }

    #[test]
    fn parses_plain_and_positioned_paths() {
        assert_eq!(parse("src/main.rs"), r("src/main.rs", None, None));
        assert_eq!(parse("src/main.rs:12"), r("src/main.rs", Some(12), None));
        assert_eq!(
            parse("src/main.rs:12:5"),
            r("src/main.rs", Some(12), Some(5))
        );
        assert_eq!(parse("src/main.rs:12:"), r("src/main.rs", Some(12), None));
    }

    #[test]
    fn strips_wrappers_and_punctuation() {
        assert_eq!(parse("`src/a.rs:3`"), r("src/a.rs", Some(3), None));
        assert_eq!(parse("(`src/a.rs:3`)."), r("src/a.rs", Some(3), None));
        assert_eq!(parse("  \"docs/x.md\",\n"), r("docs/x.md", None, None));
        assert_eq!(parse("<README.md>"), r("README.md", None, None));
    }

    #[test]
    fn handles_file_urls_and_rejects_other_urls() {
        assert_eq!(parse("file:///tmp/a%20b.md"), r("/tmp/a b.md", None, None));
        assert_eq!(
            parse("file://localhost/tmp/a.rs:12:3"),
            r("/tmp/a.rs", Some(12), Some(3))
        );
        assert_eq!(parse("file://remote-host/tmp/a.rs"), None);
        assert_eq!(parse("https://example.com/a.rs"), None);
    }

    #[test]
    fn rejects_empty_selection_and_keeps_non_numeric_colons() {
        assert_eq!(parse("   \n  "), None);
        assert_eq!(parse("C:foo"), r("C:foo", None, None));
    }

    #[test]
    fn uses_first_non_empty_line() {
        assert_eq!(
            parse("\n  src/a.rs:1\nsrc/b.rs:2"),
            r("src/a.rs", Some(1), None)
        );
    }

    #[test]
    fn resolves_against_cwd_then_repo_root() {
        let repo = tempfile::tempdir().unwrap();
        let sub = repo.path().join("sub");
        fs::create_dir_all(sub.join("src")).unwrap();
        fs::write(sub.join("src/a.rs"), "").unwrap();
        fs::write(repo.path().join("top.md"), "").unwrap();

        let a = resolve(&parse("src/a.rs:4").unwrap(), &sub, Some(repo.path()), None).unwrap();
        assert_eq!(a.path, sub.join("src/a.rs"));
        assert_eq!(a.line, Some(4));

        let top = resolve(&parse("top.md").unwrap(), &sub, Some(repo.path()), None).unwrap();
        assert_eq!(top.path, repo.path().join("top.md"));

        assert_eq!(
            resolve(&parse("missing.rs").unwrap(), &sub, Some(repo.path()), None),
            None
        );
    }

    #[test]
    fn resolves_home_absolute_and_dotdot() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("n.md"), "").unwrap();
        let t = resolve(
            &parse("~/n.md").unwrap(),
            Path::new("/"),
            None,
            Some(home.path()),
        )
        .unwrap();
        assert_eq!(t.path, home.path().join("n.md"));

        let abs = home.path().join("n.md").display().to_string();
        assert!(resolve(&parse(&abs).unwrap(), Path::new("/"), None, None).is_some());

        fs::create_dir(home.path().join("d")).unwrap();
        let up = resolve(
            &parse("../n.md").unwrap(),
            &home.path().join("d"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(up.path, home.path().join("n.md"));
    }

    #[test]
    fn find_falls_back_to_the_first_resolvable_word() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.rs"), "").unwrap();
        let t = find("see (`src/a.rs:42:3`). and nope.rs", dir.path(), None, None).unwrap();
        assert_eq!(
            (t.path, t.line, t.col),
            (dir.path().join("src/a.rs"), Some(42), Some(3))
        );
        assert!(find("nothing here", dir.path(), None, None).is_err());
    }

    #[test]
    fn display_includes_position() {
        let t = Target {
            path: "/a.rs".into(),
            line: Some(3),
            col: Some(7),
        };
        assert_eq!(t.display(), "/a.rs:3:7");
    }
}
