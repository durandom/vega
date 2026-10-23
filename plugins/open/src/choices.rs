//! Which apps can open a target, and the exact commands for each.
//!
//! Rules are compiled in on purpose (v1): a new rule is a code change plus a
//! rebuild, and every rule is covered by a unit test. VS Code comes first
//! wherever it can handle the target.

use std::path::{Path, PathBuf};

use url::Url;

use crate::target::Target;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Open a file at its line.
    VsCode,
    /// Show a folder in the Explorer (needs the vega VS Code extension).
    VsCodeReveal,
    /// Show a changed file's git diff (needs the vega VS Code extension).
    VsCodeDiff,
    Finder,
    MarkEdit,
}

const MARKDOWN_EXTS: &[&str] = &["md", "markdown"];

/// VS Code's bundled CLI; `/opt/homebrew/bin/code` is only a symlink to it.
const VSCODE_CLI: &str = "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code";

/// URI authority of the vega VS Code extension in vscode/ (`<publisher>.<name>`).
const VEGA_EXTENSION: &str = "vega.vega-vscode";

/// Facts about a target that need I/O, gathered by the caller.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// Git root containing the target, if any.
    pub repo: Option<PathBuf>,
    /// The file differs from HEAD (tracked and modified or staged).
    pub changed: bool,
}

/// Apps that fit `t`, best first. Never empty for an existing target.
pub fn choices_for(t: &Target, facts: &Facts) -> Vec<Choice> {
    if t.is_dir() {
        return vec![Choice::VsCodeReveal, Choice::Finder];
    }
    let mut v = vec![Choice::VsCode];
    if facts.changed && facts.repo.is_some() {
        v.push(Choice::VsCodeDiff);
    }
    let ext = t
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if ext.is_some_and(|e| MARKDOWN_EXTS.contains(&e.as_str())) {
        v.push(Choice::MarkEdit);
    }
    v
}

impl Choice {
    pub fn label(self) -> &'static str {
        match self {
            Choice::VsCode => "VS Code",
            Choice::VsCodeReveal => "VS Code: reveal in Explorer",
            Choice::VsCodeDiff => "VS Code: git diff",
            Choice::Finder => "Finder",
            Choice::MarkEdit => "MarkEdit",
        }
    }

    /// Stable name in the remote-open payload.
    pub fn id(self) -> &'static str {
        match self {
            Choice::VsCode => "vscode",
            Choice::VsCodeReveal => "vscode_reveal",
            Choice::VsCodeDiff => "vscode_diff",
            Choice::Finder => "finder",
            Choice::MarkEdit => "markedit",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [
            Choice::VsCode,
            Choice::VsCodeReveal,
            Choice::VsCodeDiff,
            Choice::Finder,
            Choice::MarkEdit,
        ]
        .into_iter()
        .find(|c| c.id() == id)
    }

    /// Commands to run in order, with absolute program paths because the
    /// herdr server's PATH may not include Homebrew.
    ///
    /// VS Code always opens the repo folder first, so the repo's existing
    /// window is reused and is the focused one when an extension URI
    /// follows (VS Code hands URIs to the last focused window).
    pub fn commands(self, t: &Target, repo: Option<&Path>) -> Vec<Vec<String>> {
        let path = t.path.display().to_string();
        let focus = |repo: &Path| vec![VSCODE_CLI.to_string(), repo.display().to_string()];
        match (self, repo) {
            (Choice::VsCode, Some(repo)) => {
                let mut argv = focus(repo);
                argv.extend(["-g".into(), goto_spec(t)]);
                vec![argv]
            }
            (Choice::VsCodeReveal | Choice::VsCodeDiff, Some(repo)) => {
                let action = if self == Choice::VsCodeReveal {
                    "reveal"
                } else {
                    "diff"
                };
                vec![
                    focus(repo),
                    vec![
                        VSCODE_CLI.into(),
                        "--open-url".into(),
                        extension_url(action, &t.path),
                    ],
                ]
            }
            // Outside a repo there is no window to reveal in: open the folder
            // itself, or just the file.
            (Choice::VsCodeReveal, None) => vec![vec![VSCODE_CLI.into(), path]],
            (Choice::VsCode | Choice::VsCodeDiff, None) => {
                vec![vec![VSCODE_CLI.into(), "-g".into(), goto_spec(t)]]
            }
            (Choice::Finder, _) => vec![vec!["/usr/bin/open".into(), "-R".into(), path]],
            (Choice::MarkEdit, _) => vec![vec![
                "/usr/bin/open".into(),
                "-a".into(),
                "MarkEdit".into(),
                path,
            ]],
        }
    }
}

/// Bring `dir` (a repo root or any folder) up in VS Code, reusing the
/// window that already has it open.
pub fn workspace_commands(dir: &Path) -> Vec<Vec<String>> {
    vec![vec![VSCODE_CLI.into(), dir.display().to_string()]]
}

