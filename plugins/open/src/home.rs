//! The `vega open` home menu: one row per kind of "open", each with a
//! letter hotkey. A new kind of open is a new `Entry` plus a row here; the
//! herdr keybinding never changes.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// Open what is selected (a file, folder, or later a URL).
    Selection,
    /// Bring the repo (or the cwd) up in VS Code.
    Workspace,
    /// VS Code's Source Control view for the repo's changes.
    Diff,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Row {
    pub key: char,
    pub label: String,
    pub entry: Entry,
}

/// Rows in display order; the first one is preselected, so a fresh
/// selection is one Enter away.
pub fn rows(selection: Option<&str>, repo: Option<&Path>, cwd: &Path, changed: usize) -> Vec<Row> {
    let mut v = Vec::new();
    if let Some(sel) = selection {
        v.push(Row {
            key: 's',
            label: sel.to_string(),
            entry: Entry::Selection,
        });
    }
    let dir = repo.unwrap_or(cwd);
    let name = dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    v.push(Row {
        key: 'w',
        label: format!("workspace {name} in VS Code"),
        entry: Entry::Workspace,
    });
    if repo.is_some() && changed > 0 {
        v.push(Row {
            key: 'd',
            label: format!("git diff ({changed} changed) in VS Code"),
            entry: Entry::Diff,
        });
    }
    v
}

/// The clipboard counts as a selection only while its text is visible on
/// the pane: herdr's `copy_on_select` clears the real selection on mouse
/// release, and a stale clipboard must not hijack the menu.
pub fn fresh_clipboard(clipboard: &str, screen: &str) -> Option<String> {
    let line = clipboard.lines().map(str::trim).find(|l| !l.is_empty())?;
    // Anything longer is a copied block, not a path or URL.
    if line.chars().count() > 500 {
        return None;
    }
    screen.contains(line).then(|| line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(rows: &[Row]) -> String {
        rows.iter().map(|r| r.key).collect()
    }

    #[test]
    fn selection_comes_first_and_diff_needs_changes() {
        let repo = Path::new("/src/vega");
        let r = rows(
            Some("src/main.rs:12"),
            Some(repo),
            Path::new("/src/vega/sub"),
            3,
        );
        assert_eq!(keys(&r), "swd");
        assert_eq!(r[0].label, "src/main.rs:12");
        assert_eq!(r[1].label, "workspace vega in VS Code");
        assert_eq!(r[2].label, "git diff (3 changed) in VS Code");
        assert_eq!(keys(&rows(None, Some(repo), repo, 0)), "w");
    }

    #[test]
    fn without_repo_the_workspace_is_the_cwd_and_there_is_no_diff() {
        let r = rows(None, None, Path::new("/tmp/scratch"), 5);
        assert_eq!(keys(&r), "w");
        assert_eq!(r[0].label, "workspace scratch in VS Code");
    }

    #[test]
    fn clipboard_counts_only_while_visible() {
        let screen = "error: see src/main.rs:12 for details";
        assert_eq!(
            fresh_clipboard("  src/main.rs:12\n", screen).as_deref(),
            Some("src/main.rs:12")
        );
        assert_eq!(fresh_clipboard("old/stale.rs", screen), None);
        assert_eq!(fresh_clipboard("\n \n", screen), None);
        assert_eq!(fresh_clipboard(&"x".repeat(501), &"x".repeat(600)), None);
    }
}
