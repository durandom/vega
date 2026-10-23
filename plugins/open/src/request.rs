//! What the user picked in the menu, as data: run here, or sent to the
//! user's machine when the pane lives on another herdr host.
//!
//! The JSON form is the payload of `vega post cmd open`; the receiving
//! `vega-open remote-open` parses it, maps its paths and runs the commands
//! for the same request locally.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::choices::{self, Choice};
use crate::target::Target;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Open a file or folder with one app; `repo` is its git root, if any.
    Open {
        choice: Choice,
        target: Target,
        repo: Option<PathBuf>,
    },
    /// Bring a repo (or folder) up in VS Code.
    Workspace { dir: PathBuf },
    /// The repo's Source Control view.
    Scm { repo: PathBuf },
}

impl Request {
    pub fn commands(&self) -> Vec<Vec<String>> {
        match self {
            Request::Open {
                choice,
                target,
                repo,
            } => choice.commands(target, repo.as_deref()),
            Request::Workspace { dir } => choices::workspace_commands(dir),
            Request::Scm { repo } => choices::scm_commands(repo),
        }
    }

    /// The path the user asked for, for messages.
    pub fn display(&self) -> String {
        match self {
            Request::Open { target, .. } => target.display(),
            Request::Workspace { dir } => dir.display().to_string(),
            Request::Scm { repo } => repo.display().to_string(),
        }
    }

    /// Every path in the request, rewritten by `f`; stops at the first error.
    pub fn map_paths<E>(self, f: impl Fn(&Path) -> Result<PathBuf, E>) -> Result<Self, E> {
        Ok(match self {
            Request::Open {
                choice,
                target,
                repo,
            } => Request::Open {
                choice,
                target: Target {
                    path: f(&target.path)?,
                    ..target
                },
                repo: repo.as_deref().map(&f).transpose()?,
            },
            Request::Workspace { dir } => Request::Workspace { dir: f(&dir)? },
            Request::Scm { repo } => Request::Scm { repo: f(&repo)? },
        })
    }

    pub fn to_json(&self) -> Value {
        match self {
            Request::Open {
                choice,
                target,
                repo,
            } => json!({
                "kind": "open",
                "choice": choice.id(),
                "path": target.path,
                "line": target.line,
                "col": target.col,
                "repo": repo,
            }),
            Request::Workspace { dir } => json!({"kind": "workspace", "path": dir}),
            Request::Scm { repo } => json!({"kind": "scm", "path": repo}),
        }
    }

    /// Strict parse: unknown kinds, choices or fields are errors, and every
    /// path must be absolute.
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let obj = v.as_object().ok_or("payload is not a JSON object")?;
        let kind = obj.get("kind").and_then(Value::as_str).ok_or("no kind")?;
        let allowed: &[&str] = match kind {
            "open" => &["kind", "choice", "path", "line", "col", "repo"],
            "workspace" | "scm" => &["kind", "path"],
            other => return Err(format!("unknown kind {other:?}")),
        };
        if let Some(k) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(format!("unknown field {k:?}"));
        }
        let path = abs_path(obj.get("path")).ok_or("path must be an absolute path")?;
        Ok(match kind {
            "open" => {
                let choice = obj
                    .get("choice")
                    .and_then(Value::as_str)
                    .and_then(Choice::from_id)
                    .ok_or("unknown choice")?;
                let repo = match obj.get("repo") {
                    None | Some(Value::Null) => None,
                    some => Some(abs_path(some).ok_or("repo must be an absolute path")?),
                };
                Request::Open {
                    choice,
                    target: Target {
                        path,
                        line: number(obj.get("line"))?,
                        col: number(obj.get("col"))?,
                    },
                    repo,
                }
            }
            "workspace" => Request::Workspace { dir: path },
            _ => Request::Scm { repo: path },
        })
    }
}

/// An absolute path, never a URL.
fn abs_path(v: Option<&Value>) -> Option<PathBuf> {
    let s = v?.as_str()?;
    (Path::new(s).is_absolute() && !s.contains("://")).then(|| PathBuf::from(s))
}

fn number(v: Option<&Value>) -> Result<Option<u32>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(n) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| "line and col must be positive numbers".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(choice: Choice) -> Request {
        Request::Open {
            choice,
            target: Target {
                path: "/Users/mhild/src/x/a.md".into(),
                line: Some(12),
                col: None,
            },
            repo: Some("/Users/mhild/src/x".into()),
        }
    }

    #[test]
    fn json_round_trips_every_kind() {
        let all = [
            open(Choice::VsCode),
            open(Choice::VsCodeReveal),
            open(Choice::VsCodeDiff),
            open(Choice::Finder),
            open(Choice::MarkEdit),
            Request::Workspace {
                dir: "/Users/mhild/src/x".into(),
            },
            Request::Scm {
                repo: "/Users/mhild/src/x".into(),
            },
        ];
        for r in all {
            assert_eq!(Request::from_json(&r.to_json()).unwrap(), r);
        }
    }

    #[test]
    fn rejects_unknown_and_relative_input() {
        let bad = [
            json!("open"),
            json!({"kind": "shell", "path": "/a"}),
            json!({"kind": "workspace", "path": "a"}),
            json!({"kind": "workspace", "path": "file:///a"}),
            json!({"kind": "open", "choice": "vscode", "path": "/a/https://x"}),
            json!({"kind": "workspace", "path": "/a", "argv": ["sh"]}),
            json!({"kind": "open", "choice": "terminal", "path": "/a"}),
            json!({"kind": "open", "choice": "vscode", "path": "/a", "repo": "r"}),
            json!({"kind": "open", "choice": "vscode", "path": "/a", "line": -1}),
        ];
        for v in bad {
            assert!(Request::from_json(&v).is_err(), "{v}");
        }
    }

    #[test]
    fn map_paths_rewrites_path_and_repo() {
        let mapped = open(Choice::VsCode)
            .map_paths(|p| {
                Ok::<_, ()>(
                    Path::new("/Volumes/mhild").join(p.strip_prefix("/Users/mhild").unwrap()),
                )
            })
            .unwrap();
        assert_eq!(
            mapped.commands()[0][1..],
            ["/Volumes/mhild/src/x", "-g", "/Volumes/mhild/src/x/a.md:12"]
        );
    }
}