/// The repo's window, then its Source Control view (via the vega extension).
pub fn scm_commands(repo: &Path) -> Vec<Vec<String>> {
    vec![
        vec![VSCODE_CLI.into(), repo.display().to_string()],
        vec![
            VSCODE_CLI.into(),
            "--open-url".into(),
            extension_url("scm", repo),
        ],
    ]
}

/// `vscode://vega.vega-vscode/<action><absolute path>`, path percent-encoded.
fn extension_url(action: &str, path: &Path) -> String {
    let mut url = Url::parse(&format!("vscode://{VEGA_EXTENSION}")).expect("static URL is valid");
    url.set_path(&format!("/{action}{}", path.display()));
    url.into()
}

/// `path[:line[:col]]` for `code -g`.
fn goto_spec(t: &Target) -> String {
    let mut spec = t.path.display().to_string();
    if let Some(line) = t.line {
        spec.push_str(&format!(":{line}"));
        if let Some(col) = t.col {
            spec.push_str(&format!(":{col}"));
        }
    }
    spec
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn target(path: PathBuf, line: Option<u32>, col: Option<u32>) -> Target {
        Target { path, line, col }
    }

    fn facts(repo: Option<&str>, changed: bool) -> Facts {
        Facts {
            repo: repo.map(Into::into),
            changed,
        }
    }

    #[test]
    fn files_offer_vscode_then_diff_when_changed_then_markedit() {
        let dir = tempfile::tempdir().unwrap();
        let md = dir.path().join("Notes.MD");
        fs::write(&md, "").unwrap();
        let t = target(md, None, None);
        assert_eq!(
            choices_for(&t, &facts(Some("/r"), true)),
            vec![Choice::VsCode, Choice::VsCodeDiff, Choice::MarkEdit]
        );
        assert_eq!(
            choices_for(&t, &facts(Some("/r"), false)),
            vec![Choice::VsCode, Choice::MarkEdit]
        );
        let rs = dir.path().join("a.rs");
        fs::write(&rs, "").unwrap();
        assert_eq!(
            choices_for(&target(rs, None, None), &facts(None, true)),
            vec![Choice::VsCode]
        );
    }

    #[test]
    fn dirs_offer_reveal_then_finder() {
        let dir = tempfile::tempdir().unwrap();
        let md_dir = dir.path().join("docs.md");
        fs::create_dir(&md_dir).unwrap();
        assert_eq!(
            choices_for(&target(md_dir, None, None), &Facts::default()),
            vec![Choice::VsCodeReveal, Choice::Finder]
        );
    }

    #[test]
    fn vscode_opens_the_repo_and_goes_to_the_file() {
        let t = target("/r/my dir/a.rs".into(), Some(12), Some(3));
        assert_eq!(
            Choice::VsCode.commands(&t, Some(Path::new("/r"))),
            vec![vec![VSCODE_CLI, "/r", "-g", "/r/my dir/a.rs:12:3"]]
        );
        let t = target("/tmp/a.rs".into(), None, Some(3));
        assert_eq!(
            Choice::VsCode.commands(&t, None),
            vec![vec![VSCODE_CLI, "-g", "/tmp/a.rs"]]
        );
    }

    #[test]
    fn reveal_and_diff_focus_the_repo_then_call_the_extension() {
        let t = target("/r/my dir/src".into(), None, None);
        assert_eq!(
            Choice::VsCodeReveal.commands(&t, Some(Path::new("/r"))),
            vec![
                vec![VSCODE_CLI, "/r"],
                vec![
                    VSCODE_CLI,
                    "--open-url",
                    "vscode://vega.vega-vscode/reveal/r/my%20dir/src"
                ],
            ]
        );
        let t = target("/r/a.rs".into(), Some(3), None);
        assert_eq!(
            Choice::VsCodeDiff.commands(&t, Some(Path::new("/r")))[1][2],
            "vscode://vega.vega-vscode/diff/r/a.rs"
        );
        let t = target("/tmp/d".into(), None, None);
        assert_eq!(
            Choice::VsCodeReveal.commands(&t, None),
            vec![vec![VSCODE_CLI, "/tmp/d"]]
        );
    }

    #[test]
    fn workspace_and_scm_target_the_repo_window() {
        let repo = Path::new("/src/my repo");
        assert_eq!(
            workspace_commands(repo),
            vec![vec![VSCODE_CLI, "/src/my repo"]]
        );
        assert_eq!(
            scm_commands(repo),
            vec![
                vec![VSCODE_CLI, "/src/my repo"],
                vec![
                    VSCODE_CLI,
                    "--open-url",
                    "vscode://vega.vega-vscode/scm/src/my%20repo"
                ],
            ]
        );
    }

    #[test]
    fn finder_and_markedit_use_open() {
        let t = target("/r/notes.md".into(), Some(4), None);
        assert_eq!(
            Choice::Finder.commands(&t, None),
            vec![vec!["/usr/bin/open", "-R", "/r/notes.md"]]
        );
        assert_eq!(
            Choice::MarkEdit.commands(&t, Some(Path::new("/r"))),
            vec![vec!["/usr/bin/open", "-a", "MarkEdit", "/r/notes.md"]]
        );
    }
}
