//! `$XDG_CONFIG_HOME/vega/schedule-approved.toml`: per host, never in a repo,
//! `"<project id>/<job name>" = "<hash>"`. Edits keep the user's comments.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use crate::{write_atomic, Error};

#[derive(Debug, Clone, Default)]
pub struct Approvals {
    doc: DocumentMut,
}

fn key(project_id: &str, job: &str) -> String {
    format!("{project_id}/{job}")
}

impl Approvals {
    pub fn path() -> Option<PathBuf> {
        crate::config_dir().map(|d| d.join("schedule-approved.toml"))
    }

    /// A missing file holds no approvals; a broken one is an error (never
    /// read as "nothing approved" silently, never overwritten).
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(Error::io(path, e)),
        };
        let bad = |m: String| Error::new("approvals_invalid", format!("{}: {m}", path.display()));
        let doc: DocumentMut = text.parse().map_err(|e| bad(format!("{e}")))?;
        for (k, v) in doc.as_table().iter() {
            if v.as_str().is_none() || !k.contains('/') {
                return Err(bad(format!(
                    "{k:?}: want \"<project id>/<job>\" = \"<hash>\""
                )));
            }
        }
        Ok(Approvals { doc })
    }

    pub fn save(&self, path: &Path) -> Result<(), Error> {
        write_atomic(path, self.doc.to_string().as_bytes())
    }

    pub fn get(&self, project_id: &str, job: &str) -> Option<&str> {
        self.doc.get(&key(project_id, job)).and_then(Item::as_str)
    }

    pub fn is_approved(&self, project_id: &str, job: &str, hash: &str) -> bool {
        self.get(project_id, job) == Some(hash)
    }

    pub fn approve(&mut self, project_id: &str, job: &str, hash: &str) {
        self.doc[&key(project_id, job)] = toml_edit::value(hash);
    }

    /// `false` if there was no approval.
    pub fn revoke(&mut self, project_id: &str, job: &str) -> bool {
        self.doc.remove(&key(project_id, job)).is_some()
    }

    /// `(project id, job, hash)`, split at the last `/` (job names hold none).
    pub fn entries(&self) -> Vec<(&str, &str, &str)> {
        self.doc
            .as_table()
            .iter()
            .filter_map(|(k, v)| {
                let (p, j) = k.rsplit_once('/')?;
                Some((p, j, v.as_str()?))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_revoke_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vega/schedule-approved.toml");
        let mut a = Approvals::load(&path).unwrap();
        assert_eq!(a.get("github.com/o/brainz", "fristen"), None);
        a.approve("github.com/o/brainz", "fristen", "abc");
        a.approve("local:proteus:/x", "triage", "def");
        a.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"github.com/o/brainz/fristen\" = \"abc\""),
            "{text}"
        );

        let mut a = Approvals::load(&path).unwrap();
        assert!(a.is_approved("github.com/o/brainz", "fristen", "abc"));
        assert!(!a.is_approved("github.com/o/brainz", "fristen", "abd"));
        assert_eq!(
            a.entries(),
            vec![
                ("github.com/o/brainz", "fristen", "abc"),
                ("local:proteus:/x", "triage", "def")
            ]
        );
        a.approve("github.com/o/brainz", "fristen", "new");
        assert!(a.revoke("local:proteus:/x", "triage"));
        assert!(!a.revoke("local:proteus:/x", "triage"));
        a.save(&path).unwrap();
        let a = Approvals::load(&path).unwrap();
        assert_eq!(a.entries(), vec![("github.com/o/brainz", "fristen", "new")]);
    }

    #[test]
    fn keeps_comments_and_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.toml");
        fs::write(&path, "# approved by hand\n\"p/j\" = \"h\"\n").unwrap();
        let mut a = Approvals::load(&path).unwrap();
        a.approve("p", "k", "i");
        a.save(&path).unwrap();
        assert!(fs::read_to_string(&path)
            .unwrap()
            .starts_with("# approved by hand\n"));
        for bad in ["x = 1", "nokey = \"h\"", "[p]\nj = \"h\"", "= broken"] {
            fs::write(&path, bad).unwrap();
            assert_eq!(
                Approvals::load(&path).unwrap_err().code,
                "approvals_invalid",
                "{bad}"
            );
        }
    }
}
