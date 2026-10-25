//! Host-local sync records keyed by (project ID, canonical common directory).
//! Runtime reads only v2 files; startup cutover prepares legacy projections.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub fn dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))
        .map(|s| s.join("vega").join("bd"))
}

pub(crate) fn read<T: DeserializeOwned>(file: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(file).ok()?).ok()
}

/// Write to a temp file, then rename: a reader never sees half a file.
pub(crate) fn write<T: Serialize>(file: &Path, value: &T) -> bool {
    let Some(parent) = file.parent() else {
        return false;
    };
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    fs::create_dir_all(parent).is_ok()
        && serde_json::to_vec_pretty(value)
            .ok()
            .is_some_and(|b| fs::write(&tmp, b).is_ok())
        && fs::rename(&tmp, file).is_ok()
}

/// A project `vega bd init` provisioned on this host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// The normalized project ID.
    pub key: String,
    /// Canonical Git common directory on this host, independent per clone.
    pub common: PathBuf,
    /// Where bd runs: the checkout `vega bd init` ran in.
    pub dir: PathBuf,
    /// The Dolt database, on the remote and locally.
    pub database: String,
    /// The remote URL (`<remotesapi>/<database>`).
    pub remote: String,
    /// The bd binary vegad runs (vegad's PATH has no bd).
    pub bd: PathBuf,
    /// bd's data dir (`bd info`: database_path), holding `<database>/.dolt`.
    pub data: PathBuf,
    pub since: i64,
    /// `vega bd resume`: a stop older than this no longer holds.
    #[serde(default)]
    pub resumed: i64,
}

impl Project {
    /// A changed remote requires the explicit journaled rekey. Daemon workers
    /// must skip this location on any identity/configuration failure.
    pub fn validate_location(&self) -> Result<(), vega_herdr::Failure> {
        let current = vega_herdr::project::resolve(&self.common, &vega_herdr::state::post_host())?;
        if current.id.as_str() != self.key || current.location.common_dir != self.common {
            return Err(vega_herdr::project::identity_changed(
                "A provisioned location",
                &self.key,
                &current,
            ));
        }
        Ok(())
    }

    /// The noms dir; its chunk journal changes on every Dolt commit (the
    /// manifest does not), so vegad watches the files' newest mtime.
    pub fn noms(&self) -> PathBuf {
        self.data.join(&self.database).join(".dolt").join("noms")
    }

    /// `.beads`: the parent of the data dir.
    pub fn beads_dir(&self) -> Option<&Path> {
        self.data.parent()
    }
}

/// vegad's sync state of a project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub head: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    /// Last pull attempt and last successful pull / push / full sync.
    #[serde(default)]
    pub pull_tried: i64,
    #[serde(default)]
    pub pulled: i64,
    #[serde(default)]
    pub pushed: i64,
    #[serde(default)]
    pub synced: i64,
    /// The newest journal mtime (ns) vegad acted on.
    #[serde(default)]
    pub seen_write: u128,
    /// A transient error (remote unreachable): retried on the next interval.
    pub error: Option<String>,
    /// Stopped (conflict, rejected push): no sync until `vega bd resume`.
    pub stopped: Option<i64>,
    pub stop_reason: Option<String>,
}

impl State {
    /// Stopped, and not resumed since.
    pub fn is_stopped(&self, p: &Project) -> bool {
        self.stopped.is_some_and(|t| t >= p.resumed)
    }

    /// One word for tables: `stopped`, `error`, `ok`, `pending` (not synced yet).
    pub fn label(&self, p: &Project) -> &'static str {
        if self.is_stopped(p) {
            "stopped"
        } else if self.error.is_some() {
            "error"
        } else if self.synced > 0 {
            "ok"
        } else {
            "pending"
        }
    }
}

/// Versioned envelope validates both dimensions against the filename selector.
#[derive(Serialize, Deserialize)]
pub(crate) struct Record<T> {
    pub schema: u8,
    pub project: String,
    pub common_dir: PathBuf,
    pub value: T,
}

pub(crate) fn relative_file(kind: &str, key: &str, common: &Path) -> Option<PathBuf> {
    use vega_herdr::project::encode_component;
    if !common.is_absolute() {
        return None;
    }
    Some(
        Path::new(kind)
            .join("v2")
            .join(encode_component(key))
            .join(format!("{}.json", encode_component(common.to_str()?))),
    )
}

pub(crate) fn file(kind: &str, key: &str, common: &Path) -> Option<PathBuf> {
    Some(dir()?.join(relative_file(kind, key, common)?))
}

pub(crate) fn load<T: DeserializeOwned>(kind: &str, key: &str, common: &Path) -> Option<T> {
    let r: Record<T> = read(&file(kind, key, common)?)?;
    (r.schema == 2 && r.project == key && r.common_dir == common).then_some(r.value)
}

pub(crate) fn save<T: Serialize>(kind: &str, key: &str, common: &Path, value: &T) -> bool {
    file(kind, key, common).is_some_and(|f| {
        write(
            &f,
            &Record {
                schema: 2,
                project: key.into(),
                common_dir: common.into(),
                value,
            },
        )
    })
}

/// New adoption backups are per location; historical backups stay untouched.
pub fn backup_dir(key: &str, common: &Path, now: i64) -> Option<PathBuf> {
    Some(
        file("backup", key, common)?
            .with_extension("")
            .join(now.to_string())
            .join(".beads"),
    )
}

pub fn project_path(key: &str, common: &Path) -> Option<PathBuf> {
    file("projects", key, common)
}

pub fn save_project(p: &Project) -> bool {
    save("projects", &p.key, &p.common, p)
}

pub fn project(key: &str, common: &Path) -> Option<Project> {
    let p: Project = load("projects", key, common)?;
    (p.key == key && p.common == common).then_some(p)
}

/// Every local location, grouped by project ID then common directory.
pub fn projects() -> Vec<Project> {
    let mut out = Vec::new();
    if let Some(ids) = dir().and_then(|d| fs::read_dir(d.join("projects/v2")).ok()) {
        for id in ids.flatten() {
            if let Ok(locations) = fs::read_dir(id.path()) {
                for entry in locations.flatten() {
                    if let Some(r) = read::<Record<Project>>(&entry.path()) {
                        if file("projects", &r.project, &r.common_dir).as_ref()
                            == Some(&entry.path())
                        {
                            out.extend(project(&r.project, &r.common_dir));
                        }
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.key, &a.common).cmp(&(&b.key, &b.common)));
    out
}

pub fn state(key: &str, common: &Path) -> State {
    load("state", key, common).unwrap_or_default()
}

pub fn save_state(key: &str, common: &Path, s: &State) -> bool {
    save("state", key, common, s)
}

/// The columns `vega bd status` and `vega uac status` print per project:
/// `project,database,ahead,behind,last_sync,state`.
pub const COLUMNS: &str = "project,location,database,ahead,behind,last_sync,state";

/// Add a current resolver failure without replacing the persisted problem.
pub(crate) fn location_problem(
    previous: Option<String>,
    failure: Option<&vega_herdr::Failure>,
) -> Option<String> {
    match failure {
        Some(e) => {
            let current = format!("{}: {}; {}", e.code, e.message, e.hint);
            Some(previous.map_or_else(|| current.clone(), |p| format!("{p}; {current}")))
        }
        None => previous,
    }
}

/// The cells of [`COLUMNS`] (unquoted), and the problem to show below the
/// table: the stop reason, or the transient error.
pub fn row(p: &Project, s: &State, now: i64) -> ([String; 7], Option<String>) {
    let n = |v: Option<u64>| v.map_or("-".to_string(), |v| v.to_string());
    let last = if s.synced > 0 {
        crate::ago(now - s.synced)
    } else {
        "never".into()
    };
    let problem = if s.is_stopped(p) {
        Some(s.stop_reason.clone().unwrap_or_else(|| "stopped".into()))
    } else {
        s.error.clone()
    };
    let failure = p.validate_location().err();
    let problem = location_problem(problem, failure.as_ref());
    (
        [
            p.key.clone(),
            p.common.display().to_string(),
            p.database.clone(),
            n(s.ahead),
            n(s.behind),
            last,
            if failure.is_some() && !s.is_stopped(p) {
                "error"
            } else {
                s.label(p)
            }
            .to_string(),
        ],
        problem,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        Project {
            key: "github.com/team/x".into(),
            common: "/x/.git".into(),
            dir: "/x".into(),
            database: "X".into(),
            remote: "http://r/X".into(),
            bd: "/bin/bd".into(),
            data: "/x/.beads/dolt".into(),
            since: 1,
            resumed: 0,
        }
    }

    #[test]
    fn resume_lifts_an_older_stop_only() {
        let mut p = project();
        let s = State {
            stopped: Some(100),
            ..State::default()
        };
        assert!(s.is_stopped(&p));
        p.resumed = 101;
        assert!(!s.is_stopped(&p));
        assert_eq!(s.label(&p), "pending");
    }

    #[test]
    fn noms_is_below_the_database() {
        assert_eq!(
            project().noms(),
            PathBuf::from("/x/.beads/dolt/X/.dolt/noms")
        );
    }
    #[test]
    fn v2_storage_is_per_clone_and_rejects_mismatched_envelopes() {
        let tmp = tempfile::tempdir().unwrap();
        let prior = std::env::var_os("XDG_STATE_HOME");
        std::env::set_var("XDG_STATE_HOME", tmp.path());
        let a = project();
        let b = Project {
            common: "/second/.bare".into(),
            dir: "/second/main".into(),
            ..a.clone()
        };
        assert!(save_project(&a));
        assert!(save_project(&b));
        let stopped = State {
            stopped: Some(99),
            stop_reason: Some("conflict".into()),
            ..State::default()
        };
        assert!(save_state(&a.key, &a.common, &stopped));
        assert!(state(&a.key, &a.common).is_stopped(&a));
        assert!(!state(&b.key, &b.common).is_stopped(&b));
        let git = crate::git::GitState {
            fetched: 123,
            ..Default::default()
        };
        assert!(crate::git::save_state(&a.key, &a.common, &git));
        assert_eq!(crate::git::state(&b.key, &b.common).fetched, 0);
        assert_eq!(projects().len(), 2);
        let target = project_path(&b.key, &b.common).unwrap();
        std::fs::copy(project_path(&a.key, &a.common).unwrap(), target).unwrap();
        assert!(super::project(&b.key, &b.common).is_none());
        assert_eq!(projects().len(), 1);
        let legacy = dir().unwrap().join("projects/legacy.json");
        std::fs::write(legacy, serde_json::to_vec(&a).unwrap()).unwrap();
        assert_eq!(projects().len(), 1, "no legacy reads");
        let file = file("state", &a.key, &a.common).unwrap();
        let mut record: Record<State> = read(&file).unwrap();
        record.schema = 1;
        assert!(write(&file, &record));
        assert_eq!(state(&a.key, &a.common), State::default());
        match prior {
            Some(value) => std::env::set_var("XDG_STATE_HOME", value),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
    }
}
